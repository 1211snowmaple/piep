use super::{rows::mark_row, LegacyImporter};
use crate::library::{Error, Library, LibraryId, Result};
use rusqlite::{params, OptionalExtension};
use std::sync::atomic::{AtomicBool, Ordering};

impl LegacyImporter {
    pub(super) fn validate_migration_target(&self, target: &Library) -> Result<()> {
        let conn = target.conn.lock().map_err(|_| Error::Busy)?;
        // 再開時は、各表の原記録のハッシュを preserve_tables で照合する。
        let resuming:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM migration_records WHERE source_key='__all_rows__' AND evidence_file_id IS NOT NULL)",[],|r|r.get(0))?;
        if resuming {
            let advanced:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM acquisition_observations WHERE route NOT IN ('legacy-import','legacy-profile-import','legacy-current-row')) OR EXISTS(SELECT 1 FROM acquisition_attempts)",[],|r|r.get(0))?;
            if advanced {
                return Err(Error::Integrity(
                    "移行後に取得や利用者の変更が保存されています。移行先は新しい空のライブラリにしてください"
                        .into(),
                ));
            }
            // 移行器自身が作った初回のお気に入りは許可するが、通常の利用者操作後には再適用しない。
            self.validate_imported_user_operations(target, &conn)?;
            return Ok(());
        }
        let generation: i64 = conn.query_row(
            "SELECT generation FROM library_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        if generation != 0 {
            return Err(Error::Integrity(
                "移行先は新しい空のライブラリにしてください".into(),
            ));
        }
        let mut stmt=conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT GLOB 'sqlite_*' AND name NOT IN ('library_meta','schema_migrations')")?;
        let tables = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for table in tables {
            let quoted = table.replace('"', "\"\"");
            let populated: bool = conn.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM \"{quoted}\")"),
                [],
                |r| r.get(0),
            )?;
            if populated {
                return Err(Error::Integrity(
                    "移行先は新しい空のライブラリか、同じ移行を再開するライブラリにしてください"
                        .into(),
                ));
            }
        }
        Ok(())
    }

    fn validate_imported_user_operations(
        &self,
        target: &Library,
        conn: &rusqlite::Connection,
    ) -> Result<()> {
        let reject = || {
            Error::Integrity("移行後に利用者の変更が保存されています。移行先は新しい空のライブラリにしてください".into())
        };
        let mut stmt = conn.prepare(
            "SELECT entity_id,operation,payload_json FROM outbox WHERE operation<>'capture'",
        )?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let operation: String = row.get(1)?;
            if operation != "user_state" {
                return Err(reject());
            }
            let work: Vec<u8> = row.get(0)?;
            let work_id = LibraryId::from_slice(&work).map_err(|_| reject())?;
            let (provider,key):(String,String)=conn.query_row("SELECT provider,external_key FROM external_identities WHERE entity_id=?1 AND kind='work'",[work.as_slice()],|r|Ok((r.get(0)?,r.get(1)?)))?;
            // 移行が書いた利用者の状態は、旧棚のお気に入りと監視をそのまま写した一回だけ。
            let watch = if self.has_column("downloads", "watch_updates")? {
                "watch_updates"
            } else {
                "0"
            };
            let old: Option<(bool, bool)> = self
                .conn
                .query_row(
                    &format!(
                        "SELECT favorite,{watch} FROM downloads WHERE source=?1 AND source_id=?2"
                    ),
                    params![provider, key],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((favorite, watching)) = old.filter(|(f, w)| *f || *w) else {
                return Err(reject());
            };
            let payload: String = row.get(2)?;
            let payload: serde_json::Value = serde_json::from_str(&payload)?;
            let expected = serde_json::json!({"request":{"work":work_id,"user":target.id,"expected":0,"favorite":favorite,"watching":watching},"result":{"favorite":favorite,"watching":watching,"generation":1}});
            if payload != expected {
                return Err(reject());
            }
            let state:Option<(bool,bool,i64)>=conn.query_row("SELECT favorite,watching,generation FROM work_user_state WHERE work_id=?1 AND user_id=?2",params![work.as_slice(),target.id.as_bytes().as_slice()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            if state != Some((favorite, watching, 1)) {
                return Err(reject());
            }
        }
        Ok(())
    }

    pub(super) fn normalize_current_versions(
        &self,
        target: &Library,
        cancel: &AtomicBool,
    ) -> Result<()> {
        // この列のない旧形式では、既存の取得順による取り込みから現在版を推測し直さない。
        if !self.has_column("downloads", "current_version")? {
            return Ok(());
        }
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let mut failures = Vec::new();
        self.each_row("downloads",|row| {
            if cancel.load(Ordering::Acquire) { return Err(Error::Canceled) }
            let old=row.integer("id")?;
            let checked=(||->Result<LibraryId> {
                let number=row.integer("current_version")?;
                let mut stmt=self.conn.prepare("SELECT id FROM download_versions WHERE download_id=?1 AND version=?2")?;
                let ids=stmt.query_map(params![old,number],|r|r.get::<_,i64>(0))?.collect::<std::result::Result<Vec<_>,_>>()?;
                if ids.len()!=1 { return Err(Error::Integrity("現在版に対応する保存版が一意ではありません".into())) }
                let key=format!("{old}:{}:{number}",ids[0]);
                let expected:Option<Vec<u8>>=tx.query_row("SELECT target_id FROM migration_records WHERE source_table='download_versions' AND source_key=?1 AND status='imported'",[key],|r|r.get(0)).optional()?.flatten();
                let expected=expected.ok_or_else(||Error::Integrity("現在版の保存状態を復元できていません".into()))?;
                let current:Option<Vec<u8>>=tx.query_row("SELECT c.snapshot_id FROM external_identities e JOIN entity_current c ON c.entity_id=e.entity_id WHERE e.provider=?1 AND e.kind='work' AND e.external_key=?2",params![row.text("source")?,row.text("source_id")?],|r|r.get(0)).optional()?;
                if current.as_deref()!=Some(expected.as_slice()) { return Err(Error::Integrity("旧棚の現在版と移行先の現在版が一致しません".into())) }
                LibraryId::from_slice(&expected).map_err(|_|Error::Integrity("現在版の識別が不正です".into()))
            })();
            match checked {
                Ok(snapshot)=>mark_row(&tx,"download_current",&old.to_string(),snapshot)?,
                Err(error)=>{
                    tx.execute("INSERT INTO migration_records(source_table,source_key,status) VALUES('download_current',?1,'unresolved') ON CONFLICT(source_table,source_key) DO UPDATE SET status='unresolved'",[old.to_string()])?;
                    failures.push(format!("作品 {old}: {error}"));
                }
            }
            Ok(())
        })?;
        tx.commit()?;
        if failures.is_empty() {
            Ok(())
        } else {
            Err(Error::Integrity(format!(
                "{} 件の現在版を照合できませんでした。{}",
                failures.len(),
                failures[0]
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Capture, EntityKind};
    use std::{collections::BTreeMap, fs, path::PathBuf};
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    impl Fixture {
        fn new(current: i64) -> Self {
            let root = std::env::temp_dir().join(format!("piep-current-{}", LibraryId::now_v7()));
            fs::create_dir_all(root.join("old")).unwrap();
            let conn = rusqlite::Connection::open(root.join("old/piep.db")).unwrap();
            conn.execute_batch("CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER,current_version INTEGER);
CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT);
CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER);").unwrap();
            conn.execute(
                "INSERT INTO downloads VALUES(1,'pixiv','one','title 2','author',0,?1)",
                [current],
            )
            .unwrap();
            for number in [1, 2] {
                let relative = format!("downloads/pixiv/one/v{number}/original.json");
                let path = root.join("old").join(&relative);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path,serde_json::json!({"detail":{"title":format!("title {number}"),"user":{"id":1,"name":"author"}},"text":format!("body {number}")}).to_string()).unwrap();
                conn.execute(
                    "INSERT INTO download_versions VALUES(?1,1,?1,?2,?2,'source')",
                    params![number, relative],
                )
                .unwrap();
            }
            Self(root)
        }
    }

    fn capture(key: &str) -> Capture {
        Capture {
            request_id: LibraryId::now_v7(),
            acquired_at: None,
            provider: "pixiv".into(),
            external_key: key.into(),
            kind: EntityKind::Work,
            route: "test".into(),
            raw: b"{}".to_vec().into(),
            metadata: BTreeMap::from([("title".into(), serde_json::json!("new title"))]),
            field_states: BTreeMap::new(),
            body: Some("new body".into()),
            media: vec![],
            attachments: vec![],
        }
    }

    #[test]
    fn first_migration_rejects_existing_works_and_even_unreferenced_originals_without_writing() {
        let fixture = Fixture::new(2);
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        for (name, work) in [("with-work", true), ("with-file", false)] {
            let target = Library::create(&fixture.0.join(name)).unwrap();
            if work {
                target.capture(capture("other")).unwrap();
            } else {
                target
                    .store_original(&b"unreferenced original".to_vec().into())
                    .unwrap();
            }
            let before = serde_json::to_value(target.objects.stats().unwrap()).unwrap();
            let files = target.verify().unwrap();
            assert!(matches!(
                source.import(&target, &AtomicBool::new(false), |_| {}),
                Err(Error::Integrity(_))
            ));
            assert_eq!(target.verify().unwrap(), files);
            assert_eq!(
                serde_json::to_value(target.objects.stats().unwrap()).unwrap(),
                before
            );
            assert_eq!(
                target
                    .conn
                    .lock()
                    .unwrap()
                    .query_row("SELECT count(*) FROM migration_records", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn explicit_current_versions_are_verified_and_unrestored_pointers_block_activation() {
        for number in [0, 1, 2, 3] {
            let fixture = Fixture::new(number);
            let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
            let target = Library::create(&fixture.0.join("new")).unwrap();
            let first = source
                .import(&target, &AtomicBool::new(false), |_| {})
                .unwrap();
            assert_eq!(first.snapshots, 2);
            let work = target.works(1).unwrap().remove(0);
            assert_eq!(target.snapshots(work.id).unwrap().len(), 2);
            if number == 2 {
                assert!(first.unresolved.is_empty(), "{:?}", first.unresolved);
                let mapped:Vec<u8>=target.conn.lock().unwrap().query_row("SELECT target_id FROM migration_records WHERE source_table='download_current' AND source_key='1' AND status='imported'",[],|r|r.get(0)).unwrap();
                assert_eq!(mapped, work.snapshot_id.as_bytes());
            } else {
                assert!(!first.unresolved.is_empty());
                assert!(!target.activation_problems().unwrap().is_empty());
            }
            let again = source
                .import(&target, &AtomicBool::new(false), |_| {})
                .unwrap();
            assert_eq!(again.unresolved, first.unresolved);
            assert_eq!(target.snapshots(work.id).unwrap().len(), 2);
            target.verify().unwrap();
        }
    }

    #[test]
    fn a_pending_acquisition_blocks_migration_resume_before_publishing_any_source_result() {
        let fixture = Fixture::new(2);
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let target = Library::create(&fixture.0.join("new")).unwrap();
        source
            .import(&target, &AtomicBool::new(false), |_| {})
            .unwrap();
        let before = target.works(1).unwrap().remove(0);
        let request = LibraryId::now_v7();
        target
            .begin_acquisition(request, "pixiv", EntityKind::Work, "one", "test")
            .unwrap();
        assert!(matches!(
            source.import(&target, &AtomicBool::new(false), |_| {}),
            Err(Error::Integrity(_))
        ));
        assert_eq!(
            target.current_snapshot(before.id).unwrap(),
            before.snapshot_id
        );
        assert_eq!(target.snapshots(before.id).unwrap().len(), 2);
        target.verify().unwrap();
    }

    #[test]
    fn user_favorite_changes_block_resume_while_imported_initial_favorites_allow_retry() {
        for initially_favorite in [false, true] {
            let fixture = Fixture::new(2);
            {
                let old = rusqlite::Connection::open(fixture.0.join("old/piep.db")).unwrap();
                old.execute("UPDATE downloads SET favorite=?1", [initially_favorite])
                    .unwrap();
            }
            let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
            let target = Library::create(&fixture.0.join("new")).unwrap();
            assert!(source
                .import(&target, &AtomicBool::new(false), |_| {})
                .unwrap()
                .unresolved
                .is_empty());
            assert!(source
                .import(&target, &AtomicBool::new(false), |_| {})
                .unwrap()
                .unresolved
                .is_empty());
            let work = target.works(1).unwrap().remove(0);
            let state = target.user_state(work.id, target.id).unwrap();
            let changed = target
                .set_favorite(
                    LibraryId::now_v7(),
                    work.id,
                    target.id,
                    state.generation,
                    !initially_favorite,
                )
                .unwrap();
            assert!(matches!(
                source.import(&target, &AtomicBool::new(false), |_| {}),
                Err(Error::Integrity(_))
            ));
            assert_eq!(target.user_state(work.id, target.id).unwrap(), changed);
            assert_eq!(target.current_snapshot(work.id).unwrap(), work.snapshot_id);
            assert_eq!(target.snapshots(work.id).unwrap().len(), 2);
            assert!(target.activation_problems().unwrap().is_empty());
        }
    }

    #[test]
    fn importing_again_after_a_new_acquisition_leaves_the_new_library_and_migration_state_untouched(
    ) {
        let fixture = Fixture::new(2);
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let target = Library::create(&fixture.0.join("new")).unwrap();
        assert!(source
            .import(&target, &AtomicBool::new(false), |_| {})
            .unwrap()
            .unresolved
            .is_empty());
        let later = target.capture(capture("one")).unwrap();
        let count = target
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM migration_records", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap();
        assert!(matches!(
            source.import(&target, &AtomicBool::new(false), |_| {}),
            Err(Error::Integrity(_))
        ));
        assert_eq!(
            target.current_snapshot(later.entity_id).unwrap(),
            later.snapshot_id
        );
        assert_eq!(target.snapshots(later.entity_id).unwrap().len(), 3);
        assert!(target.activation_problems().unwrap().is_empty());
        assert_eq!(
            target
                .conn
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM migration_records", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            count
        );
        target.verify().unwrap();
    }
}
