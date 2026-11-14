use super::*;
use crate::library::{
    protocol, Capture, CapturedAttachment, CapturedMedia, EditBlock, EditContent, EditMedia,
    EditRequest, EntityKind, ObjectStore,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("piep-resource-{}", LibraryId::now_v7())))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn capture(key: &str, text: &str, quality: &str, bytes: Option<&[u8]>) -> Capture {
    Capture {
        request_id: LibraryId::now_v7(),
        acquired_at: Some(1234),
        provider: "pixiv".into(),
        external_key: key.into(),
        kind: EntityKind::Work,
        route: "offline-test".into(),
        raw: serde_json::to_vec(&json!({"text":text})).unwrap().into(),
        metadata: BTreeMap::from([("title".into(), json!(text))]),
        field_states: BTreeMap::new(),
        body: Some(text.into()),
        media: vec![CapturedMedia {
            slot: "cover".into(),
            role: "cover".into(),
            source_key: Some(format!("source-image:{key}")),
            source_url: None,
            quality: quality.into(),
            original_name: "cover.png".into(),
            mime_type: "image/png".into(),
            bytes: bytes.map(|b| b.to_vec().into()),
        }],
        attachments: vec![],
    }
}
fn cover() -> ResourceReference {
    ResourceReference::Media {
        slot: "cover".into(),
    }
}

#[test]
fn transfer_uses_the_pinned_quality_snapshot_and_preserves_encoded_bytes() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = library
        .capture(capture("one", "body", "thumbnail", Some(b"old thumb")))
        .unwrap();
    let latest = library
        .capture(capture(
            "one",
            "body",
            "original",
            Some(&vec![b'x'; 30_000]),
        ))
        .unwrap();
    assert!(!latest.created_revision);
    for (saved, expected) in [
        (&first, b"old thumb".as_slice()),
        (&latest, vec![b'x'; 30_000].as_slice()),
    ] {
        let scope = ResourceScope::Snapshot(saved.snapshot_id);
        let manifest = library
            .resource_transfer_manifest(saved.entity_id, scope, &cover())
            .unwrap();
        let checked = protocol::ValidatedTransfer::parse(&manifest.encode().unwrap()).unwrap();
        assert_eq!(checked.manifest().target.library_id.get(), library.id);
        assert_eq!(checked.manifest().target.entity_id.get(), first.entity_id);
        let store = ObjectStore::open(&temp.0.join(saved.snapshot_id.to_string())).unwrap();
        for (ordinal, chunk) in checked.file().chunks.iter().enumerate() {
            let mut writer = CheckingWriter {
                library: &library,
                bytes: Vec::new(),
            };
            library
                .export_resource_chunk(
                    saved.entity_id,
                    scope,
                    &cover(),
                    checked.file().id,
                    ordinal,
                    &mut writer,
                )
                .unwrap();
            store
                .import_chunk(chunk, std::io::Cursor::new(&writer.bytes))
                .unwrap();
            let mut roundtrip = Vec::new();
            store.export_chunk(chunk, &mut roundtrip).unwrap();
            assert_eq!(roundtrip, writer.bytes);
        }
        let mut restored = Vec::new();
        store.export(checked.file(), &mut restored).unwrap();
        assert_eq!(restored, expected);
    }
}

#[test]
fn transfer_rechecks_owner_reference_file_and_ordinal_before_writing_any_bytes() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = library
        .capture(capture("one", "body", "original", Some(b"image")))
        .unwrap();
    let foreign = library
        .capture(capture("two", "another body", "unknown", None))
        .unwrap();
    let scope = ResourceScope::Snapshot(first.snapshot_id);
    let valid = library
        .resource_transfer_manifest(first.entity_id, scope, &cover())
        .unwrap();
    assert!(library
        .resource_transfer_manifest(foreign.entity_id, scope, &cover())
        .is_err());
    for (entity, scope, reference, file, ordinal) in [
        (foreign.entity_id, scope, cover(), valid.file.id.get(), 0),
        (
            first.entity_id,
            ResourceScope::Snapshot(foreign.snapshot_id),
            cover(),
            valid.file.id.get(),
            0,
        ),
        (
            first.entity_id,
            scope,
            ResourceReference::Body,
            valid.file.id.get(),
            0,
        ),
        (
            first.entity_id,
            scope,
            cover(),
            Fingerprint::of(b"wrong file"),
            0,
        ),
        (first.entity_id, scope, cover(), valid.file.id.get(), 1),
    ] {
        let mut bytes = Vec::new();
        assert!(library
            .export_resource_chunk(entity, scope, &reference, file, ordinal, &mut bytes)
            .is_err());
        assert!(bytes.is_empty());
    }
    assert!(library
        .resource_transfer_manifest(
            foreign.entity_id,
            ResourceScope::Snapshot(foreign.snapshot_id),
            &cover()
        )
        .is_err());
}

