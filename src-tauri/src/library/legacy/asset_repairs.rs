use super::{rows::LegacyRow, LegacyImporter};
use crate::library::{
    asset_repairs::{fanbox_expected_urls, legacy_fingerprint, LegacyAssetRepairMemo},
    Error, Library, ResourceReference, ResourceScope, Result,
};
use rusqlite::{params, OptionalExtension};
use std::sync::atomic::{AtomicBool, Ordering};

impl LegacyImporter {
    pub(super) fn normalize_asset_repairs(
        &self,
        target: &Library,
        cancel: &AtomicBool,
    ) -> Result<()> {
        if !self.has_column("downloads", "asset_repair_fingerprint")? {
            return Ok(());
        }
        let mut prepared = Vec::new();
        self.each_row("downloads", |row| {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::Canceled);
            }
            if row.text("source")? != "fanbox" {
                return Ok(());
            }
            let Some(raw) = row.optional_text("asset_repair_fingerprint")? else {
                return Ok(());
            };
            let memo = LegacyAssetRepairMemo::parse(raw);
            if memo.attempts == 0 {
                return Ok(());
            }
            let old = row.integer("id")?;
            let work = {
                let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
                let tx = conn.transaction()?;
                self.work_from_old(&tx, old)?
            };
            let snapshot = target.current_snapshot(work)?;
            let scope = ResourceScope::Snapshot(snapshot);
            let original = target
                .resource_manifest(work, scope)?
                .resources
                .into_iter()
                .filter(|info| {
                    matches!(info.reference, ResourceReference::Original { .. })
                        && info.file.is_some()
                })
                .min_by_key(|info| info.role != "source_response")
                .ok_or_else(|| Error::Integrity("添付の再取得の対象の原本がありません".into()))?;
            let mut raw = Vec::new();
            target.read_resource(work, scope, &original.reference, None, &mut raw)?;
            let data: serde_json::Value = serde_json::from_slice(&raw)?;
            let expected = fanbox_expected_urls(&data);
            let mut held = Vec::new();
            self.each_row_matching("assets", "download_id", old, |asset: LegacyRow| {
                let usable = asset
                    .optional_text("local_path")?
                    .and_then(|p| self.managed_path(p).ok())
                    .and_then(|p| std::fs::metadata(p).ok())
                    .is_some_and(|m| m.is_file() && m.len() > 0);
                held.push(format!(
                    "{}|{}|{}",
                    asset.optional_text("original_url")?.unwrap_or(""),
                    asset.text("filename")?,
                    usable
                ));
                Ok(())
            })?;
            // 旧指紋が今の旧棚の持ち物と一致するときだけ、回数を新しい参照へ結ぶ。
            // 一致しない覚え書きも表の原記録に残り、新しい状態には流用しない。
            if legacy_fingerprint(&expected, &held) == memo.fingerprint {
                let hash = {
                    let conn = target.reader()?;
                    crate::library::asset_repairs::fingerprint(&conn, snapshot, &expected)?
                };
                prepared.push((work, snapshot, hash, memo.attempts));
            }
            Ok(())
        })?;
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM asset_repair_state WHERE request_id IS NOT NULL)",
            [],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(Error::Integrity("移行後に添付の取得が進んでいます".into()));
        }
        let evidence:Option<Vec<u8>>=tx.query_row("SELECT evidence_file_id FROM migration_records WHERE source_table='downloads' AND source_key='__all_rows__'",[],|r|r.get(0)).optional()?.flatten();
        tx.execute(
            "DELETE FROM asset_repair_state WHERE request_id IS NULL",
            [],
        )?;
        for (work, snapshot, hash, attempts) in prepared {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::Canceled);
            }
            let evidence = evidence
                .as_ref()
                .ok_or_else(|| Error::Integrity("添付の覚え書きの原記録がありません".into()))?;
            tx.execute("INSERT INTO asset_repair_state(work_id,snapshot_id,state_fingerprint,attempts,legacy_evidence_file_id) VALUES(?1,?2,?3,?4,?5)",params![work.as_bytes().as_slice(),snapshot.as_bytes().as_slice(),hash.0.as_slice(),attempts,evidence])?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{EntityKind, LibraryId};
    use rusqlite::Connection;
    struct Fixture(std::path::PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn matching_old_attempts_survive_catchup_and_backup_while_changed_states_start_again() {
        let fixture = Fixture(
            std::env::temp_dir().join(format!("piep-legacy-asset-memo-{}", LibraryId::now_v7())),
        );
        let dir = fixture.0.join("old/downloads/fanbox/one/v1");
        std::fs::create_dir_all(&dir).unwrap();
        let data = serde_json::json!({"id":"one","title":"投稿","type":"file","body":{"files":[{"id":"f","name":"file","extension":"txt","url":"https://example.invalid/file"}]}});
        std::fs::write(
            dir.join("original.json"),
            serde_json::to_vec(&data).unwrap(),
        )
        .unwrap();
        let fingerprint = legacy_fingerprint(&fanbox_expected_urls(&data), &[]);
        let db = Connection::open(fixture.0.join("old/piep.db")).unwrap();
        db.execute_batch("CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER,asset_repair_fingerprint TEXT);
CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT);
CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER);
INSERT INTO download_versions VALUES(1,1,1,'downloads/fanbox/one/v1/original.json','downloads/fanbox/one/v1/original.json','source');").unwrap();
        db.execute(
            "INSERT INTO downloads VALUES(1,'fanbox','one','投稿','',0,?1)",
            [format!("{fingerprint}|3")],
        )
        .unwrap();
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let library = Library::create(&fixture.0.join("new")).unwrap();
        for _ in 0..2 {
            let report = source
                .import(&library, &AtomicBool::new(false), |_| {})
                .unwrap();
            assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        }
        let work = library
            .entity_by_key(EntityKind::Work, "fanbox", "one")
            .unwrap()
            .unwrap();
        let snapshot = library.current_snapshot(work).unwrap();
        let urls = fanbox_expected_urls(&data);
        assert_eq!(
            library
                .asset_repair_plan(work, snapshot, &urls)
                .unwrap()
                .attempts,
            3
        );
        library.backup(&fixture.0.join("backup")).unwrap();
        let copy = Library::open(&fixture.0.join("backup")).unwrap();
        assert_eq!(
            copy.asset_repair_plan(work, snapshot, &urls)
                .unwrap()
                .attempts,
            3
        );
        copy.verify().unwrap();
        drop(copy);
        db.execute(
            "UPDATE downloads SET asset_repair_fingerprint='different|3'",
            [],
        )
        .unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        assert_eq!(
            library
                .asset_repair_plan(work, snapshot, &urls)
                .unwrap()
                .attempts,
            0
        );
        library.verify().unwrap();
    }
}
