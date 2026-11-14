use crate::library::{
    AcquisitionState, AcquisitionTicket, Capture, CapturedAttachment, CapturedMedia, EntityKind,
    Error, Fingerprint, Library, LibraryId, ResourceReference, ResourceScope,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("piep-state-check-{}", LibraryId::now_v7())))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn begin(library: &Library) -> AcquisitionTicket {
    library
        .begin_acquisition(
            LibraryId::now_v7(),
            "pixiv",
            EntityKind::Work,
            "one",
            "offline-test",
        )
        .unwrap()
        .ticket
}
fn input(ticket: &AcquisitionTicket, counter: u64, quality: &str, image: &[u8]) -> Capture {
    Capture {
        request_id: ticket.id,
        acquired_at: Some(1234),
        provider: ticket.provider.clone(),
        external_key: ticket.external_key.clone(),
        kind: ticket.kind,
        route: ticket.route.clone(),
        raw: serde_json::to_vec(&json!({"text":"full body","counter":counter}))
            .unwrap()
            .into(),
        metadata: BTreeMap::from([
            ("title".into(), json!("Title")),
            ("caption".into(), json!("Caption")),
            ("author_name".into(), json!("Name")),
            ("author_key".into(), json!("123")),
        ]),
        field_states: BTreeMap::from([
            ("media".into(), "known".into()),
            ("attachments".into(), "known".into()),
        ]),
        body: Some("full body".into()),
        media: vec![CapturedMedia {
            slot: "cover".into(),
            role: "cover".into(),
            source_key: Some("one-image".into()),
            source_url: None,
            quality: quality.into(),
            original_name: format!("{quality}.png"),
            mime_type: "image/png".into(),
            bytes: Some(image.to_vec().into()),
        }],
        attachments: vec![CapturedAttachment {
            slot: "document".into(),
            original_name: "document.bin".into(),
            mime_type: None,
            bytes: b"document bytes".to_vec().into(),
        }],
    }
}
fn count(library: &Library, table: &str) -> i64 {
    assert!(table
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || c == b'_'));
    library
        .conn
        .lock()
        .unwrap()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn checks_keep_new_responses_but_reuse_the_snapshot_and_its_generation_and_selected_original() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = begin(&library);
    let first = library
        .complete_acquisition(first.id, input(&first, 1, "original", b"original image"))
        .unwrap();
    let before = library.works(1).unwrap().remove(0);
    let manifest = library
        .resource_manifest(first.entity_id, ResourceScope::Snapshot(first.snapshot_id))
        .unwrap();
    let next = begin(&library);
    let raw = serde_json::to_vec(&json!({"text":"full body","counter":2})).unwrap();
    let raw_file = library
        .record_acquisition_payload(next.id, "response", &raw.clone().into())
        .unwrap();
    let result = library
        .complete_acquisition(next.id, input(&next, 2, "thumbnail", b"smaller image"))
        .unwrap();
    assert_eq!(result.snapshot_id, first.snapshot_id);
    assert!(!result.created_revision);
    assert_eq!(
        library.works(1).unwrap()[0].logical_size,
        before.logical_size
    );
    assert_eq!(
        library
            .resource_manifest(first.entity_id, ResourceScope::Snapshot(first.snapshot_id))
            .unwrap(),
        manifest
    );
    let current_generation: i64 = library
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT generation FROM entity_current WHERE entity_id=?1",
            [first.entity_id.as_bytes().as_slice()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(current_generation, 1);
    assert_eq!(count(&library, "revision_snapshots"), 1);
    assert_eq!(count(&library, "source_revisions"), 1);
    assert_eq!(count(&library, "snapshot_observations"), 1);
    assert_eq!(count(&library, "acquisition_observations"), 2);
    assert_eq!(count(&library, "media_renditions"), 1);
    assert_eq!(count(&library, "media_observations"), 1);
    assert_eq!(count(&library, "outbox"), 2);
    let info = library.acquisition_status(next.id).unwrap();
    assert_eq!(info.state, AcquisitionState::Complete);
    assert_eq!(info.payloads[0].file, raw_file);
    let mut restored = Vec::new();
    library
        .objects
        .export(&library.file(raw_file).unwrap(), &mut restored)
        .unwrap();
    assert_eq!(restored, raw);
    assert!(library.file(Fingerprint::of(b"smaller image")).is_ok());
    let replay = library
        .complete_acquisition(next.id, input(&next, 2, "thumbnail", b"smaller image"))
        .unwrap();
    assert_eq!(replay.snapshot_id, first.snapshot_id);
    assert_eq!(count(&library, "acquisition_observations"), 2);
    library.verify().unwrap();
}

