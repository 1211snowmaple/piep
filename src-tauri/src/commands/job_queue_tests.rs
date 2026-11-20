//! 実行キューの排他・復旧・保存結果の所属と、旧キューからの引き継ぎを確かめる。
use super::*;
use crate::library::WorkSave;

struct Temp(std::path::PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (Temp, AppState, Arc<Library>) {
    let root = std::env::temp_dir().join(format!("piep-execution-{}", LibraryId::now_v7()));
    std::fs::create_dir_all(&root).unwrap();
    let library = Arc::new(Library::create(&root.join("shelf")).unwrap());
    library.switch_over().unwrap();
    let mut state =
        AppState::new(Database::open(&root.join("old.db"), &root.join("downloads")).unwrap());
    state.library_v2 = Arc::new(crate::commands::library_v2::LibraryV2::with_test_library(
        library.clone(),
    ));
    (Temp(root), state, library)
}
fn request() -> StartUpdateJobRequest {
    StartUpdateJobRequest {
        scope: "all".into(),
        mode: "check_only".into(),
        work_ids: None,
        target_ids: None,
        credentials: Some(
            serde_json::from_value(serde_json::json!({"pixivRefreshToken":"DO_NOT_STORE"}))
                .unwrap(),
        ),
        watch_saved: Some(true),
        new_works_only: Some(true),
        adhoc_targets: None,
    }
}
fn item(key: &str, kind: &str, status: &str) -> UpdateJobItemInput {
    UpdateJobItemInput {item_type:kind.into(),source:Some("pixiv".into()),source_id:Some(key.into()),target_type:Some("work".into()),title:format!("当時の題名 {key}"),payload_json:" { \"unknown\":9007199254740993, \"decimal\":1.2300, \"originalData\":{\"user\":{\"name\":\"当時の作者\"},\"text\":\"入力の原本\"} } ".into(),status:status.into()}
}
fn id() -> String {
    LibraryId::now_v7().to_string()
}

#[test]
#[ignore = "開発版で実行キューの表示を確かめるための棚を残す"]
fn native_queue_fixture() {
    let (temp, state, library) = fixture();
    let work = save(&library, "1");
    let job = id();
    state
        .jobs()
        .create_update_job_if_idle(
            &job,
            &request(),
            &[
                item("1", "candidate", "queued"),
                item("2", "candidate", "queued"),
            ],
        )
        .unwrap();
    let first = state.jobs().next_update_job_item(&job).unwrap().unwrap();
    state
        .jobs()
        .complete_saved_item(first.id, "saved", None, None, Some(work))
        .unwrap();
    state.jobs().next_update_job_item(&job).unwrap().unwrap();
    state
        .jobs()
        .append_update_job_log(&job, "warn", "確認用の中断記録。次の作品から続けられます")
        .unwrap();
    library.verify().unwrap();
    println!("QUEUE_NATIVE_ROOT={}", temp.0.display());
    println!("QUEUE_NATIVE_FIXTURE={}", library.root().display());
    println!("QUEUE_NATIVE_JOB={job}");
    std::mem::forget(temp);
}
fn save(library: &Library, key: &str) -> LibraryId {
    library.save_work(WorkSave{request:LibraryId::now_v7(),provider:"pixiv",key,route:"test",data:&serde_json::json!({"detail":{"title":"現在の題","user":{"id":77,"name":"現在の作者"}},"text":"本文"}),assets:vec![]}).unwrap().entity_id
}

#[test]
fn native_queue_preserves_exact_inputs_stop_resume_recovery_and_old_database() {
    let (_temp, state, library) = fixture();
    state
        .db
        .create_update_job("old-job", &request(), &[])
        .unwrap();
    state
        .db
        .set_update_job_status("old-job", "completed", None)
        .unwrap();
    let job = id();
    let input = item("1", "work", "queued");
    state
        .jobs()
        .create_update_job_if_idle(
            &job,
            &request(),
            &[input.clone(), item("2", "candidate", "queued")],
        )
        .unwrap();
    assert_eq!(
        library
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT typeof(id),length(id) FROM execution_jobs",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            )
            .unwrap(),
        ("blob".into(), 16)
    );
    assert_eq!(state.db.list_update_jobs().unwrap().len(), 1);
    assert_eq!(state.jobs().list_update_jobs().unwrap().len(), 1);
    assert!(state.jobs().update_job_watch_saved(&job).unwrap());
    assert!(state.jobs().update_job_new_works_only(&job).unwrap());
    assert!(state
        .jobs()
        .create_update_job_if_idle(&id(), &request(), &[])
        .is_err());
    let first = state.jobs().next_update_job_item(&job).unwrap().unwrap();
    assert_eq!(first.source_id.as_deref(), Some("1"));
    assert_eq!(first.payload_json, input.payload_json);
    state
        .jobs()
        .set_update_job_status(&job, "paused", None)
        .unwrap();
    assert!(state.jobs().next_update_job_item(&job).unwrap().is_none());
    assert!(!state
        .jobs()
        .transition_update_job_status(&job, "running", "completed", None)
        .unwrap());
    state.jobs().prepare_update_job_resume(&job, false).unwrap();
    state.jobs().resume_update_job_if_idle(&job, false).unwrap();
    assert_eq!(
        state.jobs().next_update_job_item(&job).unwrap().unwrap().id,
        first.id
    );
    state.jobs().recover_update_jobs_on_startup().unwrap();
    assert_eq!(
        state.jobs().update_job_status_value(&job).unwrap(),
        "paused"
    );
    assert!(state.jobs().next_update_job_item(&job).unwrap().is_none());
    state.jobs().resume_update_job_if_idle(&job, false).unwrap();
    assert_eq!(
        state.jobs().next_update_job_item(&job).unwrap().unwrap().id,
        first.id
    );
    state
        .jobs()
        .complete_update_job_item(first.id, "failed", Some("当時の失敗"), None)
        .unwrap();
    state
        .jobs()
        .set_update_job_status(&job, "failed", None)
        .unwrap();
    state.jobs().resume_update_job_if_idle(&job, true).unwrap();
    assert_eq!(
        state.jobs().next_update_job_item(&job).unwrap().unwrap().id,
        first.id
    );
    assert_eq!(
        state.jobs().update_job_snapshot(&job).unwrap().error_count,
        0
    );
    assert_eq!(
        state.db.update_job_status_value("old-job").unwrap(),
        "completed"
    );
    library.verify().unwrap();
}

#[test]
fn results_belong_to_the_work_and_survive_backup_without_numeric_legacy_ids() {
    let (temp, state, library) = fixture();
    let work = save(&library, "1");
    let wrong = save(&library, "2");
    let job = id();
    state
        .jobs()
        .create_update_job_if_idle(&job, &request(), &[item("1", "candidate", "queued")])
        .unwrap();
    let claimed = state.jobs().next_update_job_item(&job).unwrap().unwrap();
    assert!(state
        .jobs()
        .complete_saved_item(claimed.id, "saved", None, None, Some(wrong))
        .is_err());
    assert_eq!(
        state.jobs().update_job_snapshot(&job).unwrap().saved_count,
        0
    );
    state
        .jobs()
        .complete_saved_item(claimed.id, "saved", None, None, Some(work))
        .unwrap();
    let snapshot = state.jobs().update_job_snapshot(&job).unwrap();
    assert_eq!(
        (snapshot.totals, snapshot.processed, snapshot.saved_count),
        (1, 1, 1)
    );
    assert_eq!(snapshot.candidates[0].work_id, Some(work));
    assert!(snapshot.candidates[0].download_id.is_none());
    assert_eq!(
        state.jobs().list_update_job_item_states(&job).unwrap()[0].work_id,
        Some(work)
    );
    state
        .jobs()
        .complete_saved_item(claimed.id, "saved", None, None, Some(work))
        .unwrap();
    assert_eq!(
        state.jobs().update_job_snapshot(&job).unwrap().saved_count,
        1
    );
    library.backup(&temp.0.join("backup")).unwrap();
    let copy = Library::open(&temp.0.join("backup")).unwrap();
    let queue = NativeQueue {
        library: &copy,
        conn: &copy.conn,
    };
    assert_eq!(
        queue.update_job_snapshot(&job).unwrap().candidates[0].work_id,
        Some(work)
    );
    copy.verify().unwrap();
    assert!(state.db.list_update_jobs().unwrap().is_empty());
}

#[test]
fn failed_creation_rolls_back_headers_files_identities_and_items() {
    let (_temp, state, library) = fixture();
    library.conn.lock().unwrap().execute_batch("CREATE TRIGGER refuse_queue_item BEFORE INSERT ON execution_items WHEN NEW.source_id='2' BEGIN SELECT RAISE(ABORT,'injected failure'); END").unwrap();
    assert!(state
        .jobs()
        .create_update_job_if_idle(
            &id(),
            &request(),
            &[item("1", "work", "queued"), item("2", "work", "queued")]
        )
        .is_err());
    let conn = library.conn.lock().unwrap();
    for table in [
        "execution_jobs",
        "execution_items",
        "execution_logs",
        "external_identities",
        "content_files",
    ] {
        assert_eq!(
            conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "{table}"
        );
    }
}

#[test]
fn concurrent_creation_and_claims_reserve_one_job_and_one_item() {
    let (_temp, state, _library) = fixture();
    let state = Arc::new(state);
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let threads = (0..2)
        .map(|_| {
            let state = state.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let job = id();
                (
                    job.clone(),
                    state
                        .jobs()
                        .create_update_job_if_idle(&job, &request(), &[item("1", "work", "queued")])
                        .is_ok(),
                )
            })
        })
        .collect::<Vec<_>>();
    let results = threads
        .into_iter()
        .map(|t| t.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|(_, ok)| *ok).count(), 1);
    let job = results.into_iter().find(|(_, ok)| *ok).unwrap().0;
    let threads = (0..2)
        .map(|_| {
            let state = state.clone();
            let job = job.clone();
            std::thread::spawn(move || state.jobs().next_update_job_item(&job).unwrap())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        threads
            .into_iter()
            .filter_map(|t| t.join().unwrap())
            .count(),
        1
    );
}

