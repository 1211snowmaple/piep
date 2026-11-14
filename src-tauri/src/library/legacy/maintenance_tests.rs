use super::*;
use crate::library::{Capture, EntityKind, ResourceScope, SourceBody};
use rusqlite::Connection;
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("piep-maintenance-{}", LibraryId::now_v7()));
        std::fs::create_dir_all(root.join("old")).unwrap();
        let db = Connection::open(root.join("old/piep.db")).unwrap();
        db.execute_batch("CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER,current_version INTEGER);
CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT);
CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER);
CREATE TABLE incomplete_save_repairs(download_id INTEGER PRIMARY KEY,version INTEGER,attempted_at TEXT);
CREATE TABLE saved_asset_audits(download_id INTEGER PRIMARY KEY,json_path TEXT,version INTEGER,rule_version INTEGER);").unwrap();
        for work in 1..=2 {
            db.execute(
                "INSERT INTO downloads VALUES(?1,'pixiv',?2,?2,'author',0,2)",
                params![work, format!("book-{work}")],
            )
            .unwrap();
            for version in 1..=2 {
                let path = format!("downloads/pixiv/{work}/v{version}/original.json");
                let absolute = root.join("old").join(&path);
                std::fs::create_dir_all(absolute.parent().unwrap()).unwrap();
                std::fs::write(
                    &absolute,
                    serde_json::to_vec(
                        &json!({"detail":{"title":format!("book-{work}"),"tags":[]},"text":"body"}),
                    )
                    .unwrap(),
                )
                .unwrap();
                db.execute(
                    "INSERT INTO download_versions VALUES(?1,?2,?3,?4,?4,?5)",
                    params![
                        work * 10 + version,
                        work,
                        version,
                        path,
                        if version == 1 { "source" } else { "repair" }
                    ],
                )
                .unwrap();
                if version == 2 {
                    db.execute(
                        "INSERT INTO saved_asset_audits VALUES(?1,?2,2,3)",
                        params![work, path],
                    )
                    .unwrap();
                }
            }
            db.execute(
                "INSERT INTO incomplete_save_repairs VALUES(?1,1,'2025-01-02T03:04:05.123+09:00')",
                [work],
            )
            .unwrap();
        }
        Self(root)
    }
    fn change(&self, sql: &str) {
        Connection::open(self.0.join("old/piep.db"))
            .unwrap()
            .execute_batch(sql)
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn count(library: &Library, table: &str) -> i64 {
    assert!(matches!(
        table,
        "repair_attempt_markers" | "asset_audit_markers"
    ));
    library
        .conn
        .lock()
        .unwrap()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}
fn load(fixture: &Fixture) -> (LegacyImporter, Library) {
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    (source, library)
}

#[test]
fn maintenance_is_pinned_to_the_recorded_version_and_keeps_dates_evidence_and_ids_across_retry_and_backup(
) {
    let fixture = Fixture::new();
    let (source, library) = load(&fixture);
    let report = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
    let works = library.works(10).unwrap();
    assert_eq!(works.len(), 2);
    let mut histories = Vec::new();
    for work in &works {
        let snapshots = library.snapshots(work.id).unwrap();
        assert_eq!(snapshots.len(), 2);
        for snapshot in &snapshots {
            let history = library.maintenance_history(work.id, snapshot.id).unwrap();
            if snapshot.id == work.snapshot_id {
                assert!(history.repair_attempts.is_empty());
                assert_eq!(history.asset_audits.len(), 1);
                assert_eq!(history.asset_audits[0].legacy_rule, 3);
                let previous = library
                    .prior_incomplete_repair_for_revision(work.id, snapshot.id)
                    .unwrap()
                    .unwrap();
                assert_ne!(previous.snapshot, snapshot.id);
                assert_eq!(previous.attempted_at, 1735754645123);
                assert!(matches!(
                    library.maintenance_history(
                        works.iter().find(|w| w.id != work.id).unwrap().id,
                        snapshot.id
                    ),
                    Err(Error::ResourceUnavailable)
                ));
            } else {
                assert_eq!(history.repair_attempts.len(), 1);
                assert!(history.asset_audits.is_empty());
                assert_eq!(history.repair_attempts[0].attempted_at, 1735754645123);
                let raw = library
                    .file(history.repair_attempts[0].evidence_file)
                    .unwrap();
                let mut bytes = Vec::new();
                library.objects.export(&raw, &mut bytes).unwrap();
                assert!(!bytes.is_empty());
            }
            histories.push(history);
        }
    }
    let generation: i64 = library
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT generation FROM library_meta", [], |r| r.get(0))
        .unwrap();
    assert!(source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap()
        .unresolved
        .is_empty());
    assert_eq!(
        library
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT generation FROM library_meta", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        generation
    );
    for h in &histories {
        assert_eq!(
            library.maintenance_history(h.entity, h.snapshot).unwrap(),
            *h
        );
    }
    assert_eq!(count(&library, "repair_attempt_markers"), 2);
    assert_eq!(count(&library, "asset_audit_markers"), 2);
    library.backup(&fixture.0.join("backup")).unwrap();
    drop(source);
    drop(library);
    std::fs::rename(fixture.0.join("old"), fixture.0.join("unavailable")).unwrap();
    let backup = Library::open(&fixture.0.join("backup")).unwrap();
    for h in &histories {
        assert_eq!(
            backup.maintenance_history(h.entity, h.snapshot).unwrap(),
            *h
        );
    }
    backup.verify().unwrap();
}

#[test]
fn the_original_audit_schema_without_a_rule_column_is_kept_as_legacy_rule_one() {
    let fixture = Fixture::new();
    fixture.change("ALTER TABLE saved_asset_audits DROP COLUMN rule_version");
    let (source, library) = load(&fixture);
    let report = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
    for work in library.works(10).unwrap() {
        let history = library
            .maintenance_history(work.id, work.snapshot_id)
            .unwrap();
        assert_eq!(history.asset_audits[0].legacy_rule, 1);
    }
    library.verify().unwrap();
}

#[test]
fn old_attempts_do_not_block_a_new_author_revision_or_turn_old_audits_into_new_pass_results() {
    let fixture = Fixture::new();
    fixture.change("UPDATE saved_asset_audits SET rule_version=77");
    let (source, library) = load(&fixture);
    assert!(source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap()
        .unresolved
        .is_empty());
    let work = library.works(1).unwrap().remove(0);
    let mut metadata = library
        .reader_document(work.id, ResourceScope::Snapshot(work.snapshot_id))
        .unwrap()
        .metadata;
    metadata.insert("title".into(), json!("new title"));
    let ticket = library
        .begin_acquisition(
            LibraryId::now_v7(),
            "pixiv",
            EntityKind::Work,
            &work.title,
            "offline-test",
        )
        .unwrap()
        .ticket;
    let result = library
        .complete_acquisition_with_reader(
            ticket.id,
            Capture {
                request_id: ticket.id,
                acquired_at: Some(1),
                provider: "pixiv".into(),
                external_key: work.title.clone(),
                kind: EntityKind::Work,
                route: "offline-test".into(),
                raw: b"new response".to_vec().into(),
                metadata,
                field_states: BTreeMap::new(),
                body: Some(SourceBody::PixivText("body".into())),
                media: vec![],
                attachments: vec![],
            },
            vec![],
        )
        .unwrap();
    assert!(result.created_revision);
    assert!(library
        .prior_incomplete_repair_for_revision(work.id, result.snapshot_id)
        .unwrap()
        .is_none());
    let fresh = library
        .maintenance_history(work.id, result.snapshot_id)
        .unwrap();
    assert!(fresh.asset_audits.is_empty());
    assert!(fresh.repair_attempts.is_empty());
    assert_eq!(
        library
            .maintenance_history(work.id, work.snapshot_id)
            .unwrap()
            .asset_audits[0]
            .legacy_rule,
        77
    );
    library.verify().unwrap();
}

#[test]
fn invalid_old_targets_dates_paths_and_rules_keep_raw_records_and_roll_back_both_domains() {
    for sql in [
        "UPDATE incomplete_save_repairs SET version=9 WHERE download_id=2",
        "UPDATE incomplete_save_repairs SET attempted_at='invalid' WHERE download_id=2",
        "UPDATE saved_asset_audits SET version=9 WHERE download_id=2",
        "UPDATE saved_asset_audits SET json_path='different.json' WHERE download_id=2",
        "UPDATE saved_asset_audits SET rule_version=0 WHERE download_id=2",
        "UPDATE saved_asset_audits SET rule_version=NULL WHERE download_id=2",
        "UPDATE downloads SET source='fanbox' WHERE id=2",
    ] {
        let fixture = Fixture::new();
        fixture.change(sql);
        let (source, library) = load(&fixture);
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(
            report
                .unresolved
                .iter()
                .any(|s| s.contains("maintenance") || s.contains("修復試行")),
            "{sql}: {:?}",
            report.unresolved
        );
        assert_eq!(count(&library, "repair_attempt_markers"), 0);
        assert_eq!(count(&library, "asset_audit_markers"), 0);
        let raw:Option<Vec<u8>>=library.conn.lock().unwrap().query_row("SELECT evidence_file_id FROM migration_records WHERE source_table='incomplete_save_repairs' AND source_key='__all_rows__'",[],|r|r.get(0)).unwrap();
        assert!(raw.is_some());
        library.verify().unwrap();
    }
}

#[test]
fn a_failure_after_repair_markers_rolls_back_every_marker_and_can_be_retried_without_duplicate_history(
) {
    let fixture = Fixture::new();
    let (source, library) = load(&fixture);
    assert!(matches!(
        source.normalize_maintenance(&library, &AtomicBool::new(true)),
        Err(Error::Canceled)
    ));
    library.conn.lock().unwrap().execute_batch("CREATE TRIGGER fail_audit BEFORE INSERT ON asset_audit_markers BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
    let report = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert!(!report.unresolved.is_empty());
    assert_eq!(count(&library, "repair_attempt_markers"), 0);
    assert_eq!(count(&library, "asset_audit_markers"), 0);
    library
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER fail_audit")
        .unwrap();
    assert!(source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap()
        .unresolved
        .is_empty());
    assert_eq!(count(&library, "repair_attempt_markers"), 2);
    assert_eq!(count(&library, "asset_audit_markers"), 2);
    library.verify().unwrap();
}

#[test]
fn the_database_rejects_borrowed_snapshots_and_mutation_and_the_audit_detects_wrong_evidence() {
    let fixture = Fixture::new();
    let (source, library) = load(&fixture);
    assert!(source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap()
        .unresolved
        .is_empty());
    let works = library.works(10).unwrap();
    let conn = library.conn.lock().unwrap();
    assert!(conn.execute("INSERT INTO repair_attempt_markers SELECT randomblob(16),?1,snapshot_id,published,origin,attempted_at,evidence_file_id FROM repair_attempt_markers WHERE entity_id=?2",params![works[0].id.as_bytes().as_slice(),works[1].id.as_bytes().as_slice()]).is_err());
    for sql in [
        "UPDATE repair_attempt_markers SET attempted_at=0",
        "DELETE FROM repair_attempt_markers",
        "UPDATE asset_audit_markers SET legacy_rule=99",
        "DELETE FROM asset_audit_markers",
    ] {
        assert!(conn.execute_batch(sql).is_err());
    }
    conn.execute_batch("DROP TRIGGER repair_attempt_marker_update; UPDATE repair_attempt_markers SET evidence_file_id=(SELECT evidence_file_id FROM migration_records WHERE source_table='saved_asset_audits' AND source_key='__all_rows__');").unwrap();
    drop(conn);
    assert!(matches!(library.verify(), Err(Error::Integrity(_))));
}