#[test]
fn partial_checks_preserve_completeness_and_saved_images_so_a_later_explicit_removal_is_a_revision()
{
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = begin(&library);
    let first = library
        .complete_acquisition(first.id, input(&first, 1, "original", b"original image"))
        .unwrap();
    let partial = begin(&library);
    let mut check = input(&partial, 2, "unknown", b"unused");
    check.metadata.remove("caption");
    check.body = Some("partial excerpt".into());
    check.field_states.insert("body".into(), "unknown".into());
    check.field_states.insert("media".into(), "unknown".into());
    check
        .field_states
        .insert("attachments".into(), "unknown".into());
    check.media[0].source_key = None;
    check.media[0].bytes = None;
    check.attachments.clear();
    let checked = library.complete_acquisition(partial.id, check).unwrap();
    assert_eq!(checked.snapshot_id, first.snapshot_id);
    let before = library.snapshots(first.entity_id).unwrap().remove(0);
    assert_eq!(before.field_states["media"], "known");
    assert_eq!(before.field_states["attachments"], "known");
    let mut body = Vec::new();
    library
        .read_body(first.entity_id, first.snapshot_id, &mut body)
        .unwrap();
    assert_eq!(body, b"full body");
    assert_eq!(
        library
            .media_file(first.entity_id, first.snapshot_id, "cover")
            .unwrap()
            .id,
        Fingerprint::of(b"original image")
    );
    let removed = begin(&library);
    let mut check = input(&removed, 3, "unknown", b"unused");
    check.media.clear();
    check.attachments.clear();
    let removed = library.complete_acquisition(removed.id, check).unwrap();
    assert!(removed.created_revision);
    assert_ne!(removed.snapshot_id, first.snapshot_id);
    assert_eq!(library.snapshots(first.entity_id).unwrap().len(), 2);
    assert!(matches!(
        library.resource_info(
            first.entity_id,
            ResourceScope::Snapshot(removed.snapshot_id),
            &ResourceReference::Media {
                slot: "cover".into()
            }
        ),
        Err(Error::ResourceUnavailable)
    ));
    library.verify().unwrap();
}

#[test]
fn quality_and_display_changes_publish_snapshots_without_source_revisions_then_checks_reuse_them() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = begin(&library);
    let first = library
        .complete_acquisition(first.id, input(&first, 1, "thumbnail", b"thumbnail"))
        .unwrap();
    let repair = begin(&library);
    let repair = library
        .complete_acquisition(repair.id, input(&repair, 2, "original", b"original"))
        .unwrap();
    assert!(!repair.created_revision);
    assert_ne!(repair.snapshot_id, first.snapshot_id);
    let rename = begin(&library);
    let mut check = input(&rename, 3, "original", b"original");
    check
        .metadata
        .insert("author_name".into(), json!("New name"));
    let renamed = library.complete_acquisition(rename.id, check).unwrap();
    assert!(!renamed.created_revision);
    assert_ne!(renamed.snapshot_id, repair.snapshot_id);
    let repeat = begin(&library);
    let mut check = input(&repeat, 4, "original", b"original");
    check
        .metadata
        .insert("author_name".into(), json!("New name"));
    let repeated = library.complete_acquisition(repeat.id, check).unwrap();
    assert_eq!(repeated.snapshot_id, renamed.snapshot_id);
    assert_eq!(count(&library, "revision_snapshots"), 3);
    assert_eq!(count(&library, "source_revisions"), 1);
    assert_eq!(
        library.snapshots(first.entity_id).unwrap()[0].metadata["author_name"],
        json!("Name")
    );
    library.verify().unwrap();
}

#[test]
fn an_unknown_source_identity_does_not_create_a_snapshot_or_duplicate_media_for_equal_bytes() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = begin(&library);
    let mut first_input = input(
        &first,
        1,
        "unknown",
        b"a saved cover without its source URL",
    );
    first_input.media[0].source_key = None;
    let first = library.complete_acquisition(first.id, first_input).unwrap();
    let next = begin(&library);
    let mut check = input(&next, 2, "unknown", b"a saved cover without its source URL");
    check.media[0].source_key = None;
    let checked = library.complete_acquisition(next.id, check).unwrap();
    assert_eq!(checked.snapshot_id, first.snapshot_id);
    assert_eq!(count(&library, "source_media"), 1);
    assert_eq!(count(&library, "media_renditions"), 1);
    assert_eq!(count(&library, "acquisition_observations"), 2);
    library.verify().unwrap();
}

#[test]
fn an_unchanged_check_that_fails_at_commit_stays_pending_and_can_retry_without_a_new_snapshot() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = begin(&library);
    let first = library
        .complete_acquisition(first.id, input(&first, 1, "original", b"original"))
        .unwrap();
    let next = begin(&library);
    library.conn.lock().unwrap().execute_batch("CREATE TRIGGER refuse_check BEFORE INSERT ON outbox WHEN NEW.operation='capture' BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
    assert!(library
        .complete_acquisition(next.id, input(&next, 2, "original", b"original"))
        .is_err());
    assert_eq!(
        library.acquisition_status(next.id).unwrap().state,
        AcquisitionState::Pending
    );
    assert_eq!(count(&library, "acquisition_observations"), 1);
    assert_eq!(count(&library, "revision_snapshots"), 1);
    assert_eq!(
        library.current_snapshot(first.entity_id).unwrap(),
        first.snapshot_id
    );
    library.verify().unwrap();
    library
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER refuse_check")
        .unwrap();
    let retried = library
        .complete_acquisition(next.id, input(&next, 2, "original", b"original"))
        .unwrap();
    assert_eq!(retried.snapshot_id, first.snapshot_id);
    assert_eq!(count(&library, "acquisition_observations"), 2);
    library.verify().unwrap();
}
