use super::*;
use crate::library::{LibraryId, ResourceScope};
use rusqlite::Connection;
use std::path::PathBuf;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("piep-job-history-{}", LibraryId::now_v7()));
        let version = root.join("old/downloads/pixiv/one/v1");
        std::fs::create_dir_all(&version).unwrap();
        let json = version.join("original.json");
        std::fs::write(
            &json,
            b"{\"detail\":{\"title\":\"book\"},\"text\":\"body\"}",
        )
        .unwrap();
        let db = Connection::open(root.join("old/piep.db")).unwrap();
        db.execute_batch("CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER);
CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT);
CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER);
CREATE TABLE update_jobs(id TEXT PRIMARY KEY,scope TEXT,mode TEXT,status TEXT,watch_saved INTEGER,new_works_only INTEGER,totals INTEGER,processed INTEGER,candidate_count INTEGER,saved_count INTEGER,error_count INTEGER,active_label TEXT,started_at TEXT,updated_at TEXT,finished_at TEXT);
CREATE TABLE update_job_items(id INTEGER PRIMARY KEY,job_id TEXT,item_type TEXT,source TEXT,source_id TEXT,target_type TEXT,title TEXT,payload_json TEXT,status TEXT,error TEXT,result_download_id INTEGER,created_at TEXT,updated_at TEXT);
CREATE TABLE update_job_logs(id INTEGER PRIMARY KEY,job_id TEXT,log_type TEXT,message TEXT,created_at TEXT);
INSERT INTO downloads VALUES(1,'pixiv','one','book','author',0);
INSERT INTO update_jobs VALUES('job-z','author','auto','completed',1,0,100,7,3,1,2,'当時の表示','2025-01-01 00:00:00','2025-01-02 00:00:00','2025-01-02 00:00:00');
INSERT INTO update_jobs VALUES('job-a','work','manual','completed',0,1,0,0,0,0,0,NULL,'2025-01-02 00:00:00','2025-01-02 00:00:00',NULL);
INSERT INTO update_job_logs VALUES(9,'job-z','error','後の記録','2025-01-02 00:00:00');
INSERT INTO update_job_logs VALUES(2,'job-z','info','先の記録','2025-01-02 00:00:00');
INSERT INTO update_job_logs VALUES(1,'job-a','future-type','原文を保つ','2025-01-02 00:00:00');").unwrap();
        db.execute(
            "INSERT INTO download_versions VALUES(1,1,1,?1,?1,'source')",
            [json.to_str().unwrap()],
        )
        .unwrap();
        for (id, kind, key, target, status, result, error) in [
            (10, "candidate", "one", "author", "saved", Some(1), None),
            (3, "target", "42", "author", "done", None, None),
            (
                20,
                "candidate",
                "held",
                "work",
                "held",
                None,
                Some("制限の理由"),
            ),
        ] {
            db.execute("INSERT INTO update_job_items VALUES(?1,'job-z',?2,'pixiv',?3,?4,?5,?6,?7,?8,?9,'2025-01-01 00:00:00','2025-01-02T00:00:00+09:00')",params![id,kind,key,target,format!("当時の題名 {id}"),payload(),status,error,result]).unwrap();
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
fn payload() -> &'static str {
    "{ \"future\": {\"decimal\":0.07221960411257732,\"integer\":9007199254740993},\"knownHold\":true }\r\n"
}
fn count(library: &Library, table: &str) -> i64 {
    assert!(matches!(
        table,
        "job_history" | "job_history_items" | "job_history_logs"
    ));
    library
        .conn
        .lock()
        .unwrap()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn completed_history_keeps_scope_counts_order_labels_results_and_exact_payloads_in_an_independent_backup(
) {
    let fixture = Fixture::new();
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    let report = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
    let jobs = library.job_history(10).unwrap();
    assert_eq!(jobs.len(), 2);
    assert_eq!(
        library.job_history_page(Some(jobs[0].id), 1).unwrap(),
        jobs[1..]
    );
    assert!(library
        .job_history_page(Some(jobs[1].id), 1)
        .unwrap()
        .is_empty());
    assert!(matches!(
        library.job_history_page(Some(LibraryId::now_v7()), 1),
        Err(Error::ResourceUnavailable)
    ));
    assert_eq!(jobs[0].scope, "work");
    assert!(jobs[0].finished_at.is_none());
    let job = jobs[1].id;
    assert_eq!(library.job_history_entry(job).unwrap(), jobs[1]);
    assert_eq!(
        (
            jobs[1].totals,
            jobs[1].processed,
            jobs[1].candidate_count,
            jobs[1].saved_count,
            jobs[1].error_count
        ),
        (100, 7, 3, 1, 2)
    );
    assert_eq!(jobs[1].active_label.as_deref(), Some("当時の表示"));
    assert!(jobs[1].watch_saved);
    let items = library.job_history_items(job, None, 10).unwrap();
    assert_eq!(
        items.iter().map(|i| i.sequence).collect::<Vec<_>>(),
        [3, 10, 20]
    );
    let work = library.works(1).unwrap().remove(0);
    assert_eq!(items[1].entity, Some(work.id));
    assert_eq!(items[1].result_work, Some(work.id));
    assert_eq!(items[1].target_type.as_deref(), Some("author"));
    assert_ne!(items[0].entity, Some(work.id));
    assert_eq!(items[2].status, "held");
    assert_eq!(items[2].error.as_deref(), Some("制限の理由"));
    assert_eq!(items[1].updated_at, 1735743600000);
    for item in &items {
        assert_eq!(
            library.job_history_payload(job, item.id).unwrap(),
            payload()
        );
    }
    let logs = library.job_history_logs(job, None, 10).unwrap();
    assert_eq!(logs.iter().map(|i| i.sequence).collect::<Vec<_>>(), [2, 9]);
    assert_eq!(logs[0].message, "先の記録");
    let first = library.job_history_items(job, None, 1).unwrap();
    assert_eq!(
        library
            .job_history_items(job, Some(first[0].sequence), 2)
            .unwrap(),
        items[1..]
    );
    assert_eq!(
        library.job_history_logs(job, Some(2), 1).unwrap(),
        logs[1..]
    );
    assert!(matches!(
        library.job_history_payload(jobs[0].id, items[0].id),
        Err(Error::ResourceUnavailable)
    ));
    assert!(library.job_history(0).is_err());
    assert!(library.job_history_items(job, None, 1001).is_err());
    assert!(matches!(
        library.job_history_logs(LibraryId::now_v7(), None, 1),
        Err(Error::ResourceUnavailable)
    ));
    assert!(source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap()
        .unresolved
        .is_empty());
    assert_eq!(library.job_history(10).unwrap(), jobs);
    assert_eq!(library.job_history_items(job, None, 10).unwrap(), items);
    assert_eq!(library.job_history_logs(job, None, 10).unwrap(), logs);
    library
        .reader_document(work.id, ResourceScope::Snapshot(work.snapshot_id))
        .unwrap();
    library.backup(&fixture.0.join("backup")).unwrap();
    drop(source);
    drop(library);
    std::fs::rename(fixture.0.join("old"), fixture.0.join("unavailable")).unwrap();
    let backup = Library::open(&fixture.0.join("backup")).unwrap();
    assert_eq!(backup.job_history(10).unwrap(), jobs);
    assert_eq!(backup.job_history_items(job, None, 10).unwrap(), items);
    assert_eq!(backup.job_history_logs(job, None, 10).unwrap(), logs);
    assert_eq!(
        backup.job_history_payload(job, items[1].id).unwrap(),
        payload()
    );
    backup.verify().unwrap();
}

#[test]
fn resumable_or_unknown_jobs_and_unprocessed_items_never_turn_into_completed_history() {
    for status in [
        "running",
        "queued",
        "paused",
        "canceling",
        "canceled",
        "auth_required",
        "failed",
        "future",
    ] {
        let fixture = Fixture::new();
        fixture.change(&format!(
            "UPDATE update_jobs SET status='{status}' WHERE id='job-z'"
        ));
        assert_blocked(&fixture);
    }
    for status in ["running", "queued", "candidate", "future"] {
        let fixture = Fixture::new();
        fixture.change(&format!(
            "UPDATE update_job_items SET status='{status}' WHERE id=20"
        ));
        assert_blocked(&fixture);
    }
}
fn assert_blocked(fixture: &Fixture) {
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    let report = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert!(
        report
            .unresolved
            .iter()
            .any(|p| p.contains("job_history") || p.contains("ジョブ")),
        "{:?}",
        report.unresolved
    );
    for table in ["job_history", "job_history_items", "job_history_logs"] {
        assert_eq!(count(&library, table), 0);
    }
    let conn = library.conn.lock().unwrap();
    assert_eq!(conn.query_row("SELECT status FROM migration_records WHERE source_table='update_jobs' AND source_key='__all_rows__'",[],|r|r.get::<_,String>(0)).unwrap(),"unresolved");
    drop(conn);
    library.verify().unwrap();
}

#[test]
fn corrupt_payload_dates_or_ownership_roll_back_all_history_while_source_records_remain_preserved()
{
    for sql in [
        "UPDATE update_job_items SET payload_json='not JSON' WHERE id=10",
        "UPDATE update_job_logs SET created_at='not a date' WHERE id=9",
        "UPDATE update_job_logs SET job_id='missing' WHERE id=9",
        "UPDATE update_job_items SET source_id='other' WHERE id=10",
        "UPDATE update_job_items SET result_download_id=999 WHERE id=10",
        "UPDATE update_job_items SET source=NULL WHERE id=10",
        "UPDATE update_jobs SET totals=-1 WHERE id='job-z'",
    ] {
        let fixture = Fixture::new();
        fixture.change(sql);
        assert_blocked(&fixture);
    }
}

#[test]
fn errors_and_cancellation_do_not_publish_half_a_history_and_sealed_records_cannot_be_changed() {
    let fixture = Fixture::new();
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    assert!(matches!(
        source.normalize_job_history(&library, &AtomicBool::new(true)),
        Err(Error::Canceled)
    ));
    assert_eq!(count(&library, "job_history"), 0);
    library.conn.lock().unwrap().execute_batch("CREATE TRIGGER refuse_history_log BEFORE INSERT ON job_history_logs BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
    let report = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert!(!report.unresolved.is_empty());
    for table in ["job_history", "job_history_items", "job_history_logs"] {
        assert_eq!(count(&library, table), 0);
    }
    library
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER refuse_history_log")
        .unwrap();
    assert!(source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap()
        .unresolved
        .is_empty());
    let conn = library.conn.lock().unwrap();
    let pending = LibraryId::now_v7();
    conn.execute("INSERT INTO job_history(id,origin,scope,mode,status,watch_saved,new_works_only,totals,processed,candidate_count,saved_count,error_count,started_at,updated_at) VALUES(?1,'legacy-update-v1','work','auto','completed',0,0,0,0,0,0,0,0,0)",[pending.as_bytes().as_slice()]).unwrap();
    assert!(conn.execute("INSERT INTO job_history_items SELECT randomblob(16),?1,999,item_type,(SELECT entity_id FROM job_history_items WHERE item_type='target' LIMIT 1),target_type,title,payload_file_id,status,error,NULL,created_at,updated_at FROM job_history_items WHERE item_type='candidate' LIMIT 1",[pending.as_bytes().as_slice()]).is_err());
    conn.execute("INSERT INTO job_history_items SELECT randomblob(16),?1,sequence,item_type,entity_id,target_type,title,payload_file_id,status,error,result_work_id,created_at,updated_at FROM job_history_items WHERE item_type='candidate' AND status='saved' LIMIT 1",[pending.as_bytes().as_slice()]).unwrap();
    assert!(conn.execute("UPDATE job_history_items SET entity_id=(SELECT entity_id FROM job_history_items WHERE item_type='target' LIMIT 1),result_work_id=NULL WHERE job_id=?1",[pending.as_bytes().as_slice()]).is_err());
    assert!(conn.execute("UPDATE job_history_items SET result_work_id=(SELECT entity_id FROM job_history_items WHERE status='held' LIMIT 1) WHERE job_id=?1",[pending.as_bytes().as_slice()]).is_err());
    conn.execute(
        "DELETE FROM job_history_items WHERE job_id=?1",
        [pending.as_bytes().as_slice()],
    )
    .unwrap();
    conn.execute(
        "DELETE FROM job_history WHERE id=?1",
        [pending.as_bytes().as_slice()],
    )
    .unwrap();
    for sql in [
        "UPDATE job_history SET scope='other'", "DELETE FROM job_history",
        "UPDATE job_history_items SET title='other'", "DELETE FROM job_history_items",
        "UPDATE job_history_logs SET message='other'", "DELETE FROM job_history_logs",
        "INSERT INTO job_history_logs SELECT randomblob(16),job_id,999,log_type,message,created_at FROM job_history_logs LIMIT 1",
    ] { assert!(conn.execute_batch(sql).is_err(),"{sql}"); }
    drop(conn);
    library.verify().unwrap();
}
