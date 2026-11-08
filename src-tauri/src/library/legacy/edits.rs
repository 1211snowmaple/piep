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
    id: LibraryId,
    content: PreparedFile,
    document: EditContent,
    blocks: Vec<(i64, EditBlock)>,
    media: BTreeMap<i64, PreparedMedia>,
}

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
        // 元画像の走査や圧縮の間は、新 DB の書き込みトランザクションを持たない。
        let mut edits = Vec::new();
        let mut block_count = 0;
        self.each_row("work_edit_revisions", |row| {
            let old = row.integer("id")?;
            let work = row.integer("download_id")?;
            if !matches!(row.text("status")?, "active" | "draft" | "archived") {
                return Err(Error::Integrity(format!("編集 {old} の状態が不明です")));
            }
            let id = LibraryId::now_v7();
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
                        let file = target.objects.put_path(&path)?;
                        entry.insert(PreparedMedia { file: target.prepare_file(&file)?, name, mime });
                    }
                }
                let attrs = block.optional_text("attrs_json")?
                    .map(serde_json::from_str).transpose()?.unwrap_or(Value::Null);
                blocks.push((block.integer("id")?, EditBlock {
                    id: LibraryId::now_v7(), ordinal: block.integer("block_order")?,
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
            let file = target.objects.put(std::io::Cursor::new(document.encode()?))?;
            edits.push(PreparedEdit { row, id, content:target.prepare_file(&file)?, document, blocks, media });
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
            Library::insert_edit(
                &tx,
                EditHeader {
                    id: edit.id,
                    work,
                    base,
                    parent: None,
                    created_at: edit.row.time("created_at")?,
                },
                &edit.document,
                &edit.content,
            )?;
            for (old, media) in &edit.media {
                mark_row(&tx, "edit_asset_placements", &old.to_string(), edit.id)?;
                tx.execute("UPDATE migration_records SET evidence_file_id=?2 WHERE source_table='edit_asset_placements' AND source_key=?1",params![old.to_string(),media.file.file.id.0.as_slice()])?;
            }
            for (old, block) in edit.blocks {
                mark_row(&tx, "work_edit_blocks", &old.to_string(), block.id)?;
            }
            let status = edit.row.text("status")?;
            if status != "archived" {
                tx.execute("INSERT INTO edit_documents(work_id,user_id,generation) VALUES(?1,?2,0) ON CONFLICT(work_id,user_id) DO NOTHING",params![work.as_bytes().as_slice(),target.id.as_bytes().as_slice()])?;
                let column = if status == "active" {
                    "active_id"
                } else {
                    "draft_id"
                };
                let changed=tx.execute(&format!("UPDATE edit_documents SET {column}=?3,generation=generation+1 WHERE work_id=?1 AND user_id=?2 AND {column} IS NULL"),params![work.as_bytes().as_slice(),target.id.as_bytes().as_slice(),edit.id.as_bytes().as_slice()])?;
                if changed != 1 {
                    return Err(Error::Integrity(format!(
                        "作品 {old_work} に複数の {status} 編集があります"
                    )));
                }
            }
            mark_row(&tx, "work_edit_revisions", &old.to_string(), edit.id)?;
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
    fn optional_title_and_draft_and_archived_states_are_preserved() {
        let fixture = Fixture::new();
        fixture.change("UPDATE work_edit_revisions SET title=NULL; INSERT INTO work_edit_revisions VALUES(8,1,1,'draft',NULL,NULL,'2026-01-01 00:00:00','2026-01-02 00:00:00'); INSERT INTO work_edit_revisions VALUES(9,1,1,'archived','過去の編集',NULL,'2026-01-01 00:00:00','2026-01-02 00:00:00');");
        let (library, report) = fixture.import(&fixture.0.join("new"));
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        let work = library.works(1).unwrap().remove(0);
        let state = library.edit_state(work.id, library.id).unwrap();
        assert!(state.active.is_some() && state.draft.is_some());
        assert_eq!(state.generation, 2);
        let active = library.edit(work.id, state.active.unwrap()).unwrap();
        assert_eq!(active.content.title, None);
        let conn = library.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM edit_revisions", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            3
        );
    }
}
