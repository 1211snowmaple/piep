use super::{
    rows::{guarded_row, mark_row, mark_table, marked},
    LegacyImporter,
};
use crate::library::{EntityKind, Error, Library, LibraryId, Result};
use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::{BTreeMap, BTreeSet};

/// 旧表の所属が、現在版・過去の保存状態のどちらと一致したか。
enum Matched {
    Current,
    /// 作者が所属を変えた後も旧表に残っていた行。一致した保存状態を証跡にする。
    Earlier(Vec<u8>),
}

fn matched(
    tx: &Transaction<'_>,
    current: &str,
    earlier: &str,
    values: &[&dyn rusqlite::ToSql],
) -> Result<Option<Matched>> {
    if tx.query_row(current, values, |r| r.get::<_, bool>(0))? {
        return Ok(Some(Matched::Current));
    }
    Ok(tx
        .query_row(earlier, values, |r| r.get::<_, Vec<u8>>(0))
        .optional()?
        .map(Matched::Earlier))
}

/// 過去の保存状態と一致した行は、移行済みにしたうえで、どの保存状態かを残す。
fn mark_relation(
    tx: &Transaction<'_>,
    table: &str,
    key: &str,
    related: LibraryId,
    matched: Matched,
) -> Result<()> {
    mark_row(tx, table, key, related)?;
    if let Matched::Earlier(snapshot) = matched {
        tx.execute("INSERT INTO migration_records(source_table,source_key,target_id,status) VALUES('__earlier_relations__',?1,?2,'imported') ON CONFLICT(source_table,source_key) DO UPDATE SET target_id=excluded.target_id,status=excluded.status",params![format!("{table}:{key}"),snapshot])?;
    }
    Ok(())
}

const CURRENT_PEOPLE: &str = "SELECT p.person_id,e.provider,e.external_key FROM entity_current c JOIN snapshot_people p ON p.snapshot_id=c.snapshot_id JOIN external_identities e ON e.entity_id=p.person_id WHERE c.entity_id=?1";
const CURRENT_SERIES: &str = "SELECT s.series_id,e.provider,e.external_key FROM entity_current c JOIN snapshot_series s ON s.snapshot_id=c.snapshot_id JOIN external_identities e ON e.entity_id=s.series_id WHERE c.entity_id=?1";

