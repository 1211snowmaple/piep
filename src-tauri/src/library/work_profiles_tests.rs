use super::*;
use crate::library::{AcquisitionState, Fingerprint, ResourceReference, ResourceScope, WorkSave};
use std::{fs, path::PathBuf};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("piep-work-profiles-{}", LibraryId::now_v7())))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn work() -> Value {
    json!({"detail":{"title":"作品","user":{"id":77,"name":"作者","comment":"自己紹介","profile_image_url":"https://example.invalid/icon.png"},"series_id":88,"series_title":"連作","cover_url":"https://example.invalid/work-cover.png"},"text":"本文"})
}
fn save_work(library: &Library, data: &Value) {
    library
        .save_work(WorkSave {
            request: LibraryId::now_v7(),
            provider: "pixiv",
            key: "1",
            route: "test",
            data,
            assets: vec![],
        })
        .unwrap();
}
fn finish(library: &Library, seed: &WorkProfileSeed, data: &Value) -> CaptureResult {
    let ticket = library
        .begin_work_profile_seed(LibraryId::now_v7(), seed, data)
        .unwrap()
        .unwrap();
    library
        .complete_work_profile_seed(&ticket, seed, vec![])
        .unwrap()
}
fn generation(library: &Library) -> i64 {
    library
        .reader()
        .unwrap()
        .query_row("SELECT generation FROM library_meta", [], |r| r.get(0))
        .unwrap()
}
fn detailed(library: &Library, seed: &WorkProfileSeed) -> CaptureResult {
    let request = LibraryId::now_v7();
    library
        .begin_acquisition(
            request,
            &seed.provider,
            seed.kind,
            &seed.key,
            "direct-profile-test",
        )
        .unwrap();
    let raw = json!({"displayName":"直接確認した名前","description":"詳しい自己紹介"});
    let fields = profile_fields(&raw).unwrap();
    library
        .complete_acquisition(
            request,
            Capture {
                request_id: request,
                acquired_at: None,
                provider: seed.provider.clone(),
                external_key: seed.key.clone(),
                kind: seed.kind,
                route: "direct-profile-test".into(),
                raw: CaptureSource::Bytes(serde_json::to_vec(&raw).unwrap()),
                metadata: fields.metadata,
                field_states: fields.states,
                body: None,
                media: vec![],
                attachments: vec![],
            },
        )
        .unwrap()
}

#[test]
fn seeds_use_creator_identity_and_only_names_the_response_actually_contains() {
    let pixiv = work_profile_seeds("pixiv", &work()).unwrap();
    assert_eq!(pixiv.len(), 2);
    assert_eq!(pixiv[0].key, "77");
    assert_eq!(pixiv[1].key, "88");
    assert_eq!(pixiv[1].data["coverUrl"], Value::Null);
    let fanbox = work_profile_seeds(
        "fanbox",
        &json!({"creatorId":"creator-name","user":{"userId":"123","name":"作者"}}),
    )
    .unwrap();
    assert_eq!(fanbox.len(), 1);
    assert_eq!(fanbox[0].key, "creator-name");
    assert_eq!(
        fanbox[0].data["links"],
        json!(["https://www.fanbox.cc/@creator-name"])
    );
    assert!(work_profile_seeds(
        "pixiv",
        &json!({"detail":{"user":{"id":77},"series_id":88}})
    )
    .unwrap()
    .is_empty());
    assert!(work_profile_seeds("future", &work()).is_err());
}

#[test]
fn a_work_seed_keeps_its_evidence_and_images_without_claiming_a_profile_refresh() {
    let fixture = Fixture::new();
    let library = Library::create(&fixture.0.join("shelf")).unwrap();
    let data = work();
    save_work(&library, &data);
    let seeds = work_profile_seeds("pixiv", &data).unwrap();
    let ticket = library
        .begin_work_profile_seed(LibraryId::now_v7(), &seeds[0], &data)
        .unwrap()
        .unwrap();
    let image = fixture.0.join("icon.png");
    fs::write(&image, b"icon-bytes").unwrap();
    let result = library
        .complete_work_profile_seed(
            &ticket,
            &seeds[0],
            vec![StagedAsset {
                role: "icon".into(),
                name: "icon.png".into(),
                path: image.clone(),
                url: Some("https://example.invalid/icon.png".into()),
                mime_type: Some("image/png".into()),
            }],
        )
        .unwrap();
    let series = finish(&library, &seeds[1], &data);
    let info = library.acquisition_status(ticket.id).unwrap();
    assert_eq!(info.state, AcquisitionState::Complete);
    assert_eq!(info.payloads.len(), 1);
    assert_eq!(info.payloads[0].slot, "source-work");
    assert_eq!(
        info.payloads[0].file,
        Fingerprint::of(&serde_json::to_vec_pretty(&data).unwrap())
    );
    let state = library.source_entity_state(result.entity_id).unwrap();
    assert_eq!(state.last_checked_at, None);
    assert_eq!(state.last_fetched_at, None);
    let conn = library.reader().unwrap();
    let (metadata,states,body):(String,String,Option<Vec<u8>>)=conn.query_row("SELECT metadata_json,field_states_json,body_file_id FROM revision_snapshots WHERE id=?1",[result.snapshot_id.as_bytes().as_slice()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&metadata).unwrap()["display_name"],
        "作者"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&states).unwrap()["is_concluded"],
        "unknown"
    );
    assert!(body.is_none());
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM snapshot_media WHERE snapshot_id=?1",
            [series.snapshot_id.as_bytes().as_slice()],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    drop(conn);
    library.backup(&fixture.0.join("backup")).unwrap();
    fs::remove_file(image).unwrap();
    drop(library);
    let copy = Library::open(&fixture.0.join("backup")).unwrap();
    let mut bytes = Vec::new();
    copy.read_resource(
        result.entity_id,
        ResourceScope::Snapshot(result.snapshot_id),
        &ResourceReference::Media {
            slot: "icon".into(),
        },
        None,
        &mut bytes,
    )
    .unwrap();
    assert_eq!(bytes, b"icon-bytes");
    copy.verify().unwrap();
}

