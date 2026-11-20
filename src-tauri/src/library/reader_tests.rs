use super::*;
use crate::library::{Capture, CapturedAttachment, CapturedMedia, EntityKind};
use serde_json::json;
use std::path::PathBuf;
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("piep-reader-bindings-{}", LibraryId::now_v7())))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn input(body: SourceBody, image: &[u8]) -> Capture {
    Capture {
        request_id: LibraryId::now_v7(),
        acquired_at: None,
        provider: "local".into(),
        external_key: "one".into(),
        kind: EntityKind::Work,
        route: "test".into(),
        raw: b"{}".to_vec().into(),
        metadata: BTreeMap::from([("title".into(), json!("Title"))]),
        field_states: BTreeMap::from([
            ("media".into(), "known".into()),
            ("attachments".into(), "known".into()),
        ]),
        body: Some(body),
        media: vec![CapturedMedia {
            slot: "chosen-picture".into(),
            role: "illustration".into(),
            source_key: Some("picture".into()),
            source_url: None,
            quality: "original".into(),
            original_name: "not-the-body-id.png".into(),
            mime_type: "image/png".into(),
            bytes: Some(image.to_vec().into()),
        }],
        attachments: vec![CapturedAttachment {
            slot: "chosen-file".into(),
            original_name: "unrelated-name.bin".into(),
            mime_type: None,
            bytes: b"file".to_vec().into(),
        }],
    }
}
fn binding(reference: BodyAssetReference, slot: &str) -> ReaderBinding {
    let resource = if reference.is_image() {
        ResourceReference::Media { slot: slot.into() }
    } else {
        ResourceReference::File { slot: slot.into() }
    };
    ReaderBinding {
        reference,
        resource: Some(resource),
    }
}

#[test]
fn bindings_pin_images_and_files_to_each_source_snapshot_and_backup() {
    let temp = Temp::new();
    let library = Library::create(&temp.0.join("source")).unwrap();
    let body = SourceBody::FanboxBody(
        json!({"blocks":[{"type":"image","imageId":"image"},{"type":"file","fileId":"file"}],"imageMap":{"image":{"originalUrl":"https://example.test/source"}},"fileMap":{"file":{"name":"original"}}}),
    );
    let first = library
        .capture(input(body.clone(), b"first image"))
        .unwrap();
    assert!(matches!(
        library.reader_document(first.entity_id, ResourceScope::Snapshot(first.snapshot_id)),
        Err(Error::ResourceUnavailable)
    ));
    let bindings = vec![
        binding(
            BodyAssetReference::ArticleImage {
                key: "image".into(),
            },
            "chosen-picture",
        ),
        binding(
            BodyAssetReference::ArticleFile { key: "file".into() },
            "chosen-file",
        ),
    ];
    let plan = library
        .publish_reader_bindings(first.entity_id, first.snapshot_id, bindings.clone())
        .unwrap();
    assert_eq!(
        library
            .publish_reader_bindings(first.entity_id, first.snapshot_id, bindings.clone())
            .unwrap(),
        plan
    );
    let next = library.capture(input(body, b"next image")).unwrap();
    library
        .publish_reader_bindings(next.entity_id, next.snapshot_id, bindings)
        .unwrap();
    let old = library
        .reader_document(first.entity_id, ResourceScope::Snapshot(first.snapshot_id))
        .unwrap();
    let new = library
        .reader_document(next.entity_id, ResourceScope::Snapshot(next.snapshot_id))
        .unwrap();
    let image = |document: &ReaderDocument| {
        document
            .bindings
            .iter()
            .find(|b| b.reference.is_image())
            .unwrap()
            .resource
            .as_ref()
            .unwrap()
            .file
            .as_ref()
            .unwrap()
            .id
    };
    assert_eq!(image(&old), Fingerprint::of(b"first image"));
    assert_eq!(image(&new), Fingerprint::of(b"next image"));
    assert!(matches!(old.body, ReaderBody::Article(_)));
    assert!(old.base_resources.is_none());
    library.backup(&temp.0.join("backup")).unwrap();
    drop(library);
    std::fs::rename(temp.0.join("source"), temp.0.join("unavailable")).unwrap();
    let backup = Library::open(&temp.0.join("backup")).unwrap();
    assert_eq!(
        backup
            .reader_bindings(first.entity_id, first.snapshot_id)
            .unwrap(),
        plan
    );
    assert_eq!(
        image(
            &backup
                .reader_document(first.entity_id, ResourceScope::Snapshot(first.snapshot_id))
                .unwrap()
        ),
        Fingerprint::of(b"first image")
    );
    backup.verify().unwrap();
}