#[test]
fn transfer_of_large_attachment_reassembles_exact_original_and_empty_body_is_explicit() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let payload: Vec<u8> = (0..9 * 1024 * 1024 + 17).map(|n| (n % 251) as u8).collect();
    let mut input = capture("large", "", "unknown", None);
    input.attachments.push(CapturedAttachment {
        slot: "data".into(),
        original_name: "archive.zip".into(),
        mime_type: None,
        bytes: payload.clone().into(),
    });
    let saved = library.capture(input).unwrap();
    let scope = ResourceScope::Snapshot(saved.snapshot_id);
    for (reference, expected) in [
        (ResourceReference::Body, b"".as_slice()),
        (
            ResourceReference::File {
                slot: "data".into(),
            },
            payload.as_slice(),
        ),
    ] {
        let manifest = library
            .resource_transfer_manifest(saved.entity_id, scope, &reference)
            .unwrap();
        let checked = protocol::ValidatedTransfer::parse(&manifest.encode().unwrap()).unwrap();
        let receiver =
            ObjectStore::open(&temp.0.join(format!("received-{}", checked.file().id))).unwrap();
        for (ordinal, chunk) in checked.file().chunks.iter().enumerate() {
            let mut bytes = Vec::new();
            library
                .export_resource_chunk(
                    saved.entity_id,
                    scope,
                    &reference,
                    checked.file().id,
                    ordinal,
                    &mut bytes,
                )
                .unwrap();
            receiver
                .import_chunk(chunk, std::io::Cursor::new(bytes))
                .unwrap();
        }
        let mut restored = Vec::new();
        receiver.export(checked.file(), &mut restored).unwrap();
        assert_eq!(restored, expected);
    }
}
fn read(
    library: &Library,
    entity: LibraryId,
    scope: ResourceScope,
    reference: &ResourceReference,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    library
        .read_resource(entity, scope, reference, None, &mut bytes)
        .unwrap();
    bytes
}

