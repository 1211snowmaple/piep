use super::*;
use crate::library::{
    BodyAssetReference, CapturedAttachment, CapturedMedia, ReaderBody, ResourceReference,
    ResourceScope, SourceBody,
};
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("piep-acquisition-reader-{}", LibraryId::now_v7())))
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
            "example",
            EntityKind::Work,
            "one",
            "offline-test",
        )
        .unwrap()
        .ticket
}
fn input(ticket: &AcquisitionTicket, text: &str, counter: u64) -> Capture {
    let body = json!({"blocks":[{"type":"p","text":text},{"type":"image","imageId":"image"},{"type":"file","fileId":"download"},{"type":"image","imageId":"unavailable"}],"imageMap":{"image":{"originalUrl":"https://example.test/image"}},"fileMap":{"download":{"url":"https://example.test/file","name":"document"}},"unknown":{"number":9007199254740993u64}});
    Capture {
        request_id: ticket.id,
        acquired_at: Some(1234),
        provider: ticket.provider.clone(),
        external_key: ticket.external_key.clone(),
        kind: ticket.kind,
        route: ticket.route.clone(),
        raw: serde_json::to_vec(&json!({"body":body,"counter":counter}))
            .unwrap()
            .into(),
        metadata: BTreeMap::from([("title".into(), json!("Title"))]),
        field_states: BTreeMap::from([
            ("media".into(), "known".into()),
            ("attachments".into(), "known".into()),
        ]),
        body: Some(SourceBody::FanboxBody(body)),
        media: vec![CapturedMedia {
            slot: "picture".into(),
            role: "illustration".into(),
            source_key: Some("image-source".into()),
            source_url: Some("https://example.test/image".into()),
            quality: "original".into(),
            original_name: "image.png".into(),
            mime_type: "image/png".into(),
            bytes: Some(b"original image".to_vec().into()),
        }],
        attachments: vec![CapturedAttachment {
            slot: "document".into(),
            original_name: "document.pdf".into(),
            mime_type: Some("application/pdf".into()),
            bytes: b"original document".to_vec().into(),
        }],
    }
}
fn bindings() -> Vec<ReaderBinding> {
    vec![
        ReaderBinding {
            reference: BodyAssetReference::ArticleImage {
                key: "image".into(),
            },
            resource: Some(ResourceReference::Media {
                slot: "picture".into(),
            }),
        },
        ReaderBinding {
            reference: BodyAssetReference::ArticleFile {
                key: "download".into(),
            },
            resource: Some(ResourceReference::File {
                slot: "document".into(),
            }),
        },
        ReaderBinding {
            reference: BodyAssetReference::ArticleImage {
                key: "unavailable".into(),
            },
            resource: None,
        },
    ]
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
fn manifest(library: &Library, info: &AcquisitionInfo) -> serde_json::Value {
    let file = library.file(info.input_manifest.unwrap()).unwrap();
    let mut bytes = Vec::new();
    library.objects.export(&file, &mut bytes).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn work_completion_is_immediately_readable_and_retry_keeps_the_old_result_without_rewinding() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = begin(&library);
    let capture = input(&first, "first body", 1);
    let raw = match &capture.raw {
        CaptureSource::Bytes(bytes) => bytes.clone(),
        _ => unreachable!(),
    };
    library
        .record_acquisition_payload(first.id, "response", &raw.into())
        .unwrap();
    let saved = library
        .complete_acquisition_with_reader(first.id, capture, bindings())
        .unwrap();
    let info = library.acquisition_status(first.id).unwrap();
    assert_eq!(info.state, AcquisitionState::Complete);
    let output = info.reader.clone().unwrap();
    assert_eq!(output.entity, saved.entity_id);
    assert_eq!(output.snapshot, saved.snapshot_id);
    let document = library
        .reader_document(saved.entity_id, ResourceScope::Snapshot(saved.snapshot_id))
        .unwrap();
    assert_eq!(document.bindings_hash, Some(output.bindings_hash));
    assert_eq!(document.bindings.len(), 3);
    assert!(
        matches!(document.body,ReaderBody::Article(body) if body["blocks"][0]["text"]=="first body")
    );
    let recorded = manifest(&library, &info);
    assert_eq!(recorded["format"], "capture-request-v3");
    assert_eq!(
        recorded["reader_bindings"],
        serde_json::to_value(
            library
                .reader_bindings(saved.entity_id, saved.snapshot_id)
                .unwrap()
                .bindings
        )
        .unwrap()
    );
    let next = begin(&library);
    let revised = library
        .complete_acquisition_with_reader(next.id, input(&next, "next body", 2), bindings())
        .unwrap();
    let mut reordered = bindings();
    reordered.reverse();
    let retry = library
        .complete_acquisition_with_reader(first.id, input(&first, "first body", 1), reordered)
        .unwrap();
    assert_eq!(retry.snapshot_id, saved.snapshot_id);
    assert_eq!(
        library.current_snapshot(saved.entity_id).unwrap(),
        revised.snapshot_id
    );
    let mut different = bindings();
    different[2].resource = Some(ResourceReference::Media {
        slot: "picture".into(),
    });
    assert!(library
        .complete_acquisition_with_reader(first.id, input(&first, "first body", 1), different)
        .is_err());
    assert!(library
        .complete_acquisition(first.id, input(&first, "first body", 1))
        .is_err());
    assert_eq!(count(&library, "revision_snapshots"), 2);
    assert_eq!(count(&library, "acquisition_reader_outputs"), 2);
    library.verify().unwrap();
}

#[test]
fn unchanged_checks_reuse_the_snapshot_and_reader_while_recording_each_completed_request() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = begin(&library);
    let saved = library
        .complete_acquisition_with_reader(first.id, input(&first, "body", 1), bindings())
        .unwrap();
    let plan = library
        .reader_bindings(saved.entity_id, saved.snapshot_id)
        .unwrap();
    let second = begin(&library);
    let checked = library
        .complete_acquisition_with_reader(second.id, input(&second, "body", 2), bindings())
        .unwrap();
    assert_eq!(checked.snapshot_id, saved.snapshot_id);
    assert!(!checked.created_revision);
    assert_eq!(
        library
            .reader_bindings(saved.entity_id, saved.snapshot_id)
            .unwrap(),
        plan
    );
    assert_eq!(count(&library, "source_revisions"), 1);
    assert_eq!(count(&library, "revision_snapshots"), 1);
    assert_eq!(count(&library, "reader_binding_sets"), 1);
    assert_eq!(count(&library, "acquisition_reader_outputs"), 2);
    assert_eq!(
        library
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT generation FROM entity_current WHERE entity_id=?1",
                [saved.entity_id.as_bytes().as_slice()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    library.verify().unwrap();
}

#[test]
fn a_failure_after_binding_publication_rolls_back_the_reader_snapshot_and_completion_but_keeps_received_parts(
) {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = begin(&library);
    library
        .record_acquisition_payload(first.id, "response", &b"received original".to_vec().into())
        .unwrap();
    library.conn.lock().unwrap().execute_batch("CREATE TRIGGER refuse_reader_output BEFORE INSERT ON acquisition_reader_outputs BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
    assert!(library
        .complete_acquisition_with_reader(first.id, input(&first, "body", 1), bindings())
        .is_err());
    for table in [
        "revision_snapshots",
        "source_revisions",
        "reader_binding_sets",
        "reader_bindings",
        "acquisition_reader_outputs",
        "acquisition_observations",
        "outbox",
    ] {
        assert_eq!(count(&library, table), 0, "{table}");
    }
    let status = library.acquisition_status(first.id).unwrap();
    assert_eq!(status.state, AcquisitionState::Pending);
    assert_eq!(status.payloads.len(), 1);
    assert!(status.reader.is_none());
    library.verify().unwrap();
    library
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER refuse_reader_output;")
        .unwrap();
    let saved = library
        .complete_acquisition_with_reader(first.id, input(&first, "body", 1), bindings())
        .unwrap();
    let next = begin(&library);
    library.conn.lock().unwrap().execute_batch("CREATE TRIGGER refuse_reader_footer BEFORE INSERT ON outbox WHEN NEW.operation='capture' BEGIN SELECT RAISE(ABORT,'injected footer failure'); END;").unwrap();
    assert!(library
        .complete_acquisition_with_reader(next.id, input(&next, "new body", 2), bindings())
        .is_err());
    assert_eq!(
        library.current_snapshot(saved.entity_id).unwrap(),
        saved.snapshot_id
    );
    assert_eq!(count(&library, "reader_binding_sets"), 1);
    assert_eq!(count(&library, "revision_snapshots"), 1);
    assert!(library
        .acquisition_status(next.id)
        .unwrap()
        .reader
        .is_none());
    library
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER refuse_reader_footer;")
        .unwrap();
    let revised = library
        .complete_acquisition_with_reader(next.id, input(&next, "new body", 2), bindings())
        .unwrap();
    assert_ne!(saved.snapshot_id, revised.snapshot_id);
    library.verify().unwrap();
}

#[test]
fn supplementing_an_existing_snapshot_is_atomic_and_does_not_rewrite_its_source_fingerprint() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = begin(&library);
    let saved = library
        .complete_acquisition(first.id, input(&first, "body", 1))
        .unwrap();
    assert!(library
        .reader_document(saved.entity_id, ResourceScope::Snapshot(saved.snapshot_id))
        .is_err());
    let next = begin(&library);
    library.conn.lock().unwrap().execute_batch("CREATE TRIGGER refuse_reader_footer BEFORE INSERT ON outbox WHEN NEW.operation='capture' BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
    assert!(library
        .complete_acquisition_with_reader(next.id, input(&next, "body", 2), bindings())
        .is_err());
    assert_eq!(count(&library, "reader_binding_sets"), 0);
    assert_eq!(count(&library, "revision_snapshots"), 1);
    assert_eq!(count(&library, "acquisition_observations"), 1);
    library
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER refuse_reader_footer;")
        .unwrap();
    let checked = library
        .complete_acquisition_with_reader(next.id, input(&next, "body", 2), bindings())
        .unwrap();
    assert_eq!(checked.snapshot_id, saved.snapshot_id);
    assert!(!checked.created_revision);
    assert_eq!(count(&library, "revision_snapshots"), 1);
    assert_eq!(count(&library, "acquisition_reader_outputs"), 1);
    library
        .reader_document(saved.entity_id, ResourceScope::Snapshot(saved.snapshot_id))
        .unwrap();
    library.verify().unwrap();
}

#[test]
fn stale_and_cancelled_requests_keep_their_inputs_without_publishing_reader_outputs() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let old = begin(&library);
    library
        .record_acquisition_payload(old.id, "response", &b"old original".to_vec().into())
        .unwrap();
    let new = begin(&library);
    let saved = library
        .complete_acquisition_with_reader(new.id, input(&new, "new", 2), bindings())
        .unwrap();
    assert!(matches!(
        library.complete_acquisition_with_reader(old.id, input(&old, "old", 1), bindings()),
        Err(Error::Superseded)
    ));
    let stale = library.acquisition_status(old.id).unwrap();
    assert_eq!(stale.state, AcquisitionState::Superseded);
    assert!(stale.reader.is_none());
    assert_eq!(manifest(&library, &stale)["format"], "capture-request-v3");
    let cancelled = begin(&library);
    library.cancel_acquisition(cancelled.id).unwrap();
    assert!(matches!(
        library.complete_acquisition_with_reader(
            cancelled.id,
            input(&cancelled, "cancelled", 3),
            bindings()
        ),
        Err(Error::Canceled)
    ));
    assert_eq!(count(&library, "reader_binding_sets"), 1);
    assert_eq!(count(&library, "acquisition_reader_outputs"), 1);
    assert_eq!(
        library.current_snapshot(saved.entity_id).unwrap(),
        saved.snapshot_id
    );
    library.verify().unwrap();
}

#[test]
fn partial_body_inputs_must_bind_the_selected_known_body_instead_of_an_incomplete_response() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = begin(&library);
    let saved = library
        .complete_acquisition_with_reader(first.id, input(&first, "full", 1), bindings())
        .unwrap();
    let next = begin(&library);
    let partial = || {
        let mut capture = input(&next, "partial", 2);
        capture.body = Some(SourceBody::FanboxBody(json!({"text":"partial"})));
        capture.field_states.insert("body".into(), "unknown".into());
        capture
    };
    assert!(library
        .complete_acquisition_with_reader(next.id, partial(), vec![])
        .is_err());
    assert_eq!(
        library.acquisition_status(next.id).unwrap().state,
        AcquisitionState::Pending
    );
    let checked = library
        .complete_acquisition_with_reader(next.id, partial(), bindings())
        .unwrap();
    assert_eq!(checked.snapshot_id, saved.snapshot_id);
    assert!(
        matches!(library.reader_document(saved.entity_id,ResourceScope::Snapshot(saved.snapshot_id)).unwrap().body,ReaderBody::Article(body) if body["blocks"][0]["text"]=="full")
    );
    library.verify().unwrap();
}

#[test]
fn quality_improvements_make_a_readable_snapshot_without_creating_an_author_revision() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = begin(&library);
    let mut reduced = input(&first, "body", 1);
    reduced.media[0].quality = "thumbnail".into();
    reduced.media[0].bytes = Some(b"smaller image".to_vec().into());
    let saved = library
        .complete_acquisition_with_reader(first.id, reduced, bindings())
        .unwrap();
    let next = begin(&library);
    let improved = library
        .complete_acquisition_with_reader(next.id, input(&next, "body", 2), bindings())
        .unwrap();
    assert!(!improved.created_revision);
    assert_ne!(improved.snapshot_id, saved.snapshot_id);
    assert_eq!(count(&library, "source_revisions"), 1);
    let image_hash = |snapshot| {
        library
            .reader_document(saved.entity_id, ResourceScope::Snapshot(snapshot))
            .unwrap()
            .bindings
            .into_iter()
            .find(|r| {
                r.reference
                    == BodyAssetReference::ArticleImage {
                        key: "image".into(),
                    }
            })
            .unwrap()
            .resource
            .unwrap()
            .file
            .unwrap()
            .id
    };
    assert_eq!(
        image_hash(saved.snapshot_id),
        Fingerprint::of(b"smaller image")
    );
    assert_eq!(
        image_hash(improved.snapshot_id),
        Fingerprint::of(b"original image")
    );
    assert_ne!(
        library
            .acquisition_status(first.id)
            .unwrap()
            .reader
            .unwrap()
            .bindings_hash,
        library
            .acquisition_status(next.id)
            .unwrap()
            .reader
            .unwrap()
            .bindings_hash
    );
    library.verify().unwrap();
}