#[test]
fn invalid_or_foreign_bindings_never_publish_a_partial_mapping_and_missing_is_explicit() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let reference = BodyAssetReference::PixivUploaded {
        image_id: "123".into(),
    };
    let body = SourceBody::PixivText("before[uploadedimage:123]after".into());
    let first = library.capture(input(body, b"image")).unwrap();
    for bindings in [
        vec![],
        vec![binding(reference.clone(), "absent")],
        vec![
            binding(reference.clone(), "chosen-picture"),
            binding(reference.clone(), "chosen-picture"),
        ],
        vec![ReaderBinding {
            reference: reference.clone(),
            resource: Some(ResourceReference::File {
                slot: "chosen-file".into(),
            }),
        }],
    ] {
        assert!(library
            .publish_reader_bindings(first.entity_id, first.snapshot_id, bindings)
            .is_err());
        assert!(library
            .reader_bindings(first.entity_id, first.snapshot_id)
            .is_err());
    }
    assert!(library
        .publish_reader_bindings(
            LibraryId::now_v7(),
            first.snapshot_id,
            vec![binding(reference.clone(), "chosen-picture")]
        )
        .is_err());
    let missing = vec![ReaderBinding {
        reference: reference.clone(),
        resource: None,
    }];
    library
        .publish_reader_bindings(first.entity_id, first.snapshot_id, missing.clone())
        .unwrap();
    assert!(library
        .reader_document(first.entity_id, ResourceScope::Snapshot(first.snapshot_id))
        .unwrap()
        .bindings[0]
        .resource
        .is_none());
    assert!(library
        .publish_reader_bindings(
            first.entity_id,
            first.snapshot_id,
            vec![binding(reference, "chosen-picture")]
        )
        .is_err());
    assert_eq!(
        library
            .reader_bindings(first.entity_id, first.snapshot_id)
            .unwrap()
            .bindings,
        missing
    );
    library.verify().unwrap();
}

#[test]
fn publication_failure_rolls_back_every_row_and_sql_cannot_rewrite_a_sealed_plan() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = library
        .capture(input(
            SourceBody::PixivText("[pixivimage:123-2]".into()),
            b"image",
        ))
        .unwrap();
    let bindings = vec![binding(
        BodyAssetReference::PixivIllustration {
            illustration_id: "123".into(),
            page: 2,
        },
        "chosen-picture",
    )];
    library.conn.lock().unwrap().execute_batch("CREATE TRIGGER refuse_binding BEFORE INSERT ON reader_bindings BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
    assert!(library
        .publish_reader_bindings(first.entity_id, first.snapshot_id, bindings.clone())
        .is_err());
    assert_eq!(
        library
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM reader_binding_sets", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    library
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER refuse_binding;")
        .unwrap();
    library
        .publish_reader_bindings(first.entity_id, first.snapshot_id, bindings)
        .unwrap();
    {
        let conn = library.conn.lock().unwrap();
        assert!(conn.execute("DELETE FROM reader_binding_sets", []).is_err());
        assert!(conn.execute("DELETE FROM reader_bindings", []).is_err());
        assert!(conn
            .execute("UPDATE reader_bindings SET media_slot='absent'", [])
            .is_err());
        assert!(conn
            .execute(
                "UPDATE reader_binding_sets SET bindings_hash=zeroblob(32)",
                []
            )
            .is_err());
    }
    library.verify().unwrap();
}

#[test]
fn editing_keeps_its_base_metadata_and_assets_even_when_the_base_body_is_unknown() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let mut capture = input(SourceBody::PlainText(String::new()), b"original");
    capture.body = None;
    let original = library.capture(capture).unwrap();
    let content = EditContent {
        format: "piep-edit-v1".into(),
        title: Some("Edited".into()),
        blocks: vec![crate::library::EditBlock {
            id: LibraryId::now_v7(),
            ordinal: 0,
            kind: "text".into(),
            text: Some("The saved edit".into()),
            attrs: json!({}),
            media_slot: None,
        }],
        media: vec![],
        provenance: None,
    };
    let edit = library
        .save_edit(crate::library::EditRequest {
            request_id: LibraryId::now_v7(),
            work: original.entity_id,
            user: library.id,
            base: original.snapshot_id,
            parent: None,
            expected: 0,
            content: &content,
            activate: true,
        })
        .unwrap();
    let mut changed = input(SourceBody::PlainText("New body".into()), b"new image");
    changed.metadata.insert("title".into(), json!("Changed"));
    library.capture(changed).unwrap();
    let document = library
        .reader_document(original.entity_id, ResourceScope::Edit(edit))
        .unwrap();
    assert_eq!(document.metadata["title"], json!("Title"));
    assert!(matches!(document.body,ReaderBody::Edit(body) if body==content));
    let base = document.base_resources.unwrap();
    assert_eq!(base.scope, ResourceScope::Snapshot(original.snapshot_id));
    let image = base
        .resources
        .into_iter()
        .find(|r| {
            r.reference
                == ResourceReference::Media {
                    slot: "chosen-picture".into(),
                }
        })
        .unwrap();
    assert_eq!(image.file.unwrap().id, Fingerprint::of(b"original"));
    assert!(library
        .reader_document(LibraryId::now_v7(), ResourceScope::Edit(edit))
        .is_err());
    library.verify().unwrap();
}

