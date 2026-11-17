use super::*;
use crate::library::{
    Capture, CapturedMedia, EditBlock, EditContent, EditRequest, EntityKind, Fingerprint,
    ReaderBinding,
};
use std::path::PathBuf;

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn temp() -> Temp {
    Temp(std::env::temp_dir().join(format!("piep-snapshot-view-{}", LibraryId::now_v7())))
}
fn capture(key: &str, title: &str, body: Option<SourceBody>, image: Option<&[u8]>) -> Capture {
    Capture {
        request_id: LibraryId::now_v7(),
        acquired_at: None,
        provider: "pixiv".into(),
        external_key: key.into(),
        kind: EntityKind::Work,
        route: "test".into(),
        raw: br#"{"createDate":"2026-10-01T09:00:00+09:00"}"#.to_vec().into(),
        metadata: BTreeMap::from([
            ("title".into(), json!(title)),
            ("caption".into(), json!(format!("{title}の説明"))),
            ("tags".into(), json!([title])),
            ("author_name".into(), json!(format!("{title}の作者"))),
            ("author_key".into(), json!("77")),
            ("series_title".into(), json!(format!("{title}のシリーズ"))),
            ("series_id".into(), json!("88")),
        ]),
        field_states: BTreeMap::from([
            ("media".into(), "known".into()),
            ("attachments".into(), "known".into()),
        ]),
        body,
        media: image
            .map(|bytes| CapturedMedia {
                slot: "picture".into(),
                role: "illustration".into(),
                source_key: Some("123".into()),
                source_url: None,
                quality: "original".into(),
                original_name: "picture.png".into(),
                mime_type: "image/png".into(),
                bytes: Some(bytes.to_vec().into()),
            })
            .into_iter()
            .collect(),
        attachments: vec![],
    }
}
fn bindings(library: &Library, work: LibraryId, snapshot: LibraryId, present: bool) {
    library
        .publish_reader_bindings(
            work,
            snapshot,
            vec![ReaderBinding {
                reference: BodyAssetReference::PixivUploaded {
                    image_id: "123".into(),
                },
                resource: present.then(|| ResourceReference::Media {
                    slot: "picture".into(),
                }),
            }],
        )
        .unwrap();
}

#[test]
fn selected_source_metadata_and_bytes_never_follow_current_or_active_edit() {
    let dir = temp();
    let library = Library::create(&dir.0).unwrap();
    let body = Some(SourceBody::PixivText("同じ本文[uploadedimage:123]".into()));
    let first = library
        .capture(capture("one", "最初", body.clone(), Some(b"old")))
        .unwrap();
    bindings(&library, first.entity_id, first.snapshot_id, true);
    let content = EditContent {
        format: "piep-edit-v1".into(),
        title: Some("編集タイトル".into()),
        blocks: vec![EditBlock {
            id: LibraryId::now_v7(),
            ordinal: 0,
            kind: "paragraph".into(),
            text: Some("編集した本文".into()),
            attrs: Value::Null,
            media_slot: None,
        }],
        media: vec![],
        provenance: None,
    };
    let edit = library
        .save_edit(EditRequest {
            request_id: LibraryId::now_v7(),
            work: first.entity_id,
            user: library.id,
            base: first.snapshot_id,
            parent: None,
            expected: 0,
            content: &content,
            activate: true,
        })
        .unwrap();
    let mut next = capture("one", "次", body, Some(b"new"));
    next.metadata.insert("create_date".into(), json!(0));
    let second = library.capture(next).unwrap();
    bindings(&library, second.entity_id, second.snapshot_id, true);
    let generation = library
        .edit_state(first.entity_id, library.id)
        .unwrap()
        .generation;
    let old = library
        .work_revision_snapshot(first.entity_id, first.snapshot_id)
        .unwrap();
    let new = library
        .work_revision_snapshot(second.entity_id, second.snapshot_id)
        .unwrap();
    assert_eq!(old.text, new.text);
    assert_eq!(old.metadata.title.as_deref(), Some("最初"));
    assert_eq!(old.metadata.caption.as_deref(), Some("最初の説明"));
    assert_eq!(old.metadata.author.as_deref(), Some("最初の作者（77）"));
    assert_eq!(old.metadata.series.as_deref(), Some("最初のシリーズ（88）"));
    assert_eq!(old.metadata.tags, Some(vec!["最初".into()]));
    assert_eq!(
        old.metadata.published_at.as_deref(),
        Some("2026-10-01T09:00:00+09:00")
    );
    assert_eq!(
        new.metadata.published_at.as_deref(),
        Some("1970-01-01T00:00:00+00:00")
    );
    assert_eq!(new.metadata.title.as_deref(), Some("次"));
    let old_image = &old.metadata.assets[0];
    assert_eq!(old_image.key, "pixiv-uploaded:123");
    assert_eq!(
        old_image.content_hash,
        Some(Fingerprint::of(b"old").to_string())
    );
    assert_eq!(
        old_image.local_path,
        Some(format!("shelf:{}", Fingerprint::of(b"old")))
    );
    assert_eq!(
        new.metadata.assets[0].content_hash,
        Some(Fingerprint::of(b"new").to_string())
    );
    assert!(!serde_json::to_string(&old)
        .unwrap()
        .contains(&dir.0.to_string_lossy().to_string()));
    assert_eq!(
        library
            .edit_state(first.entity_id, library.id)
            .unwrap()
            .generation,
        generation
    );
    assert!(matches!(
        library.work_revision_snapshot(first.entity_id, edit),
        Err(Error::ResourceUnavailable)
    ));
    let other = library
        .capture(capture("two", "別の作品", None, None))
        .unwrap();
    assert!(matches!(
        library.work_revision_snapshot(other.entity_id, first.snapshot_id),
        Err(Error::ResourceUnavailable)
    ));
}