#[test]
fn paging_and_display_use_projections_without_opening_compressed_inputs() {
    let (temp, state, library) = fixture();
    let job = id();
    let items = (0..201)
        .map(|i| item(&i.to_string(), "candidate", "candidate"))
        .collect::<Vec<_>>();
    state
        .jobs()
        .create_update_job_if_idle(&job, &request(), &items)
        .unwrap();
    assert!(!state
        .jobs()
        .insert_update_job_candidate(&job, &items[0])
        .unwrap());
    for i in 0..305 {
        state
            .jobs()
            .append_update_job_log(&job, "info", &format!("記録{i}"))
            .unwrap();
    }
    std::fs::rename(
        library.root().join("objects"),
        temp.0.join("objects-offline"),
    )
    .unwrap();
    let first = state.jobs().update_job_snapshot(&job).unwrap();
    assert_eq!(first.candidates.len(), 200);
    assert_eq!(first.logs.len(), 300);
    assert_eq!(
        first.candidates[0].metadata.author_name.as_deref(),
        Some("当時の作者")
    );
    let next = state
        .jobs()
        .update_job_snapshot_page(&job, first.next_candidate_cursor, first.previous_log_cursor)
        .unwrap();
    assert_eq!(next.candidates.len(), 1);
    assert!(next.next_candidate_cursor.is_none());
    assert_eq!(next.logs.len(), 6);
    assert_eq!(next.logs[1].message, "記録0");
    assert_eq!(
        state
            .jobs()
            .list_update_job_item_states(&job)
            .unwrap()
            .len(),
        201
    );
    assert_eq!(
        state
            .jobs()
            .update_job_progress_delta(&job, None)
            .unwrap()
            .summary
            .candidate_count,
        201
    );
    std::fs::rename(
        temp.0.join("objects-offline"),
        library.root().join("objects"),
    )
    .unwrap();
    library.verify().unwrap();
}