#[test]
fn a_missing_body_cannot_report_ready_but_an_explicit_empty_body_can() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = begin(&library);
    let mut absent = input(&first, "body", 1);
    absent.body = None;
    absent.media.clear();
    absent.attachments.clear();
    assert!(matches!(
        library.complete_acquisition_with_reader(first.id, absent, vec![]),
        Err(Error::ResourceUnavailable)
    ));
    assert_eq!(
        library.acquisition_status(first.id).unwrap().state,
        AcquisitionState::Pending
    );
    assert_eq!(count(&library, "revision_snapshots"), 0);
    let mut empty = input(&first, "body", 1);
    empty.raw = Vec::new().into();
    empty.body = Some(SourceBody::PlainText(String::new()));
    empty.media.clear();
    empty.attachments.clear();
    let saved = library
        .complete_acquisition_with_reader(first.id, empty, vec![])
        .unwrap();
    let document = library
        .reader_document(saved.entity_id, ResourceScope::Snapshot(saved.snapshot_id))
        .unwrap();
    assert!(matches!(document.body,ReaderBody::PlainText(text) if text.is_empty()));
    assert!(document.bindings_hash.is_some());
    library.verify().unwrap();
}

#[test]
fn legacy_v2_request_encoding_keeps_the_exact_json_and_has_no_reader_field() {
    #[derive(Serialize)]
    struct Legacy<'a> {
        format: &'a str,
        ticket: &'a AcquisitionTicket,
        parts: &'a [RawPart],
        capture: Box<serde_json::value::RawValue>,
    }
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let ticket = begin(&library);
    let context = CompletionContext {
        ticket,
        parts: vec![],
        reader_bindings: None,
    };
    let raw = r#"[ "opaque", {"number":0.07221960411257732,"integer":9007199254740993} ]"#;
    let expected = serde_json::to_vec(&Legacy {
        format: "capture-request-v2",
        ticket: &context.ticket,
        parts: &context.parts,
        capture: serde_json::value::RawValue::from_string(raw.into()).unwrap(),
    })
    .unwrap();
    assert_eq!(
        encode_manifest(&context, raw.as_bytes().to_vec()).unwrap(),
        expected
    );
}

