use super::*;
use crate::library::WorkSave;
use serde_json::json;

#[tokio::test]
async fn resumed_candidate_records_the_committed_save() {
    let (_temp, state, library) = shelf::tests::fixture();
    let job = LibraryId::now_v7().to_string();
    let input = UpdateJobItemInput {
        item_type: "candidate".into(),
        source: Some("fanbox".into()),
        source_id: Some("42".into()),
        target_type: None,
        title: "投稿".into(),
        payload_json: json!({"kind": "save"}).to_string(),
        status: "queued".into(),
    };
    let jobs = state.jobs();
    jobs.create_update_job(&job, &shelf::tests::request("save"), &[input])
        .unwrap();
    let item = jobs.next_update_job_item(&job).unwrap().unwrap();
    let request = LibraryId::now_v7();
    jobs.set_update_job_item_payload(
        item.id,
        &json!({"kind": "save", "acquisitionRequest": request}).to_string(),
    )
    .unwrap();
    let saved = library
        .save_readable_work(
            WorkSave {
                request,
                provider: "fanbox",
                key: "42",
                route: "save",
                data: &shelf::tests::data("fanbox", "本文", "77"),
                assets: vec![],
            },
            None,
        )
        .unwrap();
    assert_eq!(jobs.update_job_snapshot(&job).unwrap().saved_count, 0);

    jobs.recover_update_jobs_on_startup().unwrap();
    jobs.resume_update_job_if_idle(&job, false).unwrap();
    let resumed = jobs.next_update_job_item(&job).unwrap().unwrap();
    assert_eq!(resumed.id, item.id);
    let outcome = process_candidate_item(&state, &resumed, &snapshot_credentials_missing())
        .await
        .unwrap();
    let (status, work) = match outcome {
        ItemOutcome::Saved(work, message) => {
            assert_eq!(message, "新作を保存: 投稿");
            ("saved", Some(work))
        }
        ItemOutcome::Skipped(_) => ("skipped", None),
        _ => panic!("保存結果かスキップを返すはず"),
    };
    jobs.complete_saved_item(resumed.id, status, None, work)
        .unwrap();
    assert_eq!(status, "saved");
    assert_eq!(work, Some(saved.entity_id));
    assert_eq!(jobs.update_job_snapshot(&job).unwrap().saved_count, 1);
}

#[tokio::test]
async fn resumed_revision_uses_the_attempt_that_the_job_committed() {
    let (_temp, state, library) = shelf::tests::fixture();
    shelf::tests::save(
        &library,
        "fanbox",
        "42",
        &shelf::tests::data("fanbox", "本文", "77"),
        false,
    );
    let stale = LibraryId::now_v7();
    library
        .begin_acquisition(stale, "fanbox", EntityKind::Work, "42", "save")
        .unwrap();
    let job = LibraryId::now_v7().to_string();
    let input = UpdateJobItemInput {
        item_type: "candidate".into(),
        source: Some("fanbox".into()),
        source_id: Some("42".into()),
        target_type: Some("work".into()),
        title: "投稿".into(),
        payload_json: json!({"kind": "revision", "acquisitionRequest": stale}).to_string(),
        status: "queued".into(),
    };
    let jobs = state.jobs();
    jobs.create_update_job(&job, &shelf::tests::request("all"), &[input])
        .unwrap();
    let item = jobs.next_update_job_item(&job).unwrap().unwrap();
    let credentials = snapshot_credentials_missing();
    assert!(matches!(
        process_candidate_item(&state, &item, &credentials)
            .await
            .unwrap(),
        ItemOutcome::AuthRequired(_)
    ));

    let mut data = shelf::tests::data("fanbox", "改稿本文", "77");
    data["id"] = json!("42");
    let saved = super::super::downloader::download_and_save(
        &state,
        &item,
        data,
        "fanbox".into(),
        "42".into(),
        None,
        None,
    )
    .await
    .unwrap();
    assert!(saved.revised);
    let later = shelf::tests::save(
        &library,
        "fanbox",
        "42",
        &shelf::tests::data("fanbox", "さらに改稿した本文", "77"),
        false,
    );
    assert_eq!(later.version_number, 3);

    jobs.recover_update_jobs_on_startup().unwrap();
    jobs.resume_update_job_if_idle(&job, false).unwrap();
    let resumed = jobs.next_update_job_item(&job).unwrap().unwrap();
    let payload: Value = serde_json::from_str(&resumed.payload_json).unwrap();
    let request: LibraryId =
        serde_json::from_value(payload[JOB_ACQUISITION_REQUEST].clone()).unwrap();
    assert_ne!(request, stale);
    assert_eq!(
        library.acquisition_status(request).unwrap().state,
        AcquisitionState::Complete
    );
    match process_candidate_item(&state, &resumed, &credentials)
        .await
        .unwrap()
    {
        ItemOutcome::Saved(work, message) => {
            assert_eq!(work, saved.work);
            assert_eq!(message, "改稿を保存: 投稿（+2字, v2）");
        }
        _ => panic!("ジョブ自身が確定した改稿を返すはず"),
    }
}

#[test]
fn clearing_finished_jobs_keeps_paused_jobs() {
    let root = std::env::temp_dir().join(format!("piep-history-adapter-{}", LibraryId::now_v7()));
    std::fs::create_dir_all(&root).unwrap();
    {
        let request = StartUpdateJobRequest {
            scope: "all".into(),
            mode: "check_only".into(),
            work_ids: None,
            target_ids: None,
            watch_saved: None,
            new_works_only: None,
            adhoc_targets: None,
        };
        let library = Arc::new(Library::create(&root.join("shelf")).unwrap());
        let state = AppState::new(Arc::new(super::super::shelf::Shelf::with_test_library(
            library.clone(),
        )));
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
        assert_eq!(current_job_summaries(&state).unwrap().len(), 2);
        assert_eq!(state.jobs().clear_finished_update_jobs().unwrap(), 1);
        let left = current_job_summaries(&state).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].job_id, paused);
        assert_eq!(state.jobs().clear_finished_update_jobs().unwrap(), 0);
        library.verify().unwrap();
    }
    assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
    std::fs::remove_dir_all(root).unwrap();
}