#[test]
fn candidate_selection_is_scoped_and_resume_cannot_race_another_active_job() {
    let (_temp, state, library) = fixture();
    let job = id();
    state
        .jobs()
        .create_update_job_if_idle(
            &job,
            &request(),
            &[
                item("1", "candidate", "candidate"),
                item("2", "work", "queued"),
            ],
        )
        .unwrap();
    state
        .jobs()
        .set_update_job_status(&job, "paused", None)
        .unwrap();
    assert_eq!(
        state
            .jobs()
            .queue_update_job_candidates(&job, &[i64::MAX])
            .unwrap(),
        0
    );
    let selected = state.jobs().update_job_snapshot(&job).unwrap().candidates[0].id;
    assert_eq!(
        state
            .jobs()
            .queue_update_job_candidates(&job, &[selected])
            .unwrap(),
        1
    );
    let other = id();
    state
        .jobs()
        .create_update_job_if_idle(&other, &request(), &[])
        .unwrap();
    assert!(state.jobs().resume_update_job_if_idle(&job, false).is_err());
    state
        .jobs()
        .set_update_job_status(&other, "completed", None)
        .unwrap();
    state.jobs().resume_update_job_if_idle(&job, false).unwrap();
    assert_eq!(
        state.jobs().next_update_job_item(&job).unwrap().unwrap().id,
        selected
    );
    assert!(state.jobs().next_update_job_item(&job).unwrap().is_none());
    assert_eq!(state.jobs().clear_finished_update_jobs().unwrap(), 1);
    assert_eq!(state.jobs().list_update_jobs().unwrap().len(), 1);
    library.verify().unwrap();
}

