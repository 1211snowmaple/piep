use super::*;
use crate::database::Database;
use crate::library::{Library, LibraryId};

#[test]
fn moved_history_is_not_listed_or_deleted_from_the_old_queue_and_paused_jobs_survive_clear_all() {
    let root = std::env::temp_dir().join(format!("piep-history-adapter-{}", LibraryId::now_v7()));
    std::fs::create_dir_all(&root).unwrap();
    {
        let db = Database::open(&root.join("piep.db"), &root.join("downloads")).unwrap();
        let request = StartUpdateJobRequest {
            scope: "all".into(),
            mode: "check_only".into(),
            work_ids: None,
            target_ids: None,
            credentials: None,
            watch_saved: None,
            new_works_only: None,
            adhoc_targets: None,
        };
        for (key, status) in [
            ("moved", "completed"),
            ("paused", "paused"),
            ("current", "completed"),
        ] {
            db.create_update_job(key, &request, &[]).unwrap();
            db.set_update_job_status(key, status, None).unwrap();
        }
        let library = Arc::new(Library::create(&root.join("shelf")).unwrap());
        let job = LibraryId::now_v7();
        {
            let conn = library.conn.lock().unwrap();
            conn.execute("INSERT INTO job_history(id,origin,scope,mode,status,watch_saved,new_works_only,totals,processed,candidate_count,saved_count,error_count,started_at,updated_at) VALUES(?1,'legacy-update-v1','all','check_only','completed',0,0,0,0,0,0,0,0,0)",[job.as_bytes().as_slice()]).unwrap();
            conn.execute(
                "UPDATE job_history SET sealed=1 WHERE id=?1",
                [job.as_bytes().as_slice()],
            )
            .unwrap();
            conn.execute("INSERT INTO migration_records(source_table,source_key,target_id,status) VALUES('update_jobs','moved',?1,'imported')",[job.as_bytes().as_slice()]).unwrap();
        }
        library.switch_over().unwrap();
        let mut state = AppState::new(db);
        state.library_v2 = Arc::new(super::super::library_v2::LibraryV2::with_test_library(
            library.clone(),
        ));
        let paused = LibraryId::now_v7().to_string();
        let current = LibraryId::now_v7().to_string();
        state
            .jobs()
            .create_update_job(&paused, &request, &[])
            .unwrap();
        state
            .jobs()
            .set_update_job_status(&paused, "paused", None)
            .unwrap();
        state
            .jobs()
            .create_update_job(&current, &request, &[])
            .unwrap();
        state
            .jobs()
            .set_update_job_status(&current, "completed", None)
            .unwrap();
        let summaries = current_job_summaries(&state).unwrap();
        assert_eq!(summaries.len(), 2);
        assert!(summaries.iter().all(|j| j.job_id != "moved"));
        assert_eq!(clear_finished_jobs(&state, false).unwrap(), 1);
        assert_eq!(library.job_history(10).unwrap().len(), 1);
        assert_eq!(clear_finished_jobs(&state, true).unwrap(), 1);
        assert!(library.job_history(10).unwrap().is_empty());
        assert_eq!(current_job_summaries(&state).unwrap().len(), 1);
        assert_eq!(
            state.db.update_job_status_value("paused").unwrap(),
            "paused"
        );
        assert_eq!(
            state.db.update_job_status_value("moved").unwrap(),
            "completed"
        );
        assert_eq!(library.job_history_entry(job).unwrap().status, "completed");
        assert_eq!(clear_finished_jobs(&state, true).unwrap(), 0);
        assert_eq!(state.db.list_update_jobs().unwrap().len(), 3);
        library.verify().unwrap();
    }
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    std::fs::remove_dir_all(root).unwrap();
}
