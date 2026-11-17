//! 旧棚を移した後、実際のアプリの入口で停止・再開と保留の世代を確かめる。
use super::*;
use crate::library::job_fixture::*;
use crate::library::LegacyImporter;
use rusqlite::Connection;
fn state(fixture: &Fixture, library: Arc<Library>) -> crate::AppState {
    let mut state = crate::AppState::new(
        crate::database::Database::open(
            &fixture.0.join("runtime.db"),
            &fixture.0.join("runtime-downloads"),
        )
        .unwrap(),
    );
    state.library_v2 = Arc::new(crate::commands::library_v2::LibraryV2::with_test_library(
        library,
    ));
    state
}

#[test]
fn completed_jobs_with_unprocessed_or_retryable_candidates_remain_operable() {
    for status in ["queued", "running", "failed"] {
        let fixture = Fixture::new();
        prepare(&fixture, "completed");
        add_pending(&fixture, status);
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let library = Arc::new(Library::create(&fixture.0.join("new")).unwrap());
        successful(&source, &library);
        let job = moved(&library, "job-z").to_string();
        assert_eq!(library.job_history(10).unwrap().len(), 1);
        library.switch_over().unwrap();
        let state = state(&fixture, library);
        if status == "failed" {
            assert_eq!(
                state
                    .jobs()
                    .queue_update_job_candidates(&job, &[30])
                    .unwrap(),
                1
            );
        } else {
            assert_eq!(
                state.jobs().update_job_status_value(&job).unwrap(),
                "paused"
            );
            state.jobs().resume_update_job_if_idle(&job, false).unwrap();
        }
        if status == "failed" {
            state
                .jobs()
                .set_update_job_status(&job, "queued", None)
                .unwrap();
        }
        assert_eq!(
            state.jobs().next_update_job_item(&job).unwrap().unwrap().id,
            30
        );
        assert!(state.db.list_update_jobs().unwrap().is_empty());
    }
}

#[test]
fn backup_can_resume_the_unfinished_item_after_the_old_shelf_is_unavailable() {
    let fixture = Fixture::new();
    prepare(&fixture, "running");
    add_pending(&fixture, "running");
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    successful(&source, &library);
    let job = moved(&library, "job-z").to_string();
    library.switch_over().unwrap();
    library.backup(&fixture.0.join("backup")).unwrap();
    drop(source);
    drop(library);
    std::fs::rename(fixture.0.join("old"), fixture.0.join("offline")).unwrap();
    let copy = Arc::new(Library::open(&fixture.0.join("backup")).unwrap());
    let state = state(&fixture, copy.clone());
    state.jobs().recover_update_jobs_on_startup().unwrap();
    assert_eq!(
        state.jobs().update_job_status_value(&job).unwrap(),
        "paused"
    );
    assert!(state.jobs().next_update_job_item(&job).unwrap().is_none());
    state.jobs().resume_update_job_if_idle(&job, false).unwrap();
    let claimed = state.jobs().next_update_job_item(&job).unwrap().unwrap();
    assert_eq!(claimed.id, 30);
    assert_eq!(claimed.payload_json, payload());
    assert_eq!(
        state
            .jobs()
            .list_update_job_item_states(&job)
            .unwrap()
            .iter()
            .find(|i| i.status == "saved")
            .unwrap()
            .work_id,
        Some(copy.works(1).unwrap()[0].id)
    );
    copy.verify().unwrap();
}

#[test]
fn old_permission_checks_bind_the_migrated_decision_without_changing_the_original_input() {
    let fixture = Fixture::new();
    let raw = permission(&fixture, "held");
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Arc::new(Library::create(&fixture.0.join("new")).unwrap());
    successful(&source, &library);
    assert_eq!(raw_payload(&library, 10), raw);
    let job = moved(&library, "job-z").to_string();
    library.switch_over().unwrap();
    let state = state(&fixture, library.clone());
    state.jobs().resume_update_job_if_idle(&job, false).unwrap();
    let claimed = state.jobs().next_update_job_item(&job).unwrap().unwrap();
    let value: serde_json::Value = serde_json::from_str(&claimed.payload_json).unwrap();
    assert_eq!(value["discoveryGeneration"], 0);
    assert_eq!(raw_payload(&library, 10), raw);
    state
        .candidates()
        .set_update_candidate_status("pixiv", "one", "dismissed")
        .unwrap();
    let update = crate::database::UpdateCandidateInput {
        source: "pixiv".into(),
        source_id: "one".into(),
        kind: "revision".into(),
        title: "保留の題".into(),
        payload_json: raw,
        target_type: Some("work".into()),
    };
    assert!(state
        .candidates()
        .recheck_available(&update, value["discoveryGeneration"].as_i64())
        .is_err());
    assert_eq!(
        state
            .candidates()
            .update_candidate_status("pixiv", "one")
            .unwrap()
            .as_deref(),
        Some("dismissed")
    );
    library.verify().unwrap();
}

