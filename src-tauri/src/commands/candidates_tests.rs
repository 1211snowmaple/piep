use super::*;
struct Temp(std::path::PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (Temp, AppState, Arc<Library>) {
    let root = std::env::temp_dir().join(format!("piep-candidate-adapter-{}", LibraryId::now_v7()));
    std::fs::create_dir_all(&root).unwrap();
    let library = Arc::new(Library::create(&root.join("shelf")).unwrap());
    library.switch_over().unwrap();
    let state = AppState::new(Arc::new(super::super::shelf::Shelf::with_test_library(
        library.clone(),
    )));
    (Temp(root), state, library)
}
fn candidate(key: &str, kind: &str) -> UpdateCandidateInput {
    UpdateCandidateInput{source:"pixiv".into(),source_id:key.into(),kind:"revision".into(),title:"保存した作品の改稿".into(),payload_json:serde_json::json!({"kind":"revision","holdKind":kind,"holdReason":"試験用の保留","originalData":{"user":{"name":"試験の作者"},"text":"本文を画面へ運ばない"}}).to_string(),target_type:Some("work".into())}
}
fn save(library: &Library, key: &str) {
    library.save_work(crate::library::WorkSave{request:LibraryId::now_v7(),provider:"pixiv",key,route:"test",data:&serde_json::json!({"detail":{"title":"保存した作品","user":{"id":77,"name":"試験の作者"}},"text":"本文"}),assets:vec![]}).unwrap();
}

#[test]
fn new_shelf_candidates_drive_display_blocking_and_user_choices_without_old_candidate_rows() {
    let (_temp, state, library) = fixture();
    save(&library, "11");
    let ledger = state.candidates();
    ledger.hold(&candidate("11", "incomplete")).unwrap();
    assert!(ledger
        .candidate_acquisition_blocked("pixiv", "11", false)
        .unwrap());
    assert!(!ledger
        .candidate_acquisition_blocked("pixiv", "11", true)
        .unwrap());
    assert_eq!(ledger.count_saved_deferred_update_candidates().unwrap(), 1);
    let rows = ledger.list_deferred_update_candidate_summaries(-1).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].work_id.is_some());
    assert_eq!(rows[0].download_id, None);
    assert!(rows[0].metadata.saved_at.is_some());
    assert!(!rows[0].payload_json.contains("本文を画面へ運ばない"));
    ledger
        .set_update_candidate_status("pixiv", "11", "dismissed")
        .unwrap();
    ledger
        .upsert_update_candidate(&candidate("11", "incomplete"))
        .unwrap();
    assert_eq!(ledger.count_dismissed_update_candidates().unwrap(), 1);
    assert!(ledger
        .candidate_acquisition_blocked("pixiv", "11", true)
        .unwrap());
    assert_eq!(ledger.restore_dismissed_update_candidates().unwrap(), 1);
    assert_eq!(
        ledger
            .update_candidate_status("pixiv", "11")
            .unwrap()
            .as_deref(),
        Some("pending")
    );
    let page = ledger
        .list_pending_update_candidates_after(None, 1)
        .unwrap();
    assert_eq!(page[0].source_id, "11");
    // 表示用の写しができた候補では、原入力が読めなくても一覧を表示できる。
    ledger.hold(&candidate("11", "incomplete")).unwrap();
    std::fs::rename(
        library.root().join("objects"),
        library.root().join("objects-unavailable"),
    )
    .unwrap();
    assert_eq!(
        ledger
            .list_deferred_update_candidate_summaries(-1)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn unavailable_holds_and_execution_rows_use_the_native_shelf_without_changing_the_old_queue() {
    let (_temp, state, library) = fixture();
    save(&library, "11");
    let job = LibraryId::now_v7().to_string();
    state
        .jobs()
        .create_update_job(
            &job,
            &crate::database::StartUpdateJobRequest {
                scope: "all".into(),
                mode: "check_only".into(),
                work_ids: None,
                target_ids: None,
                watch_saved: None,
                new_works_only: None,
                adhoc_targets: None,
            },
            &[],
        )
        .unwrap();
    let item = UpdateJobItemInput {
        item_type: "work".into(),
        source: Some("pixiv".into()),
        source_id: Some("11".into()),
        target_type: Some("work".into()),
        title: "作品".into(),
        payload_json: "{}".into(),
        status: "held".into(),
    };
    let ledger = state.candidates();
    let reason = "[見つからない] 公開元で投稿が見つかりません";
    assert!(ledger
        .insert_update_job_unavailable_item(
            &job,
            &item,
            &serde_json::json!({"title":"元の題"}),
            reason
        )
        .unwrap());
    assert!(!ledger
        .insert_update_job_unavailable_item(&job, &item, &serde_json::json!({}), reason)
        .unwrap());
    assert_eq!(ledger.count_saved_deferred_update_candidates().unwrap(), 1);
    state
        .jobs()
        .set_update_job_status(&job, "completed", None)
        .unwrap();
    state.jobs().clear_update_job(&job).unwrap();
    assert!(state.jobs().list_update_jobs().unwrap().is_empty());
    assert_eq!(ledger.count_saved_deferred_update_candidates().unwrap(), 1);
    ledger
        .clear_resolved_unavailable_candidates("pixiv", &["11".into()])
        .unwrap();
    assert_eq!(ledger.count_saved_deferred_update_candidates().unwrap(), 0);
    library.verify().unwrap();
}

#[test]
fn stale_recheck_cannot_reverse_a_choice_made_during_the_request() {
    let (_temp, state, library) = fixture();
    let ledger = state.candidates();
    let held = candidate("11", "restricted");
    ledger.hold(&held).unwrap();
    let expected = ledger.generation("pixiv", "11").unwrap();
    ledger
        .set_update_candidate_status("pixiv", "11", "dismissed")
        .unwrap();
    let available = UpdateCandidateInput {
        payload_json: "{\"kind\":\"new\",\"originalData\":{\"isRestricted\":false}}".into(),
        ..held
    };
    assert!(ledger.recheck_available(&available, expected).is_err());
    assert_eq!(
        ledger
            .update_candidate_status("pixiv", "11")
            .unwrap()
            .as_deref(),
        Some("dismissed")
    );
    ledger
        .recheck_available(&available, ledger.generation("pixiv", "11").unwrap())
        .unwrap();
    assert_eq!(
        ledger
            .update_candidate_status("pixiv", "11")
            .unwrap()
            .as_deref(),
        Some("pending")
    );
    library.verify().unwrap();
}

#[test]
#[ignore]
fn native_fixture() {
    let root = std::env::temp_dir().join(format!("piep-candidate-native-{}", LibraryId::now_v7()));
    let library = Library::create(&root).unwrap();
    library.switch_over().unwrap();
    save(&library, "11");
    for (key, kind) in [("11", "incomplete"), ("22", "restricted")] {
        let value = candidate(key, kind);
        library
            .record_discovery(
                LibraryId::now_v7(),
                DiscoveryInput {
                    provider: &value.source,
                    key: &value.source_id,
                    kind: &value.kind,
                    title: &value.title,
                    payload: &value.payload_json,
                    target_type: value.target_type.as_deref(),
                },
                true,
            )
            .unwrap();
    }
    println!("CANDIDATE_NATIVE_FIXTURE={}", root.display());
}
