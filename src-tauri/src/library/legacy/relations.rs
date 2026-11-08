use super::{
    rows::{mark_row, mark_table, marked},
    LegacyImporter,
};
use crate::library::{EntityKind, Error, Library, Result};
use rusqlite::params;

impl LegacyImporter {
    pub(super) fn normalize_source_relations(&self, target: &Library) -> Result<()> {
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        if !marked(&tx, "download_people")? {
            self.each_row("download_people",|row| {
                let old=row.integer("download_id")?;
                let work=self.work_from_old(&tx,old)?;
                let (provider,key,role)=(row.text("person_source")?,row.text("person_key")?,row.text("role")?);
                let person=Library::relation_entity(&tx,provider,EntityKind::Person,key,0)?;
                let matches:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM entity_current c JOIN snapshot_people p ON p.snapshot_id=c.snapshot_id WHERE c.entity_id=?1 AND p.person_id=?2 AND p.role=?3 AND p.display_name=?4)",params![work.as_bytes().as_slice(),person.as_bytes().as_slice(),role,row.text("display_name")?],|r|r.get(0))?;
                if !matches {return Err(Error::Integrity(format!("作品 {old} の人物との関係を現在の原本と照合できません")));}
                mark_row(&tx,"download_people",&format!("{old}:{provider}:{key}:{role}"),person)
            })?;
            mark_table(&tx, "download_people")?;
        }
        if !marked(&tx, "download_series")? {
            self.each_row("download_series",|row| {
                let old=row.integer("download_id")?;
                let work=self.work_from_old(&tx,old)?;
                let (provider,key)=(row.text("series_source")?,row.text("series_key")?);
                let series=Library::relation_entity(&tx,provider,EntityKind::Series,key,0)?;
                let matches:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM entity_current c JOIN snapshot_series s ON s.snapshot_id=c.snapshot_id WHERE c.entity_id=?1 AND s.series_id=?2 AND s.title=?3 AND s.position IS ?4)",params![work.as_bytes().as_slice(),series.as_bytes().as_slice(),row.text("title")?,row.optional_integer("content_order")?],|r|r.get(0))?;
                if !matches {return Err(Error::Integrity(format!("作品 {old} のシリーズとの関係を現在の原本と照合できません")));}
                mark_row(&tx,"download_series",&format!("{old}:{provider}:{key}"),series)
            })?;
            mark_table(&tx, "download_series")?;
        }
        if !marked(&tx, "download_relations")? {
            self.each_row("download_relations",|row| {
                let old=row.integer("download_id")?;
                let work=self.work_from_old(&tx,old)?;
                let (provider,key,relation)=(row.text("source")?,row.text("relation_id")?,row.text("relation_type")?);
                let (kind,query)=match relation {
                    "author"=>(EntityKind::Person,"SELECT EXISTS(SELECT 1 FROM entity_current c JOIN snapshot_people p ON p.snapshot_id=c.snapshot_id WHERE c.entity_id=?1 AND p.person_id=?2 AND p.display_name=?3)"),
                    "series"=>(EntityKind::Series,"SELECT EXISTS(SELECT 1 FROM entity_current c JOIN snapshot_series s ON s.snapshot_id=c.snapshot_id WHERE c.entity_id=?1 AND s.series_id=?2 AND s.title=?3)"),
                    _=>return Err(Error::Unsupported(format!("旧形式の {relation} 関係を解釈できません"))),
                };
                let related=Library::relation_entity(&tx,provider,kind,key,0)?;
                let matches:bool=tx.query_row(query,params![work.as_bytes().as_slice(),related.as_bytes().as_slice(),row.text("relation_name")?],|r|r.get(0))?;
                if !matches {return Err(Error::Integrity(format!("作品 {old} の旧関係表を現在の原本と照合できません")));}
                mark_row(&tx,"download_relations",&format!("{old}:{relation}:{provider}:{key}"),related)
            })?;
            mark_table(&tx, "download_relations")?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::LibraryId;
    use serde_json::json;
    use std::{fs, sync::atomic::AtomicBool};

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new(change: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("piep-source-relations-{}", LibraryId::now_v7()));
            let old = root.join("old");
            fs::create_dir_all(&old).unwrap();
            let conn = rusqlite::Connection::open(old.join("piep.db")).unwrap();
            conn.execute_batch("CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER);
CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT);
CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER);
CREATE TABLE download_people(download_id INTEGER,person_source TEXT,person_key TEXT,role TEXT,display_name TEXT);
CREATE TABLE download_series(download_id INTEGER,series_source TEXT,series_key TEXT,title TEXT,content_order INTEGER);
CREATE TABLE download_relations(download_id INTEGER,relation_type TEXT,source TEXT,relation_id TEXT,relation_name TEXT);
INSERT INTO downloads VALUES(1,'pixiv','novel','title','New author',0),(2,'fanbox','post','post title','Creator',0);
INSERT INTO download_people VALUES(1,'pixiv','2','author','New author'),(2,'fanbox','creator-slug','creator','Creator');
INSERT INTO download_series VALUES(1,'pixiv','9','Current series',NULL);
INSERT INTO download_relations VALUES(1,'author','pixiv','2','New author'),(1,'series','pixiv','9','Current series'),(2,'author','fanbox','creator-slug','Creator');").unwrap();
            for (id, work, version, data) in [
                (
                    1,
                    1,
                    1,
                    json!({"detail":{"title":"title","user":{"id":1,"name":"Old author"},"series_id":"8","series_title":"Old series"},"text":"body"}),
                ),
                (
                    2,
                    1,
                    2,
                    json!({"detail":{"title":"title","user":{"id":2,"name":"New author"},"series_id":"9","series_title":"Current series"},"text":"body"}),
                ),
                (
                    3,
                    2,
                    1,
                    json!({"title":"post title","type":"article","user":{"userId":"123","name":"Creator"},"creatorId":"creator-slug","publishedDatetime":"2026-01-01T00:00:00+09:00","body":{"blocks":[{"type":"paragraph","text":"fanbox body"}]}}),
                ),
            ] {
                let path = format!("downloads/test/{work}/v{version}/original.json");
                fs::create_dir_all(old.join(&path).parent().unwrap()).unwrap();
                fs::write(old.join(&path), data.to_string()).unwrap();
                conn.execute(
                    "INSERT INTO download_versions VALUES(?1,?2,?3,?4,NULL,'source')",
                    params![id, work, version, path],
                )
                .unwrap();
            }
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
    fn current_relation_tables_are_checked_without_rewriting_old_snapshot_relations() {
        let fixture = Fixture::new("");
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        let conn = library.conn.lock().unwrap();
        let old_author:i64=conn.query_row("SELECT count(*) FROM snapshot_people p JOIN external_identities e ON e.entity_id=p.person_id WHERE e.provider='pixiv' AND e.external_key='1'",[],|r|r.get(0)).unwrap();
        assert_eq!(old_author, 1);
        assert_eq!(conn.query_row("SELECT count(*) FROM entity_current c JOIN snapshot_people p ON p.snapshot_id=c.snapshot_id JOIN external_identities e ON e.entity_id=p.person_id WHERE e.provider='fanbox' AND e.external_key='creator-slug' AND p.role='creator'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
        assert_eq!(conn.query_row("SELECT content_type FROM works w JOIN external_identities e ON e.entity_id=w.id WHERE e.provider='fanbox'",[],|r|r.get::<_,String>(0)).unwrap(),"article");
        drop(conn);
        assert!(source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap()
            .unresolved
            .is_empty());
        library.verify().unwrap();
    }

    #[test]
    fn conflicting_current_relationships_are_not_attached_to_old_versions() {
        for change in ["UPDATE download_people SET person_key='1',display_name='Old author' WHERE download_id=1", "UPDATE download_series SET series_key='8',title='Old series'", "UPDATE download_relations SET relation_type='unknown' WHERE download_id=2"] {
            let fixture=Fixture::new(change);let source=LegacyImporter::open(&fixture.0.join("old")).unwrap();let library=Library::create(&fixture.0.join("new")).unwrap();
            let report=source.import(&library,&AtomicBool::new(false),|_|{}).unwrap();assert!(!report.unresolved.is_empty());assert!(!library.activation_problems().unwrap().is_empty());library.verify().unwrap();
        }
    }
}
