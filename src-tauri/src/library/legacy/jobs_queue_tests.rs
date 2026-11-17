use super::*;
use crate::library::job_fixture::*;

#[test]
fn supported_job_states_and_pending_candidates_keep_their_own_queue_and_exact_originals() {
    for status in [
        "queued",
        "running",
        "paused",
        "canceling",
        "canceled",
        "auth_required",
        "failed",
        "completed",
    ] {
        let fixture = Fixture::new();
        prepare(&fixture, status);
        add_pending(&fixture, "candidate");
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let library = Library::create(&fixture.0.join("new")).unwrap();
        successful(&source, &library);
        let job = moved(&library, "job-z");
        assert_eq!(library.job_history(10).unwrap().len(), 1);
        let conn = library.conn.lock().unwrap();
        let row=conn.query_row("SELECT status,totals,processed,candidate_count,saved_count,error_count,started_at,finished_at FROM execution_jobs WHERE id=?1",[job.as_bytes().as_slice()],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,i64>(5)?,r.get::<_,String>(6)?,r.get::<_,Option<String>>(7)?))).unwrap();
        assert_eq!(
            row,
            (
                status.into(),
                100,
                7,
                3,
                1,
                2,
                "2025-01-01 00:00:00".into(),
                Some("2025-01-02 00:00:00".into())
            )
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM execution_items WHERE job_id=?1",
                [job.as_bytes().as_slice()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            4
        );
        assert_eq!(
            conn.query_row(
                "SELECT message FROM execution_logs ORDER BY id LIMIT 1",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "先の記録"
        );
        let ids = conn
            .prepare("SELECT id,source_sequence FROM execution_items ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(ids, [(3, 3), (10, 10), (20, 20), (30, 30)]);
        drop(conn);
        assert_eq!(raw_payload(&library, 30), payload());
        successful(&source, &library);
        assert_eq!(moved(&library, "job-z"), job);
        assert_eq!(
            source
                .conn
                .query_row("SELECT status FROM update_jobs WHERE id='job-z'", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            status
        );
        library.verify().unwrap();
        library.switch_over().unwrap();
    }
}

#[test]
fn catching_up_preserves_ids_and_original_evidence_then_seals_only_finished_jobs() {
    let fixture = Fixture::new();
    prepare(&fixture, "paused");
    add_pending(&fixture, "queued");
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    successful(&source, &library);
    let job = moved(&library, "job-z");
    let first = library.job_history(10).unwrap();
    let files = library
        .conn
        .lock()
        .unwrap()
        .prepare("SELECT file_id FROM job_import_evidence ORDER BY source_table")
        .unwrap()
        .query_map([], |r| r.get::<_, Vec<u8>>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(files.len(), 3);
    successful(&source, &library);
    assert_eq!(
        library
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM job_import_evidence", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        3
    );
    fixture.change("UPDATE update_job_items SET status='done' WHERE id=30; UPDATE update_jobs SET processed=8,updated_at='2025-01-03 00:00:00' WHERE id='job-z'; INSERT INTO update_job_logs VALUES(12,'job-z','future-type','追いつきの原文','2025-01-03 00:00:00')");
    successful(&source, &library);
    assert_eq!(moved(&library, "job-z"), job);
    assert_eq!(library.job_history(10).unwrap(), first);
    assert_eq!(
        library
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT status FROM execution_items WHERE id=30", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        "done"
    );
    for hash in files {
        assert!(library
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM job_import_evidence WHERE file_id=?1)",
                [hash],
                |r| r.get::<_, bool>(0)
            )
            .unwrap());
    }
    fixture.change("UPDATE update_jobs SET status='completed' WHERE id='job-z'");
    successful(&source, &library);
    assert_eq!(library.job_history(10).unwrap().len(), 2);
    assert_eq!(library.job_history_entry(job).unwrap().processed, 8);
    assert_eq!(
        library
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM execution_jobs", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    successful(&source, &library);
    assert_eq!(moved(&library, "job-z"), job);
    library.verify().unwrap();
}

#[test]
fn changed_execution_rows_roll_back_together_and_old_jobs_can_be_removed() {
    let fixture = Fixture::new();
    prepare(&fixture, "paused");
    add_pending(&fixture, "queued");
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    successful(&source, &library);
    let job = moved(&library, "job-z");
    fixture.change("UPDATE update_job_items SET title='新しい題名' WHERE id=30; UPDATE update_job_logs SET created_at='bad date' WHERE id=9");
    let report = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert!(!report.unresolved.is_empty());
    assert!(library.switch_over().is_err());
    assert_eq!(
        library
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT title FROM execution_items WHERE id=30", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        "まだの題名"
    );
    assert_eq!(moved(&library, "job-z"), job);
    fixture.change("UPDATE update_job_logs SET created_at='2025-01-02 00:00:00' WHERE id=9");
    successful(&source, &library);
    fixture.change("DELETE FROM update_job_items WHERE job_id='job-z'; DELETE FROM update_job_logs WHERE job_id='job-z'; DELETE FROM update_jobs WHERE id='job-z'");
    successful(&source, &library);
    assert_eq!(
        library
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM execution_jobs", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(library.job_history(10).unwrap().len(), 1);
    library.verify().unwrap();
}

#[test]
fn older_sealed_history_gets_a_baseline_before_refresh_and_is_never_overwritten() {
    let fixture = Fixture::new();
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    successful(&source, &library);
    library
        .conn
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM migration_fingerprints WHERE source_table='update_jobs'",
            [],
        )
        .unwrap();
    let original = library.job_history(10).unwrap();
    fixture.change("INSERT INTO update_jobs SELECT 'new-job',scope,'check_only','paused',watch_saved,new_works_only,totals,processed,candidate_count,saved_count,error_count,active_label,started_at,updated_at,finished_at FROM update_jobs WHERE id='job-a'");
    successful(&source, &library);
    assert_eq!(library.job_history(10).unwrap(), original);
    fixture.change("UPDATE update_job_logs SET message='改変した原文' WHERE id=9");
    let report = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert!(report.unresolved.iter().any(|v| v.contains("封をした原本")));
    assert!(library.switch_over().is_err());
    assert_eq!(library.job_history(10).unwrap(), original);
    let job = original.iter().find(|j| j.scope == "author").unwrap().id;
    assert_eq!(
        library.job_history_logs(job, None, 10).unwrap()[1].message,
        "後の記録"
    );
    fixture.change("UPDATE update_job_logs SET message='後の記録' WHERE id=9");
    successful(&source, &library);
    library.verify().unwrap();
}

#[test]
fn an_old_recheck_cannot_undo_a_newer_dismissal_during_migration() {
    let fixture = Fixture::new();
    permission(&fixture, "dismissed");
    assert_blocked(&fixture);
}

#[test]
fn unknown_columns_in_the_original_history_remain_referenced_after_a_failed_refresh_and_reclaim() {
    let fixture = Fixture::new();
    fixture.change("ALTER TABLE update_jobs ADD COLUMN future_note TEXT; UPDATE update_jobs SET future_note='未知の原記録' WHERE id='job-z'");
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    successful(&source, &library);
    let file=library.conn.lock().unwrap().query_row("SELECT evidence_file_id FROM migration_records WHERE source_table='update_jobs' AND source_key='__all_rows__'",[],|r|r.get::<_,Vec<u8>>(0)).unwrap();
    library
        .conn
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM migration_fingerprints WHERE source_table='update_jobs'",
            [],
        )
        .unwrap();
    fixture.change("UPDATE update_jobs SET future_note='後の値' WHERE id='job-z'");
    let report = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert!(!report.unresolved.is_empty());
    library
        .reclaim(crate::library::ReclaimOptions {
            grace: std::time::Duration::ZERO,
            hold: std::time::Duration::ZERO,
        })
        .unwrap();
    assert!(library
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM job_import_evidence WHERE file_id=?1)",
            [&file],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
    let stored = library
        .file(crate::library::Fingerprint(file.try_into().unwrap()))
        .unwrap();
    let mut bytes = Vec::new();
    library.objects.export(&stored, &mut bytes).unwrap();
    assert!(bytes
        .split(|b| *b == b'\n')
        .filter(|line| !line.is_empty())
        .any(|line| {
            let value: serde_json::Value = serde_json::from_slice(line).unwrap();
            value["future_note"]["sqlite_text_bytes"]
                == serde_json::json!("未知の原記録".as_bytes())
        }));
    library.verify().unwrap();
}

#[test]
fn empty_queue_evidence_is_idempotent_from_the_first_successful_import() {
    let fixture = Fixture::new();
    fixture.change(
        "DELETE FROM update_job_items; DELETE FROM update_job_logs; DELETE FROM update_jobs",
    );
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    successful(&source, &library);
    assert_eq!(
        library
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM job_import_evidence", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        3
    );
    successful(&source, &library);
    assert_eq!(
        library
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM job_import_evidence", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        3
    );
    assert!(library.job_history(10).unwrap().is_empty());
    library.verify().unwrap();
}

#[test]
fn an_existing_sealed_job_never_silently_replaces_a_resumable_queue() {
    let fixture = Fixture::new();
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let old = Library::create(&fixture.0.join("new")).unwrap();
    successful(&source, &old);
    let originals = old.job_history(10).unwrap();
    fixture.change("UPDATE update_jobs SET mode='check_only' WHERE id='job-z'; UPDATE update_job_items SET status='failed' WHERE id=20");
    let report = source
        .import(&old, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert!(report
        .unresolved
        .iter()
        .any(|v| v.contains("実行キューとして移してください")));
    assert!(old.switch_over().is_err());
    assert_eq!(old.job_history(10).unwrap(), originals);
    let fresh = Library::create(&fixture.0.join("fresh")).unwrap();
    successful(&source, &fresh);
    assert_eq!(fresh.job_history(10).unwrap().len(), 1);
    assert_eq!(
        fresh
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM execution_jobs", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    fresh.switch_over().unwrap();
    fresh.verify().unwrap();
}