#[test]
fn queued_targets_keep_the_legacy_identity_and_checkpoint_in_the_original_input() {
    let fixture = Fixture::new();
    prepare(&fixture, "paused");
    let raw=serde_json::json!({"id":0,"targetType":"author","source":"pixiv","sourceKey":"42","displayName":"当時の作者","enabled":true,"lastSeenSourceId":"checkpoint","createdAt":"2025-01-01 00:00:00","updatedAt":"2025-01-02 00:00:00"}).to_string();
    Connection::open(fixture.0.join("old/piep.db"))
        .unwrap()
        .execute(
            "UPDATE update_job_items SET status='queued',payload_json=?1 WHERE id=3",
            [&raw],
        )
        .unwrap();
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Arc::new(Library::create(&fixture.0.join("new")).unwrap());
    successful(&source, &library);
    let job = moved(&library, "job-z").to_string();
    library.switch_over().unwrap();
    let state = state(&fixture, library.clone());
    state.jobs().resume_update_job_if_idle(&job, false).unwrap();
    let claimed = state.jobs().next_update_job_item(&job).unwrap().unwrap();
    assert_eq!(claimed.id, 3);
    assert_eq!(claimed.payload_json, raw);
    let target: crate::database::UpdateTarget =
        serde_json::from_str(&claimed.payload_json).unwrap();
    assert_eq!(target.last_seen_source_id.as_deref(), Some("checkpoint"));
    assert_eq!(raw_payload(&library, 3), raw);
    library.verify().unwrap();
}

#[test]
fn every_resumable_legacy_state_can_continue_through_the_native_app_queue() {
    for status in [
        "queued",
        "running",
        "canceling",
        "paused",
        "canceled",
        "auth_required",
        "failed",
    ] {
        let fixture = Fixture::new();
        prepare(&fixture, status);
        add_pending(
            &fixture,
            if status == "failed" {
                "failed"
            } else {
                "running"
            },
        );
        fixture.change("UPDATE update_jobs SET mode='auto_save' WHERE id='job-z'");
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let library = Arc::new(Library::create(&fixture.0.join("new")).unwrap());
        successful(&source, &library);
        let job = moved(&library, "job-z").to_string();
        library.switch_over().unwrap();
        let state = state(&fixture, library.clone());
        state.jobs().recover_update_jobs_on_startup().unwrap();
        let expected = if matches!(status, "queued" | "running" | "canceling") {
            "paused"
        } else {
            status
        };
        assert_eq!(
            state.jobs().update_job_status_value(&job).unwrap(),
            expected
        );
        assert!(state.jobs().next_update_job_item(&job).unwrap().is_none());
        assert_eq!(
            state.jobs().update_job_mode_value(&job).unwrap(),
            "auto_save"
        );
        assert!(state.jobs().update_job_watch_saved(&job).unwrap());
        assert!(!state.jobs().update_job_new_works_only(&job).unwrap());
        state
            .jobs()
            .resume_update_job_if_idle(&job, status == "failed")
            .unwrap();
        let next = state.jobs().next_update_job_item(&job).unwrap().unwrap();
        assert_eq!(next.id, 30);
        assert_eq!(next.payload_json, payload());
        library.verify().unwrap();
    }
}

#[test]
#[ignore = "旧棚から移した未処理ジョブを開発版で確認するための棚を残す"]
fn native_imported_queue_fixture() {
    let fixture = Fixture::new();
    prepare(&fixture, "running");
    add_pending(&fixture, "running");
    fixture.change("UPDATE update_jobs SET totals=4,processed=3,candidate_count=3,saved_count=1,error_count=0 WHERE id='job-z'");
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    successful(&source, &library);
    let job = moved(&library, "job-z");
    library.verify().unwrap();
    library.switch_over().unwrap();
    println!("IMPORTED_QUEUE_ROOT={}", fixture.0.display());
    println!("IMPORTED_QUEUE_JOB={job}");
    std::mem::forget(fixture);
}
