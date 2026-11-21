use super::*;
use crate::library::{AcquisitionState, StagedAsset, WorkSave};
use serde_json::json;
use std::{fs, path::PathBuf};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("piep-asset-state-{}", LibraryId::now_v7())))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn data() -> serde_json::Value {
    json!({"id":"1","title":"投稿","creatorId":"creator","type":"file","body":{"files":[{"id":"a","name":"a","extension":"txt","url":"https://example.invalid/a"},{"id":"b","name":"b","extension":"txt","url":"https://example.invalid/b"}]}})
}
fn observation(complete: bool) -> AssetRepairObservation {
    AssetRepairObservation {
        expected_urls: fanbox_expected_urls(&data()),
        complete,
    }
}
fn assets(root: &std::path::Path, include_b: bool) -> Vec<StagedAsset> {
    fs::create_dir_all(root).unwrap();
    ["a", "b"]
        .into_iter()
        .filter(|name| *name == "a" || include_b)
        .map(|name| {
            let path = root.join(format!("{name}.txt"));
            fs::write(&path, name.as_bytes()).unwrap();
            StagedAsset {
                role: "file".into(),
                name: format!("{name}.txt"),
                path,
                url: Some(format!("https://example.invalid/{name}")),
                mime_type: Some("text/plain".into()),
            }
        })
        .collect()
}
fn save(
    library: &Library,
    root: &std::path::Path,
    request: LibraryId,
    complete: bool,
) -> Result<CaptureResult> {
    library.save_work_with_asset_repair(
        WorkSave {
            request,
            provider: "fanbox",
            key: "1",
            route: "save",
            data: &data(),
            assets: assets(root, complete),
        },
        observation(complete),
    )
}
#[test]
fn attempts_are_atomic_idempotent_state_bound_and_survive_backup() {
    let fixture = Fixture::new();
    let library = Library::create(&fixture.0.join("shelf")).unwrap();
    let request = LibraryId::now_v7();
    let first = save(&library, &fixture.0.join("staged"), request, false).unwrap();
    let urls = fanbox_expected_urls(&data());
    for attempt in 1..=MAX_REPAIR_ATTEMPTS {
        if attempt > 1 {
            let result = save(
                &library,
                &fixture.0.join("staged"),
                LibraryId::now_v7(),
                false,
            )
            .unwrap();
            assert_eq!(result.snapshot_id, first.snapshot_id);
        }
        assert_eq!(
            library
                .asset_repair_plan(first.entity_id, first.snapshot_id, &urls)
                .unwrap()
                .attempts,
            attempt
        );
    }
    assert_eq!(
        save(&library, &fixture.0.join("staged"), request, false)
            .unwrap()
            .snapshot_id,
        first.snapshot_id
    );
    assert_eq!(
        library
            .asset_repair_plan(first.entity_id, first.snapshot_id, &urls)
            .unwrap()
            .attempts,
        3
    );
    let mut changed = urls.clone();
    changed.push("https://example.invalid/new".into());
    assert_eq!(
        library
            .asset_repair_plan(first.entity_id, first.snapshot_id, &changed)
            .unwrap()
            .attempts,
        0
    );
    library.backup(&fixture.0.join("backup")).unwrap();
    drop(library);
    let copy = Library::open(&fixture.0.join("backup")).unwrap();
    assert_eq!(
        copy.asset_repair_plan(first.entity_id, first.snapshot_id, &urls)
            .unwrap()
            .attempts,
        3
    );
    let filled = save(&copy, &fixture.0.join("staged"), LibraryId::now_v7(), true).unwrap();
    assert!(!filled.created_revision);
    assert_ne!(filled.snapshot_id, first.snapshot_id);
    let snapshots = copy.snapshots(filled.entity_id).unwrap();
    assert_eq!(snapshots.len(), 2);
    assert!(snapshots.iter().all(|s| s.revision == 1));
    assert_eq!(
        copy.asset_repair_plan(filled.entity_id, filled.snapshot_id, &urls)
            .unwrap()
            .attempts,
        0
    );
    copy.verify().unwrap();
    copy.backup(&fixture.0.join("filled-backup")).unwrap();
    let restored = Library::open(&fixture.0.join("filled-backup")).unwrap();
    assert_eq!(restored.snapshots(filled.entity_id).unwrap().len(), 2);
    assert!(restored
        .snapshots(filled.entity_id)
        .unwrap()
        .iter()
        .all(|s| s.revision == 1));
    restored.verify().unwrap();
}
#[test]
fn replacing_or_removing_files_and_changing_declared_content_still_creates_author_revisions() {
    let fixture = Fixture::new();
    let library = Library::create(&fixture.0.join("shelf")).unwrap();
    let first = save(
        &library,
        &fixture.0.join("staged"),
        LibraryId::now_v7(),
        true,
    )
    .unwrap();
    let changed_assets = assets(&fixture.0.join("staged"), true);
    fs::write(&changed_assets[1].path, b"replaced at the same URL").unwrap();
    let replacement = library
        .save_work_with_asset_repair(
            WorkSave {
                request: LibraryId::now_v7(),
                provider: "fanbox",
                key: "1",
                route: "save",
                data: &data(),
                assets: changed_assets,
            },
            observation(true),
        )
        .unwrap();
    assert!(replacement.created_revision);
    let removed = save(
        &library,
        &fixture.0.join("staged"),
        LibraryId::now_v7(),
        false,
    )
    .unwrap();
    assert!(removed.created_revision);
    let mut changed = data();
    changed["body"]["files"][1]["name"] = json!("作者が変えた名前");
    let observation = AssetRepairObservation {
        expected_urls: fanbox_expected_urls(&changed),
        complete: true,
    };
    let revised = library
        .save_work_with_asset_repair(
            WorkSave {
                request: LibraryId::now_v7(),
                provider: "fanbox",
                key: "1",
                route: "save",
                data: &changed,
                assets: assets(&fixture.0.join("staged"), true),
            },
            observation,
        )
        .unwrap();
    assert!(revised.created_revision);
    let snapshots = library.snapshots(first.entity_id).unwrap();
    assert_eq!(
        snapshots.iter().map(|s| s.revision).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    library.verify().unwrap();
}

#[test]
fn a_new_source_revision_starts_the_count_again_even_with_the_same_assets() {
    let fixture = Fixture::new();
    let library = Library::create(&fixture.0.join("shelf")).unwrap();
    for _ in 0..MAX_REPAIR_ATTEMPTS {
        save(
            &library,
            &fixture.0.join("staged"),
            LibraryId::now_v7(),
            false,
        )
        .unwrap();
    }
    let mut changed = data();
    changed["title"] = json!("作者が変更した題");
    let result = library
        .save_work_with_asset_repair(
            WorkSave {
                request: LibraryId::now_v7(),
                provider: "fanbox",
                key: "1",
                route: "save",
                data: &changed,
                assets: assets(&fixture.0.join("staged"), false),
            },
            observation(false),
        )
        .unwrap();
    assert!(result.created_revision);
    assert_eq!(
        library
            .asset_repair_plan(
                result.entity_id,
                result.snapshot_id,
                &observation(false).expected_urls
            )
            .unwrap()
            .attempts,
        1
    );
    library.verify().unwrap();
}

#[test]
fn a_rejected_memo_write_rolls_back_the_save_and_does_not_spend_an_attempt() {
    let fixture = Fixture::new();
    let library = Library::create(&fixture.0.join("shelf")).unwrap();
    let first = save(
        &library,
        &fixture.0.join("staged"),
        LibraryId::now_v7(),
        false,
    )
    .unwrap();
    let urls = fanbox_expected_urls(&data());
    library.conn.lock().unwrap().execute_batch("CREATE TRIGGER refuse_asset_attempt BEFORE UPDATE ON asset_repair_state BEGIN SELECT RAISE(ABORT,'injected'); END").unwrap();
    let request = LibraryId::now_v7();
    assert!(save(&library, &fixture.0.join("staged"), request, true).is_err());
    assert_eq!(
        library.acquisition_status(request).unwrap().state,
        AcquisitionState::Cancelled
    );
    assert_eq!(
        library.current_snapshot(first.entity_id).unwrap(),
        first.snapshot_id
    );
    assert_eq!(
        library
            .asset_repair_plan(first.entity_id, first.snapshot_id, &urls)
            .unwrap()
            .attempts,
        1
    );
    library
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER refuse_asset_attempt")
        .unwrap();
    save(
        &library,
        &fixture.0.join("staged"),
        LibraryId::now_v7(),
        false,
    )
    .unwrap();
    library.verify().unwrap();
}
#[test]
fn stale_attempts_do_not_change_the_current_counter_or_newer_result() {
    let fixture = Fixture::new();
    let library = Library::create(&fixture.0.join("shelf")).unwrap();
    let old = LibraryId::now_v7();
    library
        .begin_acquisition(old, "fanbox", crate::library::EntityKind::Work, "1", "save")
        .unwrap();
    let next = save(
        &library,
        &fixture.0.join("staged"),
        LibraryId::now_v7(),
        false,
    )
    .unwrap();
    assert!(matches!(
        save(&library, &fixture.0.join("staged"), old, false),
        Err(Error::Superseded)
    ));
    assert_eq!(
        library
            .asset_repair_plan(
                next.entity_id,
                next.snapshot_id,
                &fanbox_expected_urls(&data())
            )
            .unwrap()
            .attempts,
        1
    );
    library.verify().unwrap();
}