#[test]
fn missing_body_and_unprepared_bindings_are_unknown_not_empty_or_removed() {
    let dir = temp();
    let library = Library::create(&dir.0).unwrap();
    let mut input = capture("one", "未確認", None, None);
    input
        .field_states
        .insert("caption".into(), "unknown".into());
    input
        .field_states
        .insert("create_date".into(), "unknown".into());
    let absent = library.capture(input).unwrap();
    let view = library
        .work_revision_snapshot(absent.entity_id, absent.snapshot_id)
        .unwrap();
    assert_eq!(view.text, None);
    assert_eq!(view.metadata.caption, None);
    assert_eq!(
        view.metadata.published_at, None,
        "unknown must not fall back to raw JSON"
    );
    assert_eq!(view.metadata.assets_known, Some(false));
    let empty = library
        .capture(capture(
            "empty",
            "空",
            Some(SourceBody::PixivText("".into())),
            None,
        ))
        .unwrap();
    assert_eq!(
        library
            .work_revision_snapshot(empty.entity_id, empty.snapshot_id)
            .unwrap()
            .text
            .as_deref(),
        Some("")
    );
    let unprepared = library
        .capture(capture(
            "image",
            "画像",
            Some(SourceBody::PixivText("[uploadedimage:123]".into())),
            Some(b"image"),
        ))
        .unwrap();
    let before = library
        .work_revision_snapshot(unprepared.entity_id, unprepared.snapshot_id)
        .unwrap();
    assert_eq!(before.metadata.assets_known, Some(false));
    assert_eq!(
        before
            .metadata
            .assets
            .iter()
            .find(|a| a.key == "pixiv-uploaded:123")
            .unwrap()
            .available,
        None
    );
    bindings(&library, unprepared.entity_id, unprepared.snapshot_id, true);
    let prepared = library
        .work_revision_snapshot(unprepared.entity_id, unprepared.snapshot_id)
        .unwrap();
    assert_eq!(prepared.metadata.assets_known, Some(true));
    assert_eq!(prepared.metadata.assets.len(), 1);
    assert_eq!(prepared.metadata.assets[0].available, Some(true));
    let missing = library
        .capture(capture(
            "missing",
            "欠け",
            Some(SourceBody::PixivText("[uploadedimage:123]".into())),
            None,
        ))
        .unwrap();
    bindings(&library, missing.entity_id, missing.snapshot_id, false);
    let view = library
        .work_revision_snapshot(missing.entity_id, missing.snapshot_id)
        .unwrap();
    assert_eq!(view.metadata.assets_known, Some(true));
    assert_eq!(view.metadata.assets[0].key, prepared.metadata.assets[0].key);
    assert_eq!(view.metadata.assets[0].available, Some(false));
    assert_eq!(view.metadata.assets[0].content_hash, None);
}

#[test]
fn article_structure_and_asset_sources_come_from_each_selected_body() {
    let dir = temp();
    let library = Library::create(&dir.0).unwrap();
    let article = |url: &str| {
        SourceBody::FanboxBody(json!({
            "blocks": [
                {"type":"p", "text":"同じ文章", "links":[{"url":url,"offset":0,"length":2}]},
                {"type":"image", "imageId":"img"}
            ],
            "imageMap":{"img":{"originalUrl":"https://example.com/image.png"}}
        }))
    };
    let save = |url: &str| {
        let mut input = capture("article", "記事", Some(article(url)), Some(b"same"));
        input.provider = "fanbox".into();
        let saved = library.capture(input).unwrap();
        library
            .publish_reader_bindings(
                saved.entity_id,
                saved.snapshot_id,
                vec![ReaderBinding {
                    reference: BodyAssetReference::ArticleImage { key: "img".into() },
                    resource: Some(ResourceReference::Media {
                        slot: "picture".into(),
                    }),
                }],
            )
            .unwrap();
        saved
    };
    let first = save("https://example.com/old");
    let second = save("https://example.com/new");
    let old = library
        .work_revision_snapshot(first.entity_id, first.snapshot_id)
        .unwrap();
    let new = library
        .work_revision_snapshot(second.entity_id, second.snapshot_id)
        .unwrap();
    assert_eq!(old.text, new.text);
    assert!(old
        .metadata
        .body_structure
        .unwrap()
        .contains("https://example.com/old"));
    assert!(new
        .metadata
        .body_structure
        .unwrap()
        .contains("https://example.com/new"));
    assert_eq!(old.metadata.assets[0].key, "article-image:img");
    assert_eq!(
        old.metadata.assets[0].source,
        "https://example.com/image.png"
    );
    assert_eq!(
        old.metadata.assets[0].content_hash,
        new.metadata.assets[0].content_hash
    );
}