#[test]
fn a_second_work_never_replaces_an_existing_profile_or_starts_another_acquisition() {
    let fixture = Fixture::new();
    let library = Library::create(&fixture.0.join("shelf")).unwrap();
    let data = work();
    let seed = work_profile_seeds("pixiv", &data).unwrap().remove(0);
    let first = finish(&library, &seed, &data);
    let before_generation = generation(&library);
    assert!(library
        .begin_work_profile_seed(LibraryId::now_v7(), &seed, &data)
        .unwrap()
        .is_none());
    assert_eq!(generation(&library), before_generation);
    let detail = detailed(&library, &seed);
    assert_ne!(detail.snapshot_id, first.snapshot_id);
    assert!(library
        .begin_work_profile_seed(LibraryId::now_v7(), &seed, &data)
        .unwrap()
        .is_none());
    assert_eq!(
        library.current_snapshot(first.entity_id).unwrap(),
        detail.snapshot_id
    );
    assert_eq!(
        library
            .reader()
            .unwrap()
            .query_row("SELECT count(*) FROM acquisition_attempts", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    library.verify().unwrap();
}

#[test]
fn a_profile_fetched_while_the_icon_is_in_flight_wins_over_the_seed() {
    let fixture = Fixture::new();
    let library = Library::create(&fixture.0.join("shelf")).unwrap();
    let data = work();
    let seed = work_profile_seeds("pixiv", &data).unwrap().remove(0);
    let ticket = library
        .begin_work_profile_seed(LibraryId::now_v7(), &seed, &data)
        .unwrap()
        .unwrap();
    let detail = detailed(&library, &seed);
    assert!(matches!(
        library.complete_work_profile_seed(&ticket, &seed, vec![]),
        Err(Error::Superseded)
    ));
    assert_eq!(
        library.current_snapshot(detail.entity_id).unwrap(),
        detail.snapshot_id
    );
    assert_eq!(
        library.acquisition_status(ticket.id).unwrap().state,
        AcquisitionState::Superseded
    );
    library.verify().unwrap();
}

#[test]
fn a_pending_direct_profile_is_not_superseded_by_an_incidental_work_seed() {
    let fixture = Fixture::new();
    let library = Library::create(&fixture.0.join("shelf")).unwrap();
    let data = work();
    let seed = work_profile_seeds("pixiv", &data).unwrap().remove(0);
    let request = LibraryId::now_v7();
    let before = library
        .begin_acquisition(
            request,
            &seed.provider,
            seed.kind,
            &seed.key,
            "direct-profile-test",
        )
        .unwrap();
    let before_generation = generation(&library);
    assert!(library
        .begin_work_profile_seed(LibraryId::now_v7(), &seed, &data)
        .unwrap()
        .is_none());
    let after = library.acquisition_status(request).unwrap();
    assert_eq!(after.state, AcquisitionState::Pending);
    assert_eq!(after.ticket, before.ticket);
    assert_eq!(generation(&library), before_generation);
    library.verify().unwrap();
}

#[test]
fn an_interrupted_seed_can_start_again_after_reopening_the_library() {
    let fixture = Fixture::new();
    let library = Library::create(&fixture.0.join("shelf")).unwrap();
    let data = work();
    let seed = work_profile_seeds("pixiv", &data).unwrap().remove(0);
    let first = library
        .begin_work_profile_seed(LibraryId::now_v7(), &seed, &data)
        .unwrap()
        .unwrap();
    drop(library);
    let library = Library::open(&fixture.0.join("shelf")).unwrap();
    let next = library
        .begin_work_profile_seed(LibraryId::now_v7(), &seed, &data)
        .unwrap()
        .unwrap();
    assert!(next.epoch > first.epoch);
    let result = library
        .complete_work_profile_seed(&next, &seed, vec![])
        .unwrap();
    assert!(matches!(
        library.complete_work_profile_seed(&first, &seed, vec![]),
        Err(Error::Superseded)
    ));
    assert_eq!(
        library.current_snapshot(first.entity).unwrap(),
        result.snapshot_id
    );
    library.verify().unwrap();
}

#[test]
fn a_failed_seed_does_not_undo_the_saved_work_and_can_be_tried_again() {
    let fixture = Fixture::new();
    let library = Library::create(&fixture.0.join("shelf")).unwrap();
    let data = work();
    save_work(&library, &data);
    let seed = work_profile_seeds("pixiv", &data).unwrap().remove(0);
    let ticket = library
        .begin_work_profile_seed(LibraryId::now_v7(), &seed, &data)
        .unwrap()
        .unwrap();
    assert!(library
        .complete_work_profile_seed(
            &ticket,
            &seed,
            vec![StagedAsset {
                role: "icon".into(),
                name: "icon.png".into(),
                path: fixture.0.join("missing.png"),
                url: Some("https://example.invalid/icon.png".into()),
                mime_type: None
            }]
        )
        .is_err());
    assert_eq!(
        library.acquisition_status(ticket.id).unwrap().state,
        AcquisitionState::Cancelled
    );
    assert_eq!(library.works(5).unwrap().len(), 1);
    let result = finish(&library, &seed, &data);
    assert_eq!(result.entity_id, ticket.entity);
    library.verify().unwrap();
}
