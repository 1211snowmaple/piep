use super::{
    rows::{mark_row, mark_table, marked},
    LegacyImporter,
};
use crate::library::{Error, Library, LibraryId, Result};
use rusqlite::{params, OptionalExtension};

impl LegacyImporter {
    pub(super) fn normalize_tags(&self, target: &Library) -> Result<()> {
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        if marked(&tx, "tags")? && marked(&tx, "download_tags")? {
            return Ok(());
        }
        self.each_row("download_tags",|row| {
            let old_work=row.integer("download_id")?;
            let old_tag=row.integer("tag_id")?;
            let name:String=self.conn.query_row("SELECT name FROM tags WHERE id=?1",[old_tag],|r|r.get(0))?;
            let work=self.work_from_old(&tx,old_work)?;
            let id=match row.text("tag_source")? {
                "origin"=>{
                    // 現在のタグを過去版に埋めない。原本から復元した現在状態と照合する。
                    let tag:Option<Vec<u8>>=tx.query_row("SELECT t.id FROM entity_current i JOIN snapshot_tags s ON s.snapshot_id=i.snapshot_id JOIN tags t ON t.id=s.tag_id WHERE i.entity_id=?1 AND t.name=?2",params![work.as_bytes().as_slice(),name],|r|r.get(0)).optional()?;
                    tag.ok_or_else(||Error::Integrity(format!("作品 {old_work} の取得元タグ {old_tag} を原本の現在状態と照合できません")))?
                },
                provenance @ ("manual"|"llm")=>{
                    tx.execute("INSERT INTO user_tags VALUES(?1,?2,?3) ON CONFLICT(user_id,name) DO NOTHING",params![LibraryId::now_v7().as_bytes().as_slice(),target.id.as_bytes().as_slice(),name])?;
                    let tag:Vec<u8>=tx.query_row("SELECT id FROM user_tags WHERE user_id=?1 AND name=?2",params![target.id.as_bytes().as_slice(),name],|r|r.get(0))?;
                    tx.execute("INSERT INTO work_user_tag_state VALUES(?1,?2,1) ON CONFLICT(work_id,user_id) DO NOTHING",params![work.as_bytes().as_slice(),target.id.as_bytes().as_slice()])?;
                    tx.execute("INSERT INTO work_user_tags VALUES(?1,?2,?3,?4)",params![work.as_bytes().as_slice(),target.id.as_bytes().as_slice(),tag,if provenance=="manual" {"manual"}else{"accepted_model"}])?;
                    tag
                },
                _=>return Err(Error::Integrity(format!("作品 {old_work} のタグ {old_tag} の出どころが不明です"))),
            };
            let id=LibraryId::from_slice(&id).map_err(|_|Error::Integrity("タグの識別子が不正です".into()))?;
            mark_row(&tx,"download_tags",&format!("{old_work}:{old_tag}"),id)
        })?;
        self.each_row("tags", |row| {
            row.text("name")?;
            Ok(())
        })?;
        mark_table(&tx, "download_tags")?;
        mark_table(&tx, "tags")?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{TagProvenance, UserTag};
    use std::{fs, sync::atomic::AtomicBool};

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new(change: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("piep-tag-import-{}", LibraryId::now_v7()));
            let old = root.join("old");
            fs::create_dir_all(old.join("downloads/pixiv/one/v1")).unwrap();
            fs::write(
                old.join("downloads/pixiv/one/v1/original.json"),
                br#"{"detail":{"title":"title","tags":[{"name":"origin"}]},"text":"body"}"#,
            )
            .unwrap();
            let conn = rusqlite::Connection::open(old.join("piep.db")).unwrap();
            conn.execute_batch("CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER);
CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT);
CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER);
CREATE TABLE tags(id INTEGER,name TEXT);
CREATE TABLE download_tags(download_id INTEGER,tag_id INTEGER,tag_source TEXT);
INSERT INTO downloads VALUES(1,'pixiv','one','title','author',0);
INSERT INTO download_versions VALUES(1,1,1,'downloads/pixiv/one/v1/original.json',NULL,'source');
INSERT INTO tags VALUES(1,'origin'),(2,'manual tag'),(3,'chosen model tag'),(4,'unused dictionary entry');
INSERT INTO download_tags VALUES(1,1,'origin'),(1,2,'manual'),(1,3,'llm');").unwrap();
            conn.execute_batch(change).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn migration_keeps_manual_and_chosen_model_tags_separate_from_the_source() {
        let fixture = Fixture::new("");
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        let work = library.works(1).unwrap().remove(0);
        assert_eq!(
            library.source_tags(work.id, work.snapshot_id).unwrap(),
            vec!["origin"]
        );
        let tags = library.user_tags(work.id, library.id).unwrap();
        assert_eq!(tags.generation, 1);
        assert_eq!(
            tags.tags,
            vec![
                UserTag {
                    name: "chosen model tag".into(),
                    provenance: TagProvenance::AcceptedModel
                },
                UserTag {
                    name: "manual tag".into(),
                    provenance: TagProvenance::Manual
                }
            ]
        );
        assert!(source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap()
            .unresolved
            .is_empty());
        assert_eq!(library.user_tags(work.id, library.id).unwrap(), tags);
        assert_eq!(library.snapshots(work.id).unwrap().len(), 1);
        library.verify().unwrap();
    }

    #[test]
    fn unclear_provenance_or_unmatched_source_tags_do_not_partially_migrate() {
        for change in [
            "UPDATE download_tags SET tag_source='unknown' WHERE tag_id=3",
            "UPDATE tags SET name='not in source' WHERE id=1",
        ] {
            let fixture = Fixture::new(change);
            let library = Library::create(&fixture.0.join("new")).unwrap();
            let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
            let report = source
                .import(&library, &AtomicBool::new(false), |_| {})
                .unwrap();
            assert!(!report.unresolved.is_empty());
            assert!(!library.activation_problems().unwrap().is_empty());
            let work = library.works(1).unwrap().remove(0);
            assert!(library
                .user_tags(work.id, library.id)
                .unwrap()
                .tags
                .is_empty());
            assert_eq!(
                library.source_tags(work.id, work.snapshot_id).unwrap(),
                vec!["origin"]
            );
            library.verify().unwrap();
        }
    }
}
