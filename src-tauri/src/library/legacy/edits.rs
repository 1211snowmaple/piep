use super::{
    rows::{mark_row, mark_table, marked, LegacyRow},
    LegacyImporter,
};
use crate::library::{
    editing::EditHeader, repository::PreparedFile, EditBlock, EditContent, EditMedia, Error,
    Library, LibraryId, Result,
};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};
use std::collections::BTreeMap;

struct PreparedEdit {
    row: LegacyRow,
    /// 前の移行で作った編集。追いつきで中身が同じなら作り直さない。
    previous: Option<LibraryId>,
    content: PreparedFile,
    document: EditContent,
    blocks: Vec<(i64, EditBlock)>,
    media: BTreeMap<i64, PreparedMedia>,
}

/// 作品の編集の指し先（有効・下書き）。
type Pointers = (Option<Vec<u8>>, Option<Vec<u8>>);

struct PreparedMedia {
    file: PreparedFile,
    name: String,
    mime: Option<String>,
}

impl LegacyImporter {
    pub(super) fn normalize_edits(&self, target: &Library) -> Result<()> {
        {
            let conn = target.conn.lock().map_err(|_| Error::Busy)?;
            if marked(&conn, "work_edit_revisions")? && marked(&conn, "work_edit_blocks")? {
                return Ok(());
            }
        }
        // 追いつき：前に移した編集とブロックの ID を引き継ぐ。ブロックの ID が同じなら、
        // 中身の変わらない編集は同じ文書になり、作り直さずに済む。
        let (revision_ids, block_ids) = {
            let conn = target.conn.lock().map_err(|_| Error::Busy)?;
            let load = |table: &str| -> Result<BTreeMap<String, LibraryId>> {
                let mut stmt = conn.prepare("SELECT source_key,target_id FROM migration_records WHERE source_table=?1 AND target_id IS NOT NULL")?;
                let rows = stmt.query_map([table], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
                })?;
                let mut map = BTreeMap::new();
                for row in rows {
                    let (key, id) = row?;
                    map.insert(
                        key,
                        LibraryId::from_slice(&id)
                            .map_err(|_| Error::Integrity("移行対応表の ID が不正です".into()))?,
                    );
                }
                Ok(map)
            };
            (load("work_edit_revisions")?, load("work_edit_blocks")?)
        };
        // 元画像の走査や圧縮の間は、新 DB の書き込みトランザクションを持たない。
        let mut edits = Vec::new();
        let mut block_count = 0;
        self.each_row("work_edit_revisions", |row| {
            let old = row.integer("id")?;
            let work = row.integer("download_id")?;
            if !matches!(row.text("status")?, "active" | "draft" | "archived") {
                return Err(Error::Integrity(format!("編集 {old} の状態が不明です")));
            }
            let previous = revision_ids.get(&old.to_string()).copied();
            let mut blocks = Vec::new();
            let mut media = BTreeMap::new();
            self.each_row_matching("work_edit_blocks", "edit_revision_id", old, |block| {
                let asset = block.optional_integer("asset_id")?;
                if let Some(asset) = asset {
                    if let std::collections::btree_map::Entry::Vacant(entry) = media.entry(asset) {
                        let (owner, stored, name, mime, size): (i64, String, String, Option<String>, i64) = self.conn.query_row(
                            "SELECT download_id,local_path,filename,mime_type,file_size_bytes FROM assets WHERE id=?1", [asset],
                            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
                        )?;
                        if owner != work {
                            return Err(Error::Integrity(format!("編集 {old} の画像 {asset} は別作品に属しています")));
                        }
                        let path = self.managed_path(&stored)?;
                        if size < 0 || std::fs::metadata(&path)?.len() != size as u64 {
                            return Err(Error::Integrity(format!("編集画像 {asset} の容量が記録と違います")));
                        }
                        entry.insert(PreparedMedia { file: target.store_path(&path)?, name, mime });
                    }
                }
                let attrs = block.optional_text("attrs_json")?
                    .map(serde_json::from_str).transpose()?.unwrap_or(Value::Null);
                let old_block = block.integer("id")?;
                blocks.push((old_block, EditBlock {
                    id: block_ids.get(&old_block.to_string()).copied().unwrap_or_else(LibraryId::now_v7), ordinal: block.integer("block_order")?,
                    kind: block.text("block_type")?.into(), text: block.optional_text("text")?.map(str::to_owned),
                    attrs, media_slot: asset.map(|id|format!("legacy-asset:{id}")),
                }));
                Ok(())
            })?;
            blocks.sort_by_key(|(_, b)| b.ordinal);
            block_count += blocks.len();
            let document = EditContent {format:"piep-edit-v1".into(),title:row.optional_text("title")?.map(str::to_owned),
                blocks:blocks.iter().map(|(_,b)|b.clone()).collect(),
                media:media.iter().map(|(old,m)|EditMedia {slot:format!("legacy-asset:{old}"),file:m.file.file.id,original_name:m.name.clone(),mime_type:m.mime.clone()}).collect(),
                provenance:Some(json!({"legacy":row.0}))};
            let content = target.store_bytes(&document.encode()?)?;
            edits.push(PreparedEdit { row, previous, content, document, blocks, media });
            Ok(())
        })?;
        let total_blocks = self.each_row("work_edit_blocks", |_| Ok(()))?;
        if block_count != total_blocks {
            return Err(Error::Integrity(
                "所属する編集がないブロックがあります".into(),
            ));
        }
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        if marked(&tx, "work_edit_revisions")? && marked(&tx, "work_edit_blocks")? {
            return Ok(());
        }
        // 作品ごとの今の指し先。旧棚の今の状態に合わせ、変わった作品だけ世代を進める。
        let mut current: BTreeMap<LibraryId, Pointers> = BTreeMap::new();
        {
            let mut stmt = tx.prepare(
                "SELECT work_id,active_id,draft_id FROM edit_documents WHERE user_id=?1",
            )?;
            let rows = stmt.query_map([target.id.as_bytes().as_slice()], |r| {
                Ok((r.get::<_, Vec<u8>>(0)?, r.get(1)?, r.get(2)?))
            })?;
            for row in rows {
                let (work, active, draft) = row?;
                let work = LibraryId::from_slice(&work)
                    .map_err(|_| Error::Integrity("編集の作品 ID が不正です".into()))?;
                current.insert(work, (active, draft));
            }
        }
        let mut wanted: BTreeMap<LibraryId, Pointers> =
            current.keys().map(|work| (*work, (None, None))).collect();
        // 旧棚の「以前の編集版」。切り替える前は旧棚が正なので、一覧は旧棚に合わせて作り直す。
        let mut archived: Vec<(LibraryId, LibraryId)> = Vec::new();
        for edit in edits {
            let old = edit.row.integer("id")?;
            let old_work = edit.row.integer("download_id")?;
            let version = edit.row.integer("base_version")?;
            let work = self.work_from_old(&tx, old_work)?;
            // 最新版への付け替えをしない。元の版を一意に復元できない場合は移行を止める。
            let mut versions = self
                .conn
                .prepare("SELECT id FROM download_versions WHERE download_id=?1 AND version=?2")?;
            let versions = versions
                .query_map(params![old_work, version], |r| r.get::<_, i64>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            if versions.len() != 1 {
                return Err(Error::Integrity(format!(
                    "編集 {old} の基準版 {version} が一意ではありません"
                )));
            }
            let key = format!("{old_work}:{}:{version}", versions[0]);
            let base:Option<Vec<u8>>=tx.query_row("SELECT target_id FROM migration_records WHERE source_table='download_versions' AND source_key=?1 AND status='imported'",[key],|r|r.get(0)).optional()?.flatten();
            let base = base.ok_or_else(|| {
                Error::Integrity(format!("編集 {old} の基準版は移行できていません"))
            })?;
            for media in edit.media.values() {
                Library::register_file(&tx, &media.file)?;
            }
            let base = LibraryId::from_slice(&base)
                .map_err(|_| Error::Integrity("編集基準の識別子が不正です".into()))?;
            let stored: Option<Vec<u8>> = match edit.previous {
                Some(previous) => tx
                    .query_row(
                        "SELECT content_file_id FROM edit_revisions WHERE id=?1 AND work_id=?2",
                        params![previous.as_bytes().as_slice(), work.as_bytes().as_slice()],
                        |r| r.get(0),
                    )
                    .optional()?,
                None => None,
            };
            let id = match (edit.previous, stored) {
                (Some(previous), Some(file)) if file == edit.content.file.id.0 => previous,
                (previous, stored) => {
                    // 旧棚で書き換えた編集は、前の編集を親にした新しい編集として残す。
                    let id = LibraryId::now_v7();
                    Library::insert_edit(
                        &tx,
                        EditHeader {
                            id,
                            work,
                            base,
                            parent: previous.filter(|_| stored.is_some()),
                            created_at: edit.row.time("created_at")?,
                        },
                        &edit.document,
                        &edit.content,
                    )?;
                    id
                }
            };
            for (old, media) in &edit.media {
                mark_row(&tx, "edit_asset_placements", &old.to_string(), id)?;
                tx.execute("UPDATE migration_records SET evidence_file_id=?2 WHERE source_table='edit_asset_placements' AND source_key=?1",params![old.to_string(),media.file.file.id.0.as_slice()])?;
            }
            for (old, block) in edit.blocks {
                mark_row(&tx, "work_edit_blocks", &old.to_string(), block.id)?;
            }
            let status = edit.row.text("status")?;
            if status == "archived" {
                archived.push((work, id));
            } else {
                let slot = wanted.entry(work).or_default();
                let place = if status == "active" {
                    &mut slot.0
                } else {
                    &mut slot.1
                };
                if place.is_some() {
                    return Err(Error::Integrity(format!(
                        "作品 {old_work} に複数の {status} 編集があります"
                    )));
                }
                *place = Some(id.as_bytes().to_vec());
            }
            mark_row(&tx, "work_edit_revisions", &old.to_string(), id)?;
        }
        for (work, (active, draft)) in wanted {
            let before = current.get(&work).cloned().unwrap_or_default();
            if before == (active.clone(), draft.clone()) {
                continue;
            }
            tx.execute("INSERT INTO edit_documents(work_id,user_id,generation) VALUES(?1,?2,0) ON CONFLICT(work_id,user_id) DO NOTHING",params![work.as_bytes().as_slice(),target.id.as_bytes().as_slice()])?;
            tx.execute("UPDATE edit_documents SET active_id=?3,draft_id=?4,generation=generation+1 WHERE work_id=?1 AND user_id=?2",params![work.as_bytes().as_slice(),target.id.as_bytes().as_slice(),active,draft])?;
            if before.0 != active {
                target.refresh_work_list(&tx, work)?;
            }
        }
        tx.execute(
            "DELETE FROM edit_archive WHERE user_id=?1",
            [target.id.as_bytes().as_slice()],
        )?;
        for (work, edit) in archived {
            tx.execute(
                "INSERT OR IGNORE INTO edit_archive(work_id,user_id,edit_id) VALUES(?1,?2,?3)",
                params![
                    work.as_bytes().as_slice(),
                    target.id.as_bytes().as_slice(),
                    edit.as_bytes().as_slice()
                ],
            )?;
        }
        mark_table(&tx, "work_edit_revisions")?;
        mark_table(&tx, "work_edit_blocks")?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::Path, sync::atomic::AtomicBool};

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("piep-edit-import-{}", LibraryId::now_v7()));
            fs::create_dir_all(root.join("old/editor-assets")).unwrap();
            let old = root.join("old");
            let conn = rusqlite::Connection::open(old.join("piep.db")).unwrap();
            conn.execute_batch(r#"CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER);
CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT);
CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER);
CREATE TABLE work_edit_revisions(id INTEGER,download_id INTEGER,base_version INTEGER,status TEXT,title TEXT,content_hash TEXT,created_at TEXT,updated_at TEXT);
CREATE TABLE work_edit_blocks(id INTEGER,edit_revision_id INTEGER,block_order INTEGER,block_type TEXT,text TEXT,asset_id INTEGER,attrs_json TEXT);
INSERT INTO downloads VALUES(1,'pixiv','123','Current title','author',0);
INSERT INTO work_edit_revisions VALUES(7,1,1,'active','Edited title','old hash','2026-01-01 00:00:00','2026-01-02 00:00:00');
INSERT INTO work_edit_blocks VALUES(11,7,8,'paragraph','　字下げと空白  ',NULL,'{"gapBefore":2}');
INSERT INTO work_edit_blocks VALUES(12,7,9,'image',NULL,99,NULL);"#).unwrap();
            for number in [1, 2] {
                let path = old.join(format!("downloads/pixiv/123/v{number}/original.json"));
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, json!({"detail":{"title":format!("Title {number}")},"text":format!("Body {number}")}).to_string()).unwrap();
                conn.execute(
                    "INSERT INTO download_versions VALUES(?1,1,?1,?2,?2,'source')",
                    params![number, path.to_str().unwrap()],
                )
                .unwrap();
            }
            let image = old.join("editor-assets/editor.png");
            fs::write(&image, b"exact editor image including metadata\0").unwrap();
            conn.execute("INSERT INTO assets VALUES(99,1,'illustration','editor.png',?1,NULL,'image/png',?2)",params![image.to_str().unwrap(),fs::metadata(&image).unwrap().len() as i64]).unwrap();
            Self(root)
        }
        fn change(&self, sql: &str) {
            rusqlite::Connection::open(self.0.join("old/piep.db"))
                .unwrap()
                .execute_batch(sql)
                .unwrap();
        }
        fn import(&self, destination: &Path) -> (Library, crate::library::LegacyImportReport) {
            let library = Library::create(destination).unwrap();
            let source = LegacyImporter::open(&self.0.join("old")).unwrap();
            let report = source
                .import(&library, &AtomicBool::new(false), |_| {})
                .unwrap();
            (library, report)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn imported_edits_keep_the_old_base_added_images_and_exact_blocks_after_backup() {
        let fixture = Fixture::new();
        let (library, report) = fixture.import(&fixture.0.join("new"));
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        let work = library.works(10).unwrap().remove(0);
        let snapshots = library.snapshots(work.id).unwrap();
        assert_eq!(snapshots.len(), 2);
        {
            let conn = library.conn.lock().unwrap();
            let (base,active): (Vec<u8>,Vec<u8>) = conn.query_row("SELECT r.base_snapshot_id,d.active_id FROM edit_revisions r JOIN edit_documents d ON d.active_id=r.id",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
            assert_eq!(base, snapshots[0].id.as_bytes());
            assert_ne!(base, work.snapshot_id.as_bytes());
            assert_eq!(
                conn.query_row("SELECT text FROM edit_blocks WHERE ordinal=8", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
                "　字下げと空白  "
            );
            assert!(conn
                .execute("UPDATE edit_blocks SET text='changed'", [])
                .is_err());
            assert!(conn.execute("DELETE FROM edit_media", []).is_err());
            assert!(conn
                .execute(
                    "INSERT INTO edit_blocks VALUES(?1,?2,10,'paragraph','late','null',NULL)",
                    params![active, LibraryId::now_v7().as_bytes().as_slice()]
                )
                .is_err());
        }
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        assert!(source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap()
            .unresolved
            .is_empty());
        library.backup(&fixture.0.join("backup")).unwrap();
        drop(source);
        drop(library);
        fs::rename(fixture.0.join("old"), fixture.0.join("unavailable-old")).unwrap();
        let backup = Library::open(&fixture.0.join("backup")).unwrap();
        let edit = backup
            .edit_state(work.id, backup.id)
            .unwrap()
            .active
            .unwrap();
        let info = backup.edit(work.id, edit).unwrap();
        assert_eq!(info.base, snapshots[0].id);
        assert_eq!(info.content.title.as_deref(), Some("Edited title"));
        let file = backup
            .edit_media_file(work.id, edit, "legacy-asset:99")
            .unwrap();
        let mut bytes = Vec::new();
        backup.objects.export(&file, &mut bytes).unwrap();
        assert_eq!(bytes, b"exact editor image including metadata\0");
        backup.verify().unwrap();
    }

    #[test]
    fn invalid_edits_leave_no_partial_edit_records() {
        for change in [
            "UPDATE work_edit_revisions SET base_version=77",
            "UPDATE assets SET download_id=2",
            "INSERT INTO work_edit_revisions SELECT 8,download_id,base_version,status,title,content_hash,created_at,updated_at FROM work_edit_revisions",
            "INSERT INTO work_edit_blocks VALUES(13,999,0,'paragraph','orphan',NULL,NULL)",
            "UPDATE work_edit_blocks SET block_order=-1 WHERE id=11",
        ] {
            let fixture = Fixture::new();
            fixture.change(change);
            let (library,report) = fixture.import(&fixture.0.join("new"));
            assert!(!report.unresolved.is_empty(),"{change}");
            assert!(!library.activation_problems().unwrap().is_empty());
            let conn = library.conn.lock().unwrap();
            for table in ["edit_revisions","edit_documents","edit_media","edit_blocks"] {
                assert_eq!(conn.query_row(&format!("SELECT count(*) FROM {table}"),[],|r|r.get::<_,i64>(0)).unwrap(),0,"{change}: {table}");
            }
            drop(conn);
            library.verify().unwrap();
        }
    }

    #[test]
    fn catching_up_keeps_unchanged_edits_and_adds_rewritten_ones_as_children() {
        let fixture = Fixture::new();
        let destination = fixture.0.join("new");
        let (library, report) = fixture.import(&destination);
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        let work = library.works(10).unwrap().remove(0).id;
        let first = library.edit_state(work, library.id).unwrap();
        let again = || {
            let report = LegacyImporter::open(&fixture.0.join("old"))
                .unwrap()
                .import(&library, &AtomicBool::new(false), |_| {})
                .unwrap();
            assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        };
        let revisions = || -> i64 {
            library
                .conn
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM edit_revisions", [], |r| r.get(0))
                .unwrap()
        };
        // 旧棚で編集を書き換えると、前の編集を親にした新しい編集が有効になる。
        fixture.change("UPDATE work_edit_blocks SET text='直した段落' WHERE id=11; UPDATE work_edit_revisions SET updated_at='2026-03-01 00:00:00';");
        again();
        let second = library.edit_state(work, library.id).unwrap();
        assert_ne!(second.active, first.active);
        assert_eq!(second.generation, first.generation + 1);
        assert_eq!(revisions(), 2);
        let parent: Option<Vec<u8>> = library
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT parent_id FROM edit_revisions WHERE id=?1",
                [second.active.unwrap().as_bytes().as_slice()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(parent, first.active.map(|id| id.as_bytes().to_vec()));
        // 旧棚が変わらなければ、作り直さない。
        fixture.change("UPDATE work_edit_revisions SET status=status;");
        again();
        assert_eq!(library.edit_state(work, library.id).unwrap(), second);
        assert_eq!(revisions(), 2);
        // 旧棚で外した編集は、新しい棚でも有効でなくなる。
        fixture.change("UPDATE work_edit_revisions SET status='archived';");
        again();
        let third = library.edit_state(work, library.id).unwrap();
        assert!(third.active.is_none());
        // 外した編集は以前の編集版の一覧に載る。
        let listed: i64 = library
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM edit_archive", [], |r| r.get(0))
            .unwrap();
        assert_eq!(listed, 1);
        assert_eq!(third.generation, second.generation + 1);
        library.verify().unwrap();
    }

    #[test]
    fn optional_title_and_draft_and_archived_states_are_preserved() {
        let fixture = Fixture::new();
        fixture.change("UPDATE work_edit_revisions SET title=NULL; INSERT INTO work_edit_revisions VALUES(8,1,1,'draft',NULL,NULL,'2026-01-01 00:00:00','2026-01-02 00:00:00'); INSERT INTO work_edit_revisions VALUES(9,1,1,'archived','過去の編集',NULL,'2026-01-01 00:00:00','2026-01-02 00:00:00');");
        let (library, report) = fixture.import(&fixture.0.join("new"));
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        let work = library.works(1).unwrap().remove(0);
        let state = library.edit_state(work.id, library.id).unwrap();
        assert!(state.active.is_some() && state.draft.is_some());
        // 有効と下書きは一度に指すので、世代は一つ進む。
        assert_eq!(state.generation, 1);
        let active = library.edit(work.id, state.active.unwrap()).unwrap();
        assert_eq!(active.content.title, None);
        let conn = library.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM edit_revisions", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            3
        );
        // 旧棚の「以前の編集版」は、新しい棚の一覧にも載る。
        assert_eq!(
            conn.query_row("SELECT count(*) FROM edit_archive", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
