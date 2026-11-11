use super::{
    rows::{mapped_id, mark_row, mark_table, marked},
    LegacyImporter,
};
use crate::library::{EntityKind, Error, Library, Result};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::json;
use std::collections::BTreeMap;

impl LegacyImporter {
    pub(super) fn work_from_old(
        &self,
        tx: &Transaction<'_>,
        old: i64,
    ) -> Result<crate::library::LibraryId> {
        let (provider, key): (String, String) = self.conn.query_row(
            "SELECT source,source_id FROM downloads WHERE id=?1",
            [old],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Library::relation_entity(
            tx,
            &provider,
            EntityKind::Work,
            &key,
            chrono::Utc::now().timestamp_millis(),
        )
    }

    pub(super) fn normalize_catalogue(&self, target: &Library) -> Result<()> {
        // ファイルの読み込み・復元検査を、書き込みの錠を取る前に行う。
        let mut covers = BTreeMap::new();
        let mut candidate_files = BTreeMap::new();
        self.each_row("work_collections", |row| {
            if let Some(path) = row
                .optional_text("cover_image_path")?
                .filter(|s| !s.is_empty())
            {
                let path = self.managed_path(path)?;
                let prepared = target.store_path(&path)?;
                covers.insert(
                    row.text("id")?.to_owned(),
                    (prepared.file.id, prepared, path),
                );
            }
            Ok(())
        })?;
        self.each_row("update_candidates", |row| {
            let payload = row.text("payload_json")?;
            let _: serde_json::Value = serde_json::from_str(payload)?;
            let prepared = target.store_bytes(payload.as_bytes())?;
            candidate_files.insert(
                (
                    row.text("source")?.to_owned(),
                    row.text("source_id")?.to_owned(),
                ),
                (prepared.file.id, prepared),
            );
            Ok(())
        })?;
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;

        if !marked(&tx, "work_collections")? {
            self.each_row("work_collections",|row|{
                let key=row.text("id")?;
                let id=mapped_id(&tx,"work_collections",key)?;
                let created=row.time("created_at")?;
                let ordered=match row.text("collection_kind")? {"ordered"=>true,"unordered"=>false,_=>return Err(Error::Integrity("旧コレクションの種類が不正です".into()))};
                let cover_work=row.optional_integer("cover_download_id")?.map(|old|self.work_from_old(&tx,old)).transpose()?;
                let cover_file=if let Some((id,file,_))=covers.get(key) {Library::register_file(&tx,file)?;Some(id.0.to_vec())} else {None};
                let appearance=json!({"cover_mode":row.optional_text("cover_mode")?,"name_source":row.optional_text("name_source")?,"track":row.optional_text("track")?});
                tx.execute("INSERT INTO entities(id,kind,created_at) VALUES(?1,'collection',?2)",params![id.as_bytes().as_slice(),created])?;
                tx.execute("INSERT INTO collections(id,title,description,appearance_json,cover_work_id,cover_file_id,ordered,generation) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![id.as_bytes().as_slice(),row.text("name")?,row.optional_text("description")?,appearance.to_string(),cover_work.map(|id|id.as_bytes().to_vec()),cover_file,ordered,row.integer("revision")?])?;
                mark_row(&tx,"work_collections",key,id)
            })?;
            mark_table(&tx, "work_collections")?;
        }
        if !marked(&tx, "work_collection_members")? {
            self.each_row("work_collection_members",|row|{
                let old=row.text("collection_id")?;
                let collection=mapped_id(&tx,"work_collections",old)?;
                let work=Library::relation_entity(&tx,row.text("source")?,EntityKind::Work,row.text("source_id")?,row.time("created_at")?)?;
                tx.execute("INSERT INTO collection_members(collection_id,work_id,position,title_at_add,author_at_add,role,added_by,pinned,note,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",params![collection.as_bytes().as_slice(),work.as_bytes().as_slice(),row.integer("position")?,row.text("title_snapshot")?,row.text("author_snapshot")?,row.text("member_role")?,row.text("added_by")?,row.boolean("pinned")?,row.optional_text("note")?,row.time("created_at")?,row.time("updated_at")?])?;
                Ok(())
            })?;
            mark_table(&tx, "work_collection_members")?;
        }
        if !marked(&tx, "update_targets")? {
            self.each_row("update_targets",|row|{
                let kind=match row.text("target_type")? {"author"|"creator"=>EntityKind::Person,"series"=>EntityKind::Series,"work"=>EntityKind::Work,_=>return Err(Error::Unsupported("旧監視対象の種類に対応していません".into()))};
                let created=row.time("created_at")?;
                let entity=Library::relation_entity(&tx,row.text("source")?,kind,row.text("source_key")?,created)?;
                let old=row.integer("id")?.to_string();let id=mapped_id(&tx,"update_targets",&old)?;
                let options=row.optional_text("metadata_json")?.unwrap_or("{}");let _:serde_json::Value=serde_json::from_str(options)?;
                tx.execute("INSERT INTO watch_targets(id,entity_id,display_name,enabled,last_checked_at,last_seen_external_key,last_seen_source_updated_at,last_hit_at,consecutive_errors,options_json,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![id.as_bytes().as_slice(),entity.as_bytes().as_slice(),row.text("display_name")?,row.boolean("enabled")?,row.optional_time("last_checked_at")?,row.optional_text("last_seen_source_id")?,row.optional_time("last_seen_source_updated_at")?,row.optional_time("last_hit_at")?,row.optional_integer("consecutive_errors")?.unwrap_or(0),options,created,row.time("updated_at")?])?;
                mark_row(&tx,"update_targets",&old,id)
            })?;
            mark_table(&tx, "update_targets")?;
        }
        if !marked(&tx, "watch_groups")? {
            self.each_row("watch_groups", |row| {
                let old = row.integer("id")?.to_string();
                let id = mapped_id(&tx, "watch_groups", &old)?;
                tx.execute(
                    "INSERT INTO watch_groups VALUES(?1,?2,?3,?4,?5)",
                    params![
                        id.as_bytes().as_slice(),
                        row.text("name")?,
                        row.integer("sort_order")?,
                        row.time("created_at")?,
                        row.time("updated_at")?
                    ],
                )?;
                mark_row(&tx, "watch_groups", &old, id)
            })?;
            mark_table(&tx, "watch_groups")?;
        }
        if !marked(&tx, "watch_group_members")? {
            self.each_row("watch_group_members", |row| {
                let group = mapped_id(&tx, "watch_groups", &row.integer("group_id")?.to_string())?;
                let target = mapped_id(
                    &tx,
                    "update_targets",
                    &row.integer("target_id")?.to_string(),
                )?;
                tx.execute(
                    "INSERT INTO watch_group_members VALUES(?1,?2)",
                    params![group.as_bytes().as_slice(), target.as_bytes().as_slice()],
                )?;
                Ok(())
            })?;
            mark_table(&tx, "watch_group_members")?;
        }
        if !marked(&tx, "update_candidates")? {
            self.each_row("update_candidates", |row| {
                let provider = row.text("source")?;
                let key = row.text("source_id")?;
                let work = Library::relation_entity(
                    &tx,
                    provider,
                    EntityKind::Work,
                    key,
                    row.time("first_seen_at")?,
                )?;
                let (file_id, file) = candidate_files
                    .get(&(provider.to_owned(), key.to_owned()))
                    .ok_or_else(|| Error::Integrity("候補の原本が準備されていません".into()))?;
                Library::register_file(&tx, file)?;
                let old = format!("{provider}:{key}");
                let id = mapped_id(&tx, "update_candidates", &old)?;
                tx.execute(
                    "INSERT INTO discovery_candidates VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    params![
                        id.as_bytes().as_slice(),
                        work.as_bytes().as_slice(),
                        row.text("kind")?,
                        row.text("title")?,
                        row.text("status")?,
                        file_id.0.as_slice(),
                        row.optional_text("target_type")?,
                        row.time("first_seen_at")?,
                        row.time("updated_at")?
                    ],
                )?;
                mark_row(&tx, "update_candidates", &old, id)
            })?;
            mark_table(&tx, "update_candidates")?;
        }
        if !marked(&tx, "work_links")? {
            self.each_row("work_links",|row|{
                let created=row.time("discovered_at")?;
                let from=Library::relation_entity(&tx,row.text("from_source")?,EntityKind::Work,row.text("from_source_id")?,created)?;
                let to=Library::relation_entity(&tx,row.text("to_source")?,EntityKind::Work,row.text("to_source_id")?,created)?;
                let relation=row.text("relation_type")?;
                let existing:Option<Vec<u8>>=tx.query_row("SELECT id FROM work_links WHERE from_work_id=?1 AND to_work_id=?2 AND relation=?3",params![from.as_bytes().as_slice(),to.as_bytes().as_slice(),relation],|r|r.get(0)).optional()?;
                let id=existing.map(|id|crate::library::LibraryId::from_slice(&id).map_err(|_|Error::Integrity("関係の ID が不正です".into()))).transpose()?.unwrap_or_else(crate::library::LibraryId::now_v7);
                tx.execute("INSERT INTO work_links VALUES(?1,?2,?3,?4,'observed',?5,?6) ON CONFLICT(id) DO NOTHING",params![id.as_bytes().as_slice(),from.as_bytes().as_slice(),to.as_bytes().as_slice(),relation,created,row.time("updated_at")?])?;
                let old=row.integer("id")?.to_string();let evidence=mapped_id(&tx,"work_link_evidence",&old)?;
                let confidence=row.0.get("confidence").and_then(serde_json::Value::as_f64).ok_or_else(||Error::Integrity("関係の確度が不正です".into()))?;
                tx.execute("INSERT INTO link_evidence VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![evidence.as_bytes().as_slice(),id.as_bytes().as_slice(),row.text("evidence_type")?,row.text("status")?,row.optional_text("anchor_text")?,row.optional_text("context_text")?,confidence,created])?;
                tx.execute("UPDATE work_links SET status=CASE WHEN EXISTS(SELECT 1 FROM link_evidence WHERE link_id=?1 AND decision='accepted') AND NOT EXISTS(SELECT 1 FROM link_evidence WHERE link_id=?1 AND decision='rejected') THEN 'accepted' WHEN EXISTS(SELECT 1 FROM link_evidence WHERE link_id=?1 AND decision='rejected') AND NOT EXISTS(SELECT 1 FROM link_evidence WHERE link_id=?1 AND decision='accepted') THEN 'rejected' ELSE 'observed' END WHERE id=?1",[id.as_bytes().as_slice()])?;
                mark_row(&tx,"work_links",&old,id)?;mark_row(&tx,"work_link_evidence",&old,evidence)
            })?;
            mark_table(&tx, "work_links")?;
        }
        if !marked(&tx, "collection_pair_feedback")? {
            self.each_row("collection_pair_feedback", |row| {
                let time = row.time("updated_at")?;
                let mut left = Library::relation_entity(
                    &tx,
                    row.text("left_source")?,
                    EntityKind::Work,
                    row.text("left_source_id")?,
                    time,
                )?;
                let mut right = Library::relation_entity(
                    &tx,
                    row.text("right_source")?,
                    EntityKind::Work,
                    row.text("right_source_id")?,
                    time,
                )?;
                if left > right {
                    std::mem::swap(&mut left, &mut right);
                }
                tx.execute(
                    "INSERT INTO collection_pair_feedback VALUES(?1,?2,?3,?4,?5)",
                    params![
                        left.as_bytes().as_slice(),
                        right.as_bytes().as_slice(),
                        row.text("decision")?,
                        row.text("rule_version")?,
                        time
                    ],
                )?;
                Ok(())
            })?;
            mark_table(&tx, "collection_pair_feedback")?;
        }
        for (key, (file, _, path)) in covers {
            let stored:Vec<u8>=tx.query_row("SELECT c.cover_file_id FROM collections c JOIN migration_records r ON r.target_id=c.id WHERE r.source_table='work_collections' AND r.source_key=?1 AND r.status='imported'",[key],|r|r.get(0))?;
            if stored != file.0 {
                return Err(Error::Integrity(
                    "移行先のコレクション表紙が元ファイルと一致しません".into(),
                ));
            }
            self.mark_used_file(&tx, &path, &stored)?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::LibraryId;
    use std::fs;
    use std::sync::atomic::AtomicBool;

    fn source(root: &std::path::Path, invalid_member: bool) {
        fs::create_dir_all(root).unwrap();
        let conn = rusqlite::Connection::open(root.join("piep.db")).unwrap();
        conn.execute_batch("CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER); CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT); CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER);
CREATE TABLE work_collections(id TEXT,name TEXT,description TEXT,collection_kind TEXT,cover_download_id INTEGER,revision INTEGER,created_at TEXT,updated_at TEXT,cover_mode TEXT,cover_image_path TEXT,name_source TEXT,track TEXT);
CREATE TABLE work_collection_members(collection_id TEXT,source TEXT,source_id TEXT,title_snapshot TEXT,author_snapshot TEXT,position INTEGER,member_role TEXT,added_by TEXT,pinned INTEGER,note TEXT,created_at TEXT,updated_at TEXT);
CREATE TABLE update_targets(id INTEGER,target_type TEXT,source TEXT,source_key TEXT,display_name TEXT,enabled INTEGER,last_checked_at TEXT,last_seen_source_id TEXT,last_seen_source_updated_at TEXT,last_hit_at TEXT,consecutive_errors INTEGER,metadata_json TEXT,created_at TEXT,updated_at TEXT);
CREATE TABLE watch_groups(id INTEGER,name TEXT,sort_order INTEGER,created_at TEXT,updated_at TEXT);
CREATE TABLE watch_group_members(group_id INTEGER,target_id INTEGER);
CREATE TABLE update_candidates(source TEXT,source_id TEXT,kind TEXT,title TEXT,status TEXT,payload_json TEXT,target_type TEXT,first_seen_at TEXT,updated_at TEXT);
CREATE TABLE work_links(id INTEGER,from_source TEXT,from_source_id TEXT,to_source TEXT,to_source_id TEXT,relation_type TEXT,evidence_type TEXT,anchor_text TEXT,context_text TEXT,confidence REAL,status TEXT,discovered_at TEXT,updated_at TEXT);
CREATE TABLE collection_pair_feedback(left_source TEXT,left_source_id TEXT,right_source TEXT,right_source_id TEXT,decision TEXT,rule_version TEXT,updated_at TEXT);
INSERT INTO work_collections VALUES('c1','利用者の並び','説明','ordered',NULL,4,'2026-01-01 00:00:00','2026-01-02 00:00:00','mosaic',NULL,'manual','manual');
INSERT INTO work_collection_members VALUES('c1','pixiv','one','一つ目','作者',8,'main','manual',1,'残すメモ','2026-01-01 00:00:00','2026-01-02 00:00:00');
INSERT INTO work_collection_members VALUES('c1','pixiv','two','二つ目','作者',2,'main','suggestion',0,NULL,'2026-01-01 00:00:00','2026-01-02 00:00:00');
INSERT INTO update_targets VALUES(42,'author','pixiv','author1','監視中の作者',1,'2026-01-01T00:00:00+09:00','123',NULL,NULL,2,'{}','2026-01-01 00:00:00','2026-01-02 00:00:00');
INSERT INTO watch_groups VALUES(7,'監視の束',3,'2026-01-01 00:00:00','2026-01-02 00:00:00');
INSERT INTO watch_group_members VALUES(7,42);
INSERT INTO update_candidates VALUES('pixiv','two','new','保留した作品','held','{\"unread\":true}','author','2026-01-01 00:00:00','2026-01-02 00:00:00');
INSERT INTO work_links VALUES(1,'pixiv','one','pixiv','two','continues_to','manual','表示','根拠',1.0,'accepted','2026-01-01 00:00:00','2026-01-02 00:00:00');
INSERT INTO work_links VALUES(2,'pixiv','one','pixiv','two','continues_to','content_link',NULL,'別の根拠',0.7,'rejected','2026-01-01 00:00:00','2026-01-02 00:00:00');
INSERT INTO collection_pair_feedback VALUES('pixiv','one','pixiv','two','reject','rule1','2026-01-02 00:00:00');").unwrap();
        if invalid_member {
            conn.execute(
                "UPDATE work_collection_members SET collection_id='missing' WHERE source_id='two'",
                [],
            )
            .unwrap();
        }
    }

    #[test]
    fn catalogue_migration_preserves_unsaved_members_order_decisions_and_watches() {
        let root = std::env::temp_dir().join(format!("piep-catalogue-{}", LibraryId::now_v7()));
        let old = root.join("old");
        source(&old, false);
        {
            let importer = LegacyImporter::open(&old).unwrap();
            let library = Library::create(&root.join("new")).unwrap();
            let report = importer
                .import(&library, &AtomicBool::new(false), |_| {})
                .unwrap();
            assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
            assert_eq!(report.works, 0);
            assert!(library.works(10).unwrap().is_empty());
            let collections = library.collections().unwrap();
            assert_eq!(collections.len(), 1);
            assert_eq!(collections[0].generation, 4);
            assert_eq!(collections[0].members[0].title_at_add, "二つ目");
            assert_eq!(collections[0].members[1].note.as_deref(), Some("残すメモ"));
            assert!(collections[0].members[1].pinned);
            {
                let conn = library.conn.lock().unwrap();
                for table in [
                    "watch_targets",
                    "watch_groups",
                    "watch_group_members",
                    "discovery_candidates",
                    "work_links",
                    "collection_pair_feedback",
                ] {
                    assert_eq!(
                        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                            .get::<_, i64>(0))
                            .unwrap(),
                        1
                    );
                }
                assert_eq!(
                    conn.query_row("SELECT decision FROM discovery_candidates", [], |r| r
                        .get::<_, String>(0))
                        .unwrap(),
                    "held"
                );
                assert_eq!(
                    conn.query_row("SELECT status FROM work_links", [], |r| r
                        .get::<_, String>(0))
                        .unwrap(),
                    "observed"
                );
                assert_eq!(
                    conn.query_row("SELECT count(*) FROM link_evidence", [], |r| r
                        .get::<_, i64>(0))
                        .unwrap(),
                    2
                );
            }
            importer
                .import(&library, &AtomicBool::new(false), |_| {})
                .unwrap();
            assert_eq!(library.collections().unwrap()[0].generation, 4);
            library.verify().unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_membership_does_not_publish_half_of_the_catalogue() {
        let root = std::env::temp_dir().join(format!("piep-catalogue-bad-{}", LibraryId::now_v7()));
        let old = root.join("old");
        source(&old, true);
        {
            let importer = LegacyImporter::open(&old).unwrap();
            let library = Library::create(&root.join("new")).unwrap();
            let report = importer
                .import(&library, &AtomicBool::new(false), |_| {})
                .unwrap();
            assert!(!report.unresolved.is_empty());
            assert!(!library.activation_problems().unwrap().is_empty());
            assert!(library.collections().unwrap().is_empty());
            library.verify().unwrap();
        }
        fs::remove_dir_all(root).unwrap();
    }
}