#[test]
fn quality_repairs_keep_old_bytes_and_missing_images_and_bodies_never_fall_back_or_become_empty_files(
) {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = library
        .capture(capture(
            "one",
            "body",
            "thumbnail",
            Some(b"thumbnail bytes"),
        ))
        .unwrap();
    let repair = library
        .capture(capture(
            "one",
            "body",
            "original",
            Some(b"original image with metadata"),
        ))
        .unwrap();
    assert!(!repair.created_revision);
    for (snapshot, quality, expected) in [
        (
            first.snapshot_id,
            "thumbnail",
            b"thumbnail bytes".as_slice(),
        ),
        (
            repair.snapshot_id,
            "original",
            b"original image with metadata".as_slice(),
        ),
    ] {
        let scope = ResourceScope::Snapshot(snapshot);
        let manifest = library.resource_manifest(first.entity_id, scope).unwrap();
        assert_eq!(
            manifest
                .resources
                .iter()
                .filter(|info| !matches!(info.reference, ResourceReference::Original { .. }))
                .count(),
            2
        );
        let originals: Vec<_> = manifest
            .resources
            .iter()
            .filter(|info| info.role == "source_response")
            .collect();
        assert_eq!(originals.len(), 1);
        assert_eq!(originals[0].format, Some(ResourceFormat::OriginalResponse));
        assert_eq!(
            read(&library, first.entity_id, scope, &originals[0].reference),
            serde_json::to_vec(&json!({"text":"body"})).unwrap()
        );
        let info = library
            .resource_info(first.entity_id, scope, &cover())
            .unwrap();
        assert_eq!(info.quality.as_deref(), Some(quality));
        assert_eq!(info.file.unwrap().id, Fingerprint::of(expected));
        assert_eq!(read(&library, first.entity_id, scope, &cover()), expected);
        assert_eq!(
            read(&library, first.entity_id, scope, &ResourceReference::Body),
            b"body"
        );
    }
    let mut missing = capture("one", "body", "unknown", None);
    missing.media[0].source_key = Some("a replacement image that could not be saved".into());
    let missing = library.capture(missing).unwrap();
    let scope = ResourceScope::Snapshot(missing.snapshot_id);
    assert!(library
        .resource_info(first.entity_id, scope, &cover())
        .unwrap()
        .file
        .is_none());
    assert!(matches!(
        library.read_resource(first.entity_id, scope, &cover(), None, Vec::new()),
        Err(Error::ResourceUnavailable)
    ));
    let mut metadata_only = capture("metadata-only", "title", "unknown", None);
    metadata_only.body = None;
    let metadata_only = library.capture(metadata_only).unwrap();
    assert!(matches!(
        library.read_body(
            metadata_only.entity_id,
            metadata_only.snapshot_id,
            Vec::new()
        ),
        Err(Error::ResourceUnavailable)
    ));
    let empty = library
        .capture(capture("empty", "", "unknown", None))
        .unwrap();
    assert_eq!(
        read(
            &library,
            empty.entity_id,
            ResourceScope::Snapshot(empty.snapshot_id),
            &ResourceReference::Body
        ),
        b""
    );
    assert_eq!(
        library
            .resource_info(
                empty.entity_id,
                ResourceScope::Snapshot(empty.snapshot_id),
                &ResourceReference::Body
            )
            .unwrap()
            .file
            .unwrap()
            .length,
        0
    );
    library.verify().unwrap();
}

struct CheckingWriter<'a> {
    library: &'a Library,
    bytes: Vec<u8>,
}
impl Write for CheckingWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.library.works(1).unwrap();
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn attachment_ranges_cross_chunks_without_holding_the_database_and_cannot_escape_the_selected_owner(
) {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let payload: Vec<u8> = (0..(9 * 1024 * 1024 + 13))
        .map(|n| (n % 251) as u8)
        .collect();
    let mut input = capture("one", "body", "unknown", None);
    input.attachments.push(CapturedAttachment {
        slot: "payload".into(),
        original_name: "document.bin".into(),
        mime_type: Some("application/octet-stream".into()),
        bytes: payload.clone().into(),
    });
    let saved = library.capture(input).unwrap();
    let scope = ResourceScope::Snapshot(saved.snapshot_id);
    let reference = ResourceReference::File {
        slot: "payload".into(),
    };
    let info = library
        .resource_info(saved.entity_id, scope, &reference)
        .unwrap();
    assert_eq!(info.file.as_ref().unwrap().length, payload.len() as u64);
    let offset = 4 * 1024 * 1024 - 7;
    let mut writer = CheckingWriter {
        library: &library,
        bytes: Vec::new(),
    };
    library
        .read_resource(
            saved.entity_id,
            scope,
            &reference,
            Some(ResourceRange {
                offset: offset as u64,
                length: 64,
            }),
            &mut writer,
        )
        .unwrap();
    assert_eq!(writer.bytes, payload[offset..offset + 64]);
    assert!(matches!(
        library.read_resource(
            saved.entity_id,
            scope,
            &reference,
            Some(ResourceRange {
                offset: 1,
                length: u64::MAX
            }),
            Vec::new()
        ),
        Err(Error::InvalidRange)
    ));
    library
        .read_resource(
            saved.entity_id,
            scope,
            &reference,
            Some(ResourceRange {
                offset: payload.len() as u64,
                length: 0,
            }),
            Vec::new(),
        )
        .unwrap();
    let other = library
        .capture(capture("other", "other body", "unknown", None))
        .unwrap();
    assert!(matches!(
        library.resource_manifest(other.entity_id, scope),
        Err(Error::ResourceUnavailable)
    ));
    assert!(matches!(
        library.read_resource(
            other.entity_id,
            scope,
            &reference,
            Some(ResourceRange {
                offset: 1,
                length: u64::MAX
            }),
            Vec::new()
        ),
        Err(Error::ResourceUnavailable)
    ));
    let guessed_hash = ResourceReference::File {
        slot: info.file.unwrap().id.to_string(),
    };
    assert!(matches!(
        library.resource_info(saved.entity_id, scope, &guessed_hash),
        Err(Error::ResourceUnavailable)
    ));
    let output = temp.0.join("export.bin");
    library
        .export_resource(saved.entity_id, scope, &reference, &output)
        .unwrap();
    assert_eq!(std::fs::read(&output).unwrap(), payload);
    assert!(library
        .export_resource(saved.entity_id, scope, &reference, &output)
        .is_err());
    library.verify().unwrap();
}