#[test]
fn missing_or_borrowed_completion_links_are_rejected_by_sql_and_by_the_audit() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = begin(&library);
    let saved = library
        .complete_acquisition_with_reader(first.id, input(&first, "body", 1), bindings())
        .unwrap();
    let output = library
        .acquisition_status(first.id)
        .unwrap()
        .reader
        .unwrap();
    let other = library
        .begin_acquisition(
            LibraryId::now_v7(),
            "example",
            EntityKind::Work,
            "other",
            "offline-test",
        )
        .unwrap()
        .ticket;
    {
        let conn = library.conn.lock().unwrap();
        assert!(conn
            .execute(
                "INSERT INTO acquisition_reader_outputs VALUES(?1,?2,?3,1,?4)",
                params![
                    other.id.as_bytes().as_slice(),
                    saved.entity_id.as_bytes().as_slice(),
                    saved.snapshot_id.as_bytes().as_slice(),
                    output.bindings_hash.0.as_slice()
                ]
            )
            .is_err());
        assert!(conn
            .execute("DELETE FROM acquisition_reader_outputs", [])
            .is_err());
        conn.execute_batch(
            "DROP TRIGGER acquisition_reader_delete; DELETE FROM acquisition_reader_outputs;",
        )
        .unwrap();
    }
    assert!(matches!(library.verify(), Err(Error::Integrity(_))));
    assert!(library
        .complete_acquisition_with_reader(first.id, input(&first, "body", 1), bindings())
        .is_err());
}