#[test]
fn auditing_and_reading_reject_a_corrupted_binding_even_when_its_target_exists() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let first = library
        .capture(input(
            SourceBody::PixivText("[uploadedimage:123]".into()),
            b"image",
        ))
        .unwrap();
    library
        .publish_reader_bindings(
            first.entity_id,
            first.snapshot_id,
            vec![binding(
                BodyAssetReference::PixivUploaded {
                    image_id: "123".into(),
                },
                "chosen-picture",
            )],
        )
        .unwrap();
    {
        let conn = library.conn.lock().unwrap();
        conn.execute_batch("DROP TRIGGER reader_binding_update; UPDATE reader_bindings SET reference_json='{\"kind\":\"pixiv_uploaded\",\"image_id\":\"other\"}';").unwrap();
    }
    assert!(matches!(
        library.reader_document(first.entity_id, ResourceScope::Snapshot(first.snapshot_id)),
        Err(Error::Integrity(_))
    ));
    assert!(matches!(library.verify(), Err(Error::Integrity(_))));
}

#[test]
fn reference_parsing_keeps_repeated_ids_article_arrays_and_invalid_pixiv_notation_separate() {
    let pixiv=SourceBody::PixivText("[uploadedimage:1][uploadedimage:1][pixivimage:2][pixivimage:2-2][pixivimage:2-0][uploadedimage:1-2][pixivimage:3-99999999999999]".into());
    assert_eq!(
        pixiv.asset_references().unwrap(),
        vec![
            BodyAssetReference::PixivUploaded {
                image_id: "1".into()
            },
            BodyAssetReference::PixivIllustration {
                illustration_id: "2".into(),
                page: 1
            },
            BodyAssetReference::PixivIllustration {
                illustration_id: "2".into(),
                page: 2
            }
        ]
    );
    let article = SourceBody::FanboxBody(
        json!({"images":[{"id":"same"},{"id":"same"}],"files":[{}],"imageMap":{"a/b~c":{}},"blocks":[{"type":"file","fileId":"missing-map-entry"}],"unknown":{"retain":true}}),
    );
    assert_eq!(article.asset_references().unwrap().len(), 5);
    assert!(SourceBody::FanboxBody(json!({"imageMap":[]}))
        .asset_references()
        .is_err());
    assert!(SourceBody::FanboxBody(json!({"blocks":"unrecognized"}))
        .asset_references()
        .is_err());
    assert!(SourceBody::PlainText("[pixivimage:1]".into())
        .asset_references()
        .unwrap()
        .is_empty());
    let many = SourceBody::PixivText(
        (0..=reader_references::MAX_BODY_ASSETS)
            .map(|id| format!("[uploadedimage:{id}]"))
            .collect::<String>(),
    );
    assert!(matches!(
        many.asset_references(),
        Err(Error::Unsupported(_))
    ));
}