#[test]
fn edited_documents_pin_their_base_and_images_and_never_expose_another_work_or_unsealed_edit() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let original = library
        .capture(capture(
            "one",
            "original text",
            "original",
            Some(b"original cover"),
        ))
        .unwrap();
    let base_image = library
        .media_file(original.entity_id, original.snapshot_id, "cover")
        .unwrap();
    let added = library
        .store_original(&b"an added image".to_vec().into())
        .unwrap();
    let document = EditContent {
        format: "piep-edit-v1".into(),
        title: Some("edited".into()),
        blocks: vec![
            EditBlock {
                id: LibraryId::now_v7(),
                ordinal: 0,
                kind: "image".into(),
                text: None,
                attrs: json!({}),
                media_slot: Some("base-cover".into()),
            },
            EditBlock {
                id: LibraryId::now_v7(),
                ordinal: 1,
                kind: "image".into(),
                text: None,
                attrs: json!({}),
                media_slot: Some("added".into()),
            },
        ],
        media: vec![
            EditMedia {
                slot: "base-cover".into(),
                file: base_image.id,
                original_name: "base.png".into(),
                mime_type: Some("image/png".into()),
            },
            EditMedia {
                slot: "added".into(),
                file: added.id,
                original_name: "added.png".into(),
                mime_type: Some("image/png".into()),
            },
        ],
        provenance: None,
    };
    let edit = library
        .save_edit(EditRequest {
            request_id: LibraryId::now_v7(),
            work: original.entity_id,
            user: library.id,
            base: original.snapshot_id,
            parent: None,
            expected: 0,
            content: &document,
            activate: true,
        })
        .unwrap();
    let revised = library
        .capture(capture(
            "one",
            "revised text",
            "original",
            Some(b"a changed cover"),
        ))
        .unwrap();
    let scope = ResourceScope::Edit(edit);
    let manifest = library
        .resource_manifest(original.entity_id, scope)
        .unwrap();
    assert_eq!(manifest.base_snapshot, Some(original.snapshot_id));
    assert_ne!(manifest.base_snapshot, Some(revised.snapshot_id));
    assert_eq!(manifest.resources.len(), 3);
    let body_info = library
        .resource_info(original.entity_id, scope, &ResourceReference::Body)
        .unwrap();
    assert_eq!(body_info.format, Some(ResourceFormat::EditDocumentV1));
    let decoded: EditContent = serde_json::from_slice(&read(
        &library,
        original.entity_id,
        scope,
        &ResourceReference::Body,
    ))
    .unwrap();
    assert_eq!(decoded, document);
    assert_eq!(
        read(
            &library,
            original.entity_id,
            scope,
            &ResourceReference::Media {
                slot: "base-cover".into()
            }
        ),
        b"original cover"
    );
    assert_eq!(
        read(
            &library,
            original.entity_id,
            scope,
            &ResourceReference::Media {
                slot: "added".into()
            }
        ),
        b"an added image"
    );
    let other = library
        .capture(capture("other", "other body", "unknown", None))
        .unwrap();
    assert!(matches!(
        library.resource_manifest(other.entity_id, scope),
        Err(Error::ResourceUnavailable)
    ));
    assert!(matches!(
        library.resource_info(
            original.entity_id,
            scope,
            &ResourceReference::File {
                slot: "added".into()
            }
        ),
        Err(Error::ResourceUnavailable)
    ));
    let staged = LibraryId::now_v7();
    {
        let conn = library.conn.lock().unwrap();
        conn.execute("INSERT INTO edit_revisions(id,work_id,base_snapshot_id,parent_id,content_file_id,created_at,sealed) SELECT ?1,work_id,base_snapshot_id,parent_id,content_file_id,created_at,0 FROM edit_revisions WHERE id=?2",params![staged.as_bytes().as_slice(),edit.as_bytes().as_slice()]).unwrap();
    }
    assert!(matches!(
        library.resource_manifest(original.entity_id, ResourceScope::Edit(staged)),
        Err(Error::ResourceUnavailable)
    ));
    library
        .conn
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM edit_revisions WHERE id=?1",
            [staged.as_bytes().as_slice()],
        )
        .unwrap();
    let staged_snapshot = LibraryId::now_v7();
    {
        let conn = library.conn.lock().unwrap();
        conn.execute("INSERT INTO revision_snapshots(id,entity_id,revision_id,sequence,metadata_json,field_states_json,interpretation_version,semantic_rule,semantic_hash,manifest_hash,created_at) SELECT ?1,entity_id,revision_id,sequence+100,metadata_json,field_states_json,interpretation_version,semantic_rule,semantic_hash,manifest_hash,created_at FROM revision_snapshots WHERE id=?2",params![staged_snapshot.as_bytes().as_slice(),original.snapshot_id.as_bytes().as_slice()]).unwrap();
    }
    assert!(matches!(
        library.resource_manifest(original.entity_id, ResourceScope::Snapshot(staged_snapshot)),
        Err(Error::ResourceUnavailable)
    ));
    library
        .conn
        .lock()
        .unwrap()
        .execute(
            "DELETE FROM revision_snapshots WHERE id=?1",
            [staged_snapshot.as_bytes().as_slice()],
        )
        .unwrap();
    library.verify().unwrap();
}