impl LegacyImporter {
    /// 過去の保存状態とだけ一致する行を、消し残りとして受け入れてよいか。
    /// 現在版の所属がすべて旧表にも載っているときに限る。旧表に現在の所属がなく
    /// 過去の所属だけがある場合は、旧表と原本の食い違いなので未解決にする。
    fn earlier_relation_is_leftover(
        &self,
        tx: &Transaction<'_>,
        work: LibraryId,
        old: i64,
        current: &str,
        listed: &str,
    ) -> Result<()> {
        let listed: BTreeSet<(String, String)> = self
            .conn
            .prepare(listed)?
            .query_map([old], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        let mut entities = BTreeMap::<Vec<u8>, bool>::new();
        let mut statement = tx.prepare(current)?;
        for row in statement.query_map([work.as_bytes().as_slice()], |r| {
            Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?, r.get(2)?))
        })? {
            let (entity, provider, key) = row?;
            *entities.entry(entity).or_default() |= listed.contains(&(provider, key));
        }
        if entities.values().all(|found| *found) {
            Ok(())
        } else {
            Err(Error::Integrity(format!(
                "作品 {old} の旧表に現在の所属がなく、過去の所属だけが残っています"
            )))
        }
    }

    /// 旧表の所属を一行ずつ原本と照合する。照合できない行だけを未解決として返し、
    /// ほかの行の移行は続ける。
    pub(super) fn normalize_source_relations(&self, target: &Library) -> Result<Vec<String>> {
        let mut anomalies = Vec::new();
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        if !marked(&tx, "download_people")? {
            let before = anomalies.len();
            self.each_row("download_people", |row| {
                let label = row.label(&["download_id", "person_source", "person_key", "role"]);
                guarded_row(target, &tx, "download_people", &label, &mut anomalies, || {
                    let old = row.integer("download_id")?;
                    let work = self.work_from_old(&tx, old)?;
                    let (provider, key, role) = (
                        row.text("person_source")?,
                        row.text("person_key")?,
                        row.text("role")?,
                    );
                    let person =
                        Library::relation_entity(&tx, provider, EntityKind::Person, key, 0)?;
                    let found = matched(
                        &tx,
                        "SELECT EXISTS(SELECT 1 FROM entity_current c JOIN snapshot_people p ON p.snapshot_id=c.snapshot_id WHERE c.entity_id=?1 AND p.person_id=?2 AND p.role=?3 AND p.display_name=?4)",
                        "SELECT v.id FROM revision_snapshots v JOIN snapshot_people p ON p.snapshot_id=v.id WHERE v.entity_id=?1 AND v.published=1 AND p.person_id=?2 AND p.role=?3 AND p.display_name=?4 ORDER BY v.sequence DESC LIMIT 1",
                        params![
                            work.as_bytes().as_slice(),
                            person.as_bytes().as_slice(),
                            role,
                            row.text("display_name")?
                        ],
                    )?
                    .ok_or_else(|| {
                        Error::Integrity(format!(
                            "作品 {old} の人物との関係を保存済みの原本と照合できません"
                        ))
                    })?;
                    if matches!(found, Matched::Earlier(_)) {
                        self.earlier_relation_is_leftover(
                            &tx,
                            work,
                            old,
                            CURRENT_PEOPLE,
                            "SELECT person_source,person_key FROM download_people WHERE download_id=?1",
                        )?;
                    }
                    mark_relation(&tx, "download_people", &label, person, found)
                })
            })?;
            if anomalies.len() == before {
                mark_table(&tx, "download_people")?;
            }
        }
        if !marked(&tx, "download_series")? {
            let before = anomalies.len();
            self.each_row("download_series", |row| {
                let label = row.label(&["download_id", "series_source", "series_key"]);
                guarded_row(target, &tx, "download_series", &label, &mut anomalies, || {
                    let old = row.integer("download_id")?;
                    let work = self.work_from_old(&tx, old)?;
                    let (provider, key) = (row.text("series_source")?, row.text("series_key")?);
                    let series =
                        Library::relation_entity(&tx, provider, EntityKind::Series, key, 0)?;
                    let found = matched(
                        &tx,
                        "SELECT EXISTS(SELECT 1 FROM entity_current c JOIN snapshot_series s ON s.snapshot_id=c.snapshot_id WHERE c.entity_id=?1 AND s.series_id=?2 AND s.title=?3 AND s.position IS ?4)",
                        "SELECT v.id FROM revision_snapshots v JOIN snapshot_series s ON s.snapshot_id=v.id WHERE v.entity_id=?1 AND v.published=1 AND s.series_id=?2 AND s.title=?3 AND s.position IS ?4 ORDER BY v.sequence DESC LIMIT 1",
                        params![
                            work.as_bytes().as_slice(),
                            series.as_bytes().as_slice(),
                            row.text("title")?,
                            row.optional_integer("content_order")?
                        ],
                    )?
                    .ok_or_else(|| {
                        Error::Integrity(format!(
                            "作品 {old} のシリーズとの関係を保存済みの原本と照合できません"
                        ))
                    })?;
                    if matches!(found, Matched::Earlier(_)) {
                        self.earlier_relation_is_leftover(
                            &tx,
                            work,
                            old,
                            CURRENT_SERIES,
                            "SELECT series_source,series_key FROM download_series WHERE download_id=?1",
                        )?;
                    }
                    mark_relation(&tx, "download_series", &label, series, found)
                })
            })?;
            if anomalies.len() == before {
                mark_table(&tx, "download_series")?;
            }
        }
        if !marked(&tx, "download_relations")? {
            let before = anomalies.len();
            self.each_row("download_relations", |row| {
                let label = row.label(&["download_id", "relation_type", "source", "relation_id"]);
                guarded_row(target, &tx, "download_relations", &label, &mut anomalies, || {
                    let old = row.integer("download_id")?;
                    let work = self.work_from_old(&tx, old)?;
                    let (provider, key, relation) = (
                        row.text("source")?,
                        row.text("relation_id")?,
                        row.text("relation_type")?,
                    );
                    let (kind, current, earlier, now, listed) = match relation {
                        "author" => (
                            EntityKind::Person,
                            "SELECT EXISTS(SELECT 1 FROM entity_current c JOIN snapshot_people p ON p.snapshot_id=c.snapshot_id WHERE c.entity_id=?1 AND p.person_id=?2 AND p.display_name=?3)",
                            "SELECT v.id FROM revision_snapshots v JOIN snapshot_people p ON p.snapshot_id=v.id WHERE v.entity_id=?1 AND v.published=1 AND p.person_id=?2 AND p.display_name=?3 ORDER BY v.sequence DESC LIMIT 1",
                            CURRENT_PEOPLE,
                            "SELECT source,relation_id FROM download_relations WHERE download_id=?1 AND relation_type='author'",
                        ),
                        "series" => (
                            EntityKind::Series,
                            "SELECT EXISTS(SELECT 1 FROM entity_current c JOIN snapshot_series s ON s.snapshot_id=c.snapshot_id WHERE c.entity_id=?1 AND s.series_id=?2 AND s.title=?3)",
                            "SELECT v.id FROM revision_snapshots v JOIN snapshot_series s ON s.snapshot_id=v.id WHERE v.entity_id=?1 AND v.published=1 AND s.series_id=?2 AND s.title=?3 ORDER BY v.sequence DESC LIMIT 1",
                            CURRENT_SERIES,
                            "SELECT source,relation_id FROM download_relations WHERE download_id=?1 AND relation_type='series'",
                        ),
                        _ => {
                            return Err(Error::Unsupported(format!(
                                "旧形式の {relation} 関係を解釈できません"
                            )))
                        }
                    };
                    let related = Library::relation_entity(&tx, provider, kind, key, 0)?;
                    let found = matched(
                        &tx,
                        current,
                        earlier,
                        params![
                            work.as_bytes().as_slice(),
                            related.as_bytes().as_slice(),
                            row.text("relation_name")?
                        ],
                    )?
                    .ok_or_else(|| {
                        Error::Integrity(format!(
                            "作品 {old} の旧関係表を保存済みの原本と照合できません"
                        ))
                    })?;
                    if matches!(found, Matched::Earlier(_)) {
                        self.earlier_relation_is_leftover(&tx, work, old, now, listed)?;
                    }
                    mark_relation(&tx, "download_relations", &label, related, found)
                })
            })?;
            if anomalies.len() == before {
                mark_table(&tx, "download_relations")?;
            }
        }
        tx.commit()?;
        Ok(anomalies)
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
    fn rows_left_from_an_earlier_snapshot_resolve_only_beside_the_current_relation() {
        // 作者がシリーズを移した後も、旧表に前のシリーズの行が残っていた実例と同じ形。
        let fixture = Fixture::new(
            "INSERT INTO download_series VALUES(1,'pixiv','8','Old series',NULL);
INSERT INTO download_relations VALUES(1,'series','pixiv','8','Old series');",
        );
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        let conn = library.conn.lock().unwrap();
        let earlier: Vec<String> = conn
            .prepare("SELECT r.source_key FROM migration_records r JOIN revision_snapshots s ON s.id=r.target_id JOIN entity_current c ON c.entity_id=s.entity_id WHERE r.source_table='__earlier_relations__' AND c.snapshot_id<>s.id ORDER BY r.source_key")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(
            earlier,
            [
                "download_relations:1:series:pixiv:8",
                "download_series:1:pixiv:8"
            ]
        );
        // 現在版の所属は書き換えない。
        assert_eq!(conn.query_row("SELECT count(*) FROM entity_current c JOIN snapshot_series s ON s.snapshot_id=c.snapshot_id JOIN external_identities e ON e.entity_id=s.series_id WHERE e.external_key='8'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        drop(conn);
        library.verify().unwrap();
    }

    #[test]
    fn one_unmatched_row_stays_unresolved_without_stopping_the_other_rows() {
        let fixture = Fixture::new(
            "INSERT INTO download_series VALUES(2,'pixiv','404','Unknown series',NULL);",
        );
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(
            report
                .unresolved
                .iter()
                .any(|line| line.starts_with("download_series 2:pixiv:404: ")),
            "{:?}",
            report.unresolved
        );
        let conn = library.conn.lock().unwrap();
        let status = |table: &str, key: &str| -> Option<String> {
            conn.query_row(
                "SELECT status FROM migration_records WHERE source_table=?1 AND source_key=?2",
                [table, key],
                |r| r.get(0),
            )
            .ok()
        };
        assert_eq!(
            status("download_series", "2:pixiv:404").as_deref(),
            Some("unresolved")
        );
        // 理由は移行先に残り、再起動後も読める。
        let reason: Vec<u8> = conn.query_row("SELECT evidence_file_id FROM migration_records WHERE source_table='download_series' AND source_key='2:pixiv:404'",[],|r|r.get(0)).unwrap();
        let file = Library::file_from_conn(
            &conn,
            crate::library::Fingerprint(reason.try_into().unwrap()),
        )
        .unwrap();
        let mut bytes = Vec::new();
        library.objects.export(&file, &mut bytes).unwrap();
        assert!(String::from_utf8(bytes).unwrap().contains("照合できません"));
        // 同じ表のほかの行と、ほかの表は移行する。失敗した行の作りかけも残さない。
        assert_eq!(
            status("download_series", "1:pixiv:9").as_deref(),
            Some("imported")
        );
        assert_eq!(
            status("download_people", "__all_rows__").as_deref(),
            Some("imported")
        );
        assert_eq!(
            status("download_relations", "__all_rows__").as_deref(),
            Some("imported")
        );
        assert_ne!(
            status("download_series", "__all_rows__").as_deref(),
            Some("imported")
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM external_identities WHERE external_key='404'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        drop(conn);
        assert!(!library.activation_problems().unwrap().is_empty());
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