#[test]
fn previous_queue_import_is_atomic_idempotent_and_does_not_resurrect_cleared_jobs() {
    let (_temp, state, library) = fixture();
    let input = item("1", "candidate", "queued");
    state
        .db
        .create_update_job("previous-runtime", &request(), std::slice::from_ref(&input))
        .unwrap();
    let original = state
        .db
        .next_update_job_item("previous-runtime")
        .unwrap()
        .unwrap();
    state
        .db
        .append_update_job_log("previous-runtime", "future-type", "当時の操作記録")
        .unwrap();
    library.conn.lock().unwrap().execute_batch("CREATE TRIGGER reject_queue_import BEFORE INSERT ON execution_logs WHEN NEW.log_type='future-type' BEGIN SELECT RAISE(ABORT,'injected failure'); END").unwrap();
    assert!(state.jobs().import_previous_queue().is_err());
    assert!(state.jobs().list_update_jobs().unwrap().is_empty());
    assert_eq!(
        state
            .db
            .update_job_status_value("previous-runtime")
            .unwrap(),
        "running"
    );
    library
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_queue_import")
        .unwrap();
    state.jobs().import_previous_queue().unwrap();
    state.jobs().import_previous_queue().unwrap();
    let jobs = state.jobs().list_update_jobs().unwrap();
    assert_eq!(jobs.len(), 1);
    let job = &jobs[0].job_id;
    assert_ne!(job, "previous-runtime");
    assert!(job.parse::<LibraryId>().is_ok());
    let snapshot = state.jobs().update_job_snapshot(job).unwrap();
    assert_eq!(snapshot.logs.last().unwrap().message, "当時の操作記録");
    assert_eq!(snapshot.logs.last().unwrap().log_type, "future-type");
    {
        let conn = library.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT source_sequence FROM execution_items", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            original.id
        );
        let file = conn
            .query_row("SELECT payload_file_id FROM execution_items", [], |r| {
                r.get::<_, Vec<u8>>(0)
            })
            .unwrap();
        assert_eq!(
            library
                .read_job_payload(&conn, crate::library::Fingerprint(file.try_into().unwrap()))
                .unwrap(),
            input.payload_json
        );
    }
    state.jobs().recover_update_jobs_on_startup().unwrap();
    assert_eq!(state.jobs().update_job_status_value(job).unwrap(), "paused");
    assert!(state.jobs().next_update_job_item(job).unwrap().is_none());
    state.jobs().clear_update_job(job).unwrap();
    state.jobs().import_previous_queue().unwrap();
    assert!(state.jobs().list_update_jobs().unwrap().is_empty());
    assert_eq!(
        state
            .db
            .update_job_status_value("previous-runtime")
            .unwrap(),
        "running"
    );
    assert_eq!(
        state
            .db
            .update_job_snapshot("previous-runtime")
            .unwrap()
            .logs
            .last()
            .unwrap()
            .message,
        "当時の操作記録"
    );
    library.verify().unwrap();
}

#[path = "job_queue_migration_tests.rs"]
mod migration;
