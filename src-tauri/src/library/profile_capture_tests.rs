use super::*;
use crate::library::{AcquisitionState, ResourceReference, ResourceScope};
use serde_json::json;
use std::{fs, path::PathBuf};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(
            std::env::temp_dir().join(format!("piep-profile-refresh-test-{}", LibraryId::now_v7())),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn data(name: &str) -> Value {
    json!({"source":"pixiv","sourceKey":"77","displayName":name,"description":"紹介","links":[],"iconUrl":null,"coverUrl":null})
}
const RAW: &[u8] = b" \n{\"future\":9007199254740993,\"decimal\":0.07221960411257732}\n";
fn begin(library: &Library) -> LibraryId {
    let id = LibraryId::now_v7();
    library
        .begin_acquisition(id, "pixiv", EntityKind::Person, "77", PROFILE_REFRESH_ROUTE)
        .unwrap();
    library
        .record_acquisition_payload(id, "api", &CaptureSource::Bytes(RAW.to_vec()))
        .unwrap();
    id
}
fn save(
    library: &Library,
    id: LibraryId,
    value: &Value,
    time: i64,
    images: Vec<StagedAsset>,
    complete: bool,
) -> Result<CaptureResult> {
    library.save_profile(ProfileSave {
        request: id,
        provider: "pixiv",
        key: "77",
        kind: EntityKind::Person,
        data: value,
        checked_at: time,
        images,
        media_complete: complete,
    })
}
#[test]
fn identical_refreshes_keep_one_snapshot_but_advance_freshness_and_preserve_exact_responses_after_backup(
) {
    let fixture = Fixture::new();
    let library = Library::create(&fixture.0.join("shelf")).unwrap();
    let time = chrono::Utc::now().timestamp_millis();
    let first = save(&library, begin(&library), &data("作者"), time, vec![], true).unwrap();
    let request = begin(&library);
    let next = save(&library, request, &data("作者"), time + 1, vec![], true).unwrap();
    assert_eq!(next.snapshot_id, first.snapshot_id);
    assert!(!next.created_revision);
    let state = library.source_entity_state(next.entity_id).unwrap();
    assert_eq!(state.last_checked_at, Some(time + 1));
    assert_eq!(state.last_fetched_at, Some(time + 1));
    assert_eq!(state.last_deep_checked_at, None);
    assert_eq!(
        library.acquisition_status(request).unwrap().state,
        AcquisitionState::Complete
    );
    assert_eq!(
        save(&library, request, &data("作者"), time + 1, vec![], true)
            .unwrap()
            .snapshot_id,
        next.snapshot_id
    );
    library.backup(&fixture.0.join("backup")).unwrap();
    drop(library);
    let copy = Library::open(&fixture.0.join("backup")).unwrap();
    assert_eq!(copy.source_entity_state(next.entity_id).unwrap(), state);
    let payload = copy.acquisition_status(request).unwrap().payloads.remove(0);
    let mut bytes = Vec::new();
    copy.objects
        .export(&copy.file(payload.file).unwrap(), &mut bytes)
        .unwrap();
    assert_eq!(bytes, RAW);
    copy.verify().unwrap();
}
#[test]
fn a_failed_freshness_write_rolls_back_the_revision_and_current_pointer_and_can_be_retried() {
    let fixture = Fixture::new();
    let library = Library::create(&fixture.0.join("shelf")).unwrap();
    let time = chrono::Utc::now().timestamp_millis();
    let first = save(
        &library,
        begin(&library),
        &data("前の名前"),
        time,
        vec![],
        true,
    )
    .unwrap();
    library.conn.lock().unwrap().execute_batch("CREATE TRIGGER reject_profile_check BEFORE UPDATE ON source_entity_state BEGIN SELECT RAISE(ABORT,'injected'); END").unwrap();
    let request = begin(&library);
    assert!(save(
        &library,
        request,
        &data("新しい名前"),
        time + 1,
        vec![],
        true
    )
    .is_err());
    assert_eq!(
        library.current_snapshot(first.entity_id).unwrap(),
        first.snapshot_id
    );
    assert_eq!(
        library
            .source_entity_state(first.entity_id)
            .unwrap()
            .last_checked_at,
        Some(time)
    );
    assert_eq!(
        library.acquisition_status(request).unwrap().state,
        AcquisitionState::Cancelled
    );
    assert_eq!(library.snapshots(first.entity_id).unwrap().len(), 1);
    library
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER reject_profile_check")
        .unwrap();
    assert!(
        save(
            &library,
            begin(&library),
            &data("新しい名前"),
            time + 1,
            vec![],
            true
        )
        .unwrap()
        .created_revision
    );
    library.verify().unwrap();
}
#[test]
fn an_old_response_cannot_replace_a_newer_profile_or_advance_its_freshness() {
    let fixture = Fixture::new();
    let library = Library::create(&fixture.0.join("shelf")).unwrap();
    let old = begin(&library);
    let time = chrono::Utc::now().timestamp_millis();
    let next = save(
        &library,
        begin(&library),
        &data("新しい名前"),
        time,
        vec![],
        true,
    )
    .unwrap();
    assert!(matches!(
        save(&library, old, &data("古い名前"), time + 1, vec![], true),
        Err(Error::Superseded)
    ));
    assert_eq!(
        library.current_snapshot(next.entity_id).unwrap(),
        next.snapshot_id
    );
    assert_eq!(
        library
            .source_entity_state(next.entity_id)
            .unwrap()
            .last_checked_at,
        Some(time)
    );
    library.verify().unwrap();
}
#[test]
fn unknown_fields_and_failed_image_downloads_preserve_known_values_but_known_removal_is_applied() {
    let fixture = Fixture::new();
    let library = Library::create(&fixture.0.join("shelf")).unwrap();
    let value = json!({"source":"pixiv","sourceKey":"77","displayName":"作者","description":"以前の紹介","iconUrl":"https://example.invalid/icon.png","coverUrl":null});
    let path = fixture.0.join("icon.png");
    fs::write(&path, b"icon").unwrap();
    let image = StagedAsset {
        role: "icon".into(),
        name: "icon.png".into(),
        url: Some("https://example.invalid/icon.png".into()),
        path,
        mime_type: Some("image/png".into()),
    };
    let time = chrono::Utc::now().timestamp_millis();
    let first = save(&library, begin(&library), &value, time, vec![image], true).unwrap();
    let mut partial = value.clone();
    partial["description"] = Value::Null;
    let second = save(&library, begin(&library), &partial, time + 1, vec![], false).unwrap();
    assert_eq!(second.snapshot_id, first.snapshot_id);
    let mut bytes = Vec::new();
    library
        .read_resource(
            second.entity_id,
            ResourceScope::Snapshot(second.snapshot_id),
            &ResourceReference::Media {
                slot: "icon".into(),
            },
            None,
            &mut bytes,
        )
        .unwrap();
    assert_eq!(bytes, b"icon");
    let removed = save(
        &library,
        begin(&library),
        &data("作者"),
        time + 2,
        vec![],
        true,
    )
    .unwrap();
    assert!(removed.created_revision);
    assert!(matches!(
        library.media_file(removed.entity_id, removed.snapshot_id, "icon"),
        Err(Error::ResourceUnavailable)
    ));
    let mut old_bytes = Vec::new();
    library
        .read_resource(
            first.entity_id,
            ResourceScope::Snapshot(first.snapshot_id),
            &ResourceReference::Media {
                slot: "icon".into(),
            },
            None,
            &mut old_bytes,
        )
        .unwrap();
    assert_eq!(old_bytes, b"icon");
    library.verify().unwrap();
}
#[test]
fn success_requires_an_original_response_and_matching_identity() {
    let fixture = Fixture::new();
    let library = Library::create(&fixture.0.join("shelf")).unwrap();
    let time = chrono::Utc::now().timestamp_millis();
    let request = LibraryId::now_v7();
    library
        .begin_acquisition(
            request,
            "pixiv",
            EntityKind::Person,
            "77",
            PROFILE_REFRESH_ROUTE,
        )
        .unwrap();
    assert!(save(&library, request, &data("作者"), time, vec![], true).is_err());
    let request = begin(&library);
    let mut wrong = data("作者");
    wrong["sourceKey"] = json!("88");
    assert!(save(&library, request, &wrong, time, vec![], true).is_err());
    assert!(library
        .source_entity_state(library.acquisition_status(request).unwrap().ticket.entity)
        .unwrap()
        .last_checked_at
        .is_none());
    library.verify().unwrap();
}

#[test]
fn saved_work_uuids_find_each_incomplete_profile_once_and_skip_fetched_or_removed_targets() {
    let fixture = Fixture::new();
    let library = Library::create(&fixture.0.join("shelf")).unwrap();
    let mut works = Vec::new();
    for key in ["1", "2"] {
        let result=library.save_work(crate::library::WorkSave{request:LibraryId::now_v7(),provider:"pixiv",key,route:"test",data:&json!({"detail":{"title":"作品","user":{"id":77,"name":"作者"},"series_id":88,"series_title":"連作"},"text":"本文"}),assets:vec![]}).unwrap();
        works.push(result.entity_id);
    }
    let expected = vec![
        (EntityKind::Person, "pixiv".into(), "77".into()),
        (EntityKind::Series, "pixiv".into(), "88".into()),
    ];
    assert_eq!(
        library
            .missing_profiles_for_works(&[works[0], works[1], works[0]])
            .unwrap(),
        expected
    );
    save(
        &library,
        begin(&library),
        &data("作者"),
        chrono::Utc::now().timestamp_millis(),
        vec![],
        true,
    )
    .unwrap();
    assert_eq!(
        library.missing_profiles_for_works(&works).unwrap(),
        vec![(EntityKind::Series, "pixiv".into(), "88".into())]
    );
    for work in &works {
        library
            .remove_work(
                LibraryId::now_v7(),
                *work,
                library.library_item_generation(*work).unwrap().unwrap(),
            )
            .unwrap();
    }
    assert!(library
        .missing_profiles_for_works(&works)
        .unwrap()
        .is_empty());
    let post=library.save_work(crate::library::WorkSave{request:LibraryId::now_v7(),provider:"fanbox",key:"post",route:"test",data:&json!({"title":"投稿","creatorId":"creator-name","user":{"id":123,"name":"作者"},"type":"text","body":{"text":"本文"}}),assets:vec![]}).unwrap();
    assert_eq!(
        library
            .missing_profiles_for_works(&[post.entity_id])
            .unwrap(),
        vec![(EntityKind::Person, "fanbox".into(), "creator-name".into())]
    );
    library.verify().unwrap();
}