#[test]
fn structured_article_bodies_are_delivered_as_json_with_their_body_format() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let body = json!({"blocks":[{"id":"one","type":"p","text":"本文","styles":[{"type":"bold","offset":0,"length":2}]},{"id":"two","type":"image","imageId":"image-one"}],"imageMap":{"image-one":{"originalUrl":"https://example.invalid/original.png"}}});
    let mut input = capture("article", "title", "unknown", None);
    input.provider = "fanbox".into();
    input.body = Some(crate::library::SourceBody::FanboxBody(body.clone()));
    let saved = library.capture(input).unwrap();
    let scope = ResourceScope::Snapshot(saved.snapshot_id);
    let info = library
        .resource_info(saved.entity_id, scope, &ResourceReference::Body)
        .unwrap();
    assert_eq!(
        info.format,
        Some(ResourceFormat::SourceBody(BodyFormat::FanboxBody))
    );
    assert_eq!(info.mime_type.as_deref(), Some("application/json"));
    let restored: serde_json::Value = serde_json::from_slice(&read(
        &library,
        saved.entity_id,
        scope,
        &ResourceReference::Body,
    ))
    .unwrap();
    assert_eq!(restored, body);
    library.verify().unwrap();
}

#[test]
fn a_corrupted_rendition_relation_never_exposes_the_foreign_file() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = library
        .capture(capture("one", "one", "original", Some(b"one image")))
        .unwrap();
    let other = library
        .capture(capture(
            "other",
            "other",
            "original",
            Some(b"a foreign image"),
        ))
        .unwrap();
    {
        let conn = library.conn.lock().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=OFF; DROP TRIGGER snapshot_media_published_update;",
        )
        .unwrap();
        conn.execute("UPDATE snapshot_media SET rendition_id=(SELECT rendition_id FROM snapshot_media WHERE snapshot_id=?2 AND slot='cover') WHERE snapshot_id=?1 AND slot='cover'",params![first.snapshot_id.as_bytes().as_slice(),other.snapshot_id.as_bytes().as_slice()]).unwrap();
    }
    let mut output = Vec::new();
    assert!(matches!(
        library.read_resource(
            first.entity_id,
            ResourceScope::Snapshot(first.snapshot_id),
            &cover(),
            None,
            &mut output
        ),
        Err(Error::Integrity(_))
    ));
    assert!(output.is_empty());
}
