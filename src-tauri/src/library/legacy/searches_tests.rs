use super::*;
use crate::library::{LibraryId, SavedSearchUpdate};
use rusqlite::Connection;
use std::path::PathBuf;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("piep-search-migration-{}", LibraryId::now_v7()));
        std::fs::create_dir_all(root.join("old")).unwrap();
        let db = Connection::open(root.join("old/piep.db")).unwrap();
        db.execute_batch("CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER);
CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT);
CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER);
CREATE TABLE saved_searches(id INTEGER PRIMARY KEY,name TEXT,query TEXT,params_json TEXT,created_at TEXT,updated_at TEXT);").unwrap();
        for (id, created, query) in [
            (8, "2025-01-02 00:00:00", Some("")),
            (3, "2025-01-02 00:00:00", None),
            (9, "2025-01-01 00:00:00", Some("本文")),
        ] {
            db.execute(
                "INSERT INTO saved_searches VALUES(?1,?2,?3,?4,?5,'2025-02-01T00:00:00+09:00')",
                params![id, format!("検索 {id}"), query, conditions(), created],
            )
            .unwrap();
        }
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn conditions() -> &'static str {
    "{ \"tab\":\"works\",\"filters\":{\"tagsInclude\":[\"好きなタグ\"]},\"future\":{\"value\":0.07221960411257732,\"large\":9007199254740993} }\r\n"
}

#[test]
fn migration_keeps_order_exact_conditions_null_query_dates_ids_and_independent_backup() {
    let fixture = Fixture::new();
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    let report = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
    let searches = library.saved_searches(library.id).unwrap();
    assert_eq!(
        searches
            .iter()
            .map(|s| s.definition.name.as_str())
            .collect::<Vec<_>>(),
        vec!["検索 9", "検索 3", "検索 8"]
    );
    assert_eq!(searches[0].definition.query, Some("本文".into()));
    assert_eq!(searches[1].definition.query, None);
    assert_eq!(searches[2].definition.query, Some(String::new()));
    for (i, s) in searches.iter().enumerate() {
        assert_eq!(s.definition.params_json, conditions());
        assert_eq!(s.updated_at, 1738335600000);
        assert_eq!(s.generation, 1);
        assert_eq!(s.position, i as i64);
    }
    assert_eq!(searches[0].created_at, 1735689600000);
    assert!(source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap()
        .unresolved
        .is_empty());
    assert_eq!(library.saved_searches(library.id).unwrap(), searches);
    library.backup(&fixture.0.join("backup")).unwrap();
    drop(source);
    drop(library);
    // 元の DB とファイルがなくても、新しい棚だけで利用者の条件を読み出せる。
    let old = fixture.0.join("old").canonicalize().unwrap();
    assert!(old.starts_with(fixture.0.canonicalize().unwrap()));
    std::fs::rename(&old, fixture.0.join("old-unavailable")).unwrap();
    let backup = Library::open(&fixture.0.join("backup")).unwrap();
    assert_eq!(backup.saved_searches(backup.id).unwrap(), searches);
    backup.verify().unwrap();
}

#[test]
fn invalid_conditions_or_dates_roll_back_the_whole_domain_and_block_activation() {
    for (column, value) in [("params_json", "[]"), ("updated_at", "not a date")] {
        let fixture = Fixture::new();
        {
            let old = Connection::open(fixture.0.join("old/piep.db")).unwrap();
            old.execute(
                &format!("UPDATE saved_searches SET {column}=?1 WHERE id=8"),
                [value],
            )
            .unwrap();
        }
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(!report.unresolved.is_empty());
        assert!(library.saved_searches(library.id).unwrap().is_empty());
        assert!(library
            .activation_problems()
            .unwrap()
            .iter()
            .any(|p| p == "__normalization__: saved_searches"));
        let conn = library.conn.lock().unwrap();
        let mapped:i64=conn.query_row("SELECT count(*) FROM migration_records WHERE source_table='saved_searches' AND target_id IS NOT NULL",[],|r|r.get(0)).unwrap();
        assert_eq!(mapped, 0);
        drop(conn);
        library.verify().unwrap();
    }
}

#[test]
fn migration_cannot_be_reapplied_after_a_user_changes_a_saved_search() {
    let fixture = Fixture::new();
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    assert!(source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap()
        .unresolved
        .is_empty());
    let mut search = library.saved_searches(library.id).unwrap().remove(0);
    search.definition.query = Some("変更した条件".into());
    let changed = library
        .update_saved_search(SavedSearchUpdate {
            request_id: LibraryId::now_v7(),
            id: search.id,
            user: library.id,
            expected_generation: 1,
            definition: Some(search.definition),
        })
        .unwrap();
    assert!(source
        .import(&library, &AtomicBool::new(false), |_| {})
        .is_err());
    assert_eq!(
        library.saved_search(library.id, changed.id).unwrap(),
        changed
    );
    assert!(library.activation_problems().unwrap().is_empty());
    library.verify().unwrap();
}

#[test]
fn a_cancelled_normalization_does_not_mark_the_table_or_publish_any_search() {
    let fixture = Fixture::new();
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    assert!(matches!(
        source.normalize_searches(&library, &AtomicBool::new(true)),
        Err(Error::Canceled)
    ));
    assert!(library.saved_searches(library.id).unwrap().is_empty());
    assert!(!marked(&library.conn.lock().unwrap(), "saved_searches").unwrap());
}
