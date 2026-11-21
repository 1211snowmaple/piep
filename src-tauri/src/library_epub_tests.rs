use super::*;
use crate::library::{
    CaptureSource, EditBlock, EditContent, EditMedia, EditRequest, StagedAsset, WorkSave,
};
use serde_json::json;

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn png(path: &Path, shade: u8) {
    image::RgbImage::from_pixel(2, 2, image::Rgb([shade, 20, 30]))
        .save(path)
        .unwrap();
}

/// 表紙と挿絵を一枚ずつ持つ pixiv の小説。本文は挿絵を一枚指す。
fn shelf() -> (Temp, Library, LibraryId, LibraryId) {
    let root = std::env::temp_dir().join(format!("piep-epub-shelf-{}", LibraryId::now_v7()));
    let library = Library::create(&root.join("shelf")).unwrap();
    let staged = root.join("staged");
    std::fs::create_dir_all(&staged).unwrap();
    png(&staged.join("cover.png"), 1);
    png(&staged.join("100.png"), 2);
    let saved = library
        .save_work(WorkSave {
            request: LibraryId::now_v7(),
            provider: "pixiv",
            key: "7",
            route: "test",
            data: &json!({
                "detail": {
                    "id": "7",
                    "title": "棚の本",
                    "userId": "10",
                    "userName": "作者",
                    "content": "はじまり\n[uploadedimage:100]\nおわり",
                    "textEmbeddedImages": {"100": {"urls": {"original": "https://i.pximg.net/novel-cover-original/img/100.png"}}},
                    "createDate": "2026-01-01T00:00:00+09:00",
                },
                "text": "はじまり\n[uploadedimage:100]\nおわり",
            }),
            assets: vec![
                StagedAsset {
                    role: "cover".into(),
                    name: "cover.png".into(),
                    path: staged.join("cover.png"),
                    url: None,
                    mime_type: Some("image/png".into()),
                },
                StagedAsset {
                    role: "illustration".into(),
                    name: "100.png".into(),
                    path: staged.join("100.png"),
                    url: None,
                    mime_type: Some("image/png".into()),
                },
            ],
        })
        .unwrap();
    (Temp(root), library, saved.entity_id, saved.snapshot_id)
}

#[test]
fn the_saved_response_and_images_are_laid_out_like_the_old_shelf() {
    let (_temp, library, work, _) = shelf();
    let source = library.epub_source(work).unwrap();
    assert_eq!(source.provider, "pixiv");
    assert_eq!(source.title, "棚の本");
    assert!(source.edit.is_none());
    assert_eq!(source.data["detail"]["title"], "棚の本");
    let assets = source.staging.assets_dir();
    assert!(assets.join("illustrations").join("100.png").is_file());
    assert_eq!(
        source.cover.as_deref(),
        Some(assets.join("cover").join("cover.png").as_path())
    );

    // 手放すと一時の置き場は消える。
    let root = source.staging.root.clone();
    drop(source);
    assert!(!root.exists());
}

#[test]
fn the_book_has_the_illustration_and_the_cover() {
    let (_temp, library, work, _) = shelf();
    let (manifest, provider, title, _staging) =
        crate::commands::epub::load_shelf_manifest(&library, work).unwrap();
    assert_eq!((provider.as_str(), title.as_str()), ("pixiv", "棚の本"));
    assert!(manifest.content.cover_image.is_some());
    let html: String = manifest
        .content
        .pages
        .iter()
        .map(|p| p.html_content.clone())
        .collect();
    assert!(
        html.contains("はじまり") && html.contains("おわり"),
        "{html}"
    );
}

#[test]
fn an_applied_edit_replaces_the_body_and_brings_its_added_image() {
    let (temp, library, work, snapshot) = shelf();
    let added = temp.0.join("added.png");
    png(&added, 9);
    let file = library.store_original(&CaptureSource::File(added)).unwrap();
    // 版の挿絵を指す画像は、同じファイルなら取得元の画像として扱う。
    let source_image = library
        .store_original(&CaptureSource::File(temp.0.join("staged").join("100.png")))
        .unwrap();
    let block = |ordinal: i64,
                 kind: &str,
                 text: Option<&str>,
                 slot: Option<&str>,
                 attrs: serde_json::Value| EditBlock {
        id: LibraryId::now_v7(),
        ordinal,
        kind: kind.into(),
        text: text.map(str::to_owned),
        attrs,
        media_slot: slot.map(str::to_owned),
    };
    let content = EditContent {
        format: "piep-edit-v1".into(),
        title: None,
        provenance: None,
        blocks: vec![
            block(
                0,
                "paragraph",
                Some("書き直した"),
                None,
                serde_json::Value::Null,
            ),
            block(
                1,
                "image",
                None,
                Some("old"),
                json!({"sourceRef": "[uploadedimage:100]"}),
            ),
            block(2, "image", None, Some("new"), serde_json::Value::Null),
        ],
        media: vec![
            EditMedia {
                slot: "old".into(),
                file: source_image.id,
                original_name: "100.png".into(),
                mime_type: Some("image/png".into()),
            },
            EditMedia {
                slot: "new".into(),
                file: file.id,
                original_name: "added.png".into(),
                mime_type: Some("image/png".into()),
            },
        ],
    };
    library
        .save_edit(EditRequest {
            request_id: LibraryId::now_v7(),
            work,
            user: library.id,
            base: snapshot,
            parent: None,
            expected: 0,
            content: &content,
            activate: true,
        })
        .unwrap();

    let source = library.epub_source(work).unwrap();
    let edit = source.edit.as_ref().unwrap();
    assert!(
        edit.pixiv_text.starts_with("書き直した"),
        "{}",
        edit.pixiv_text
    );
    assert!(
        edit.pixiv_text.contains("[uploadedimage:100]"),
        "{}",
        edit.pixiv_text
    );
    assert_eq!(edit.added_images.len(), 1);
    let added = &edit.added_images[0];
    assert!(Path::new(&added.local_path).is_file());
    assert!(edit.pixiv_text.contains(&format!(
        "[uploadedimage:{}]",
        Path::new(&added.filename)
            .file_stem()
            .unwrap()
            .to_str()
            .unwrap()
    )));
    drop(source);

    let (manifest, _, _, _staging) =
        crate::commands::epub::load_shelf_manifest(&library, work).unwrap();
    let html: String = manifest
        .content
        .pages
        .iter()
        .map(|p| p.html_content.clone())
        .collect();
    assert!(
        html.contains("書き直した") && !html.contains("はじまり"),
        "{html}"
    );
    assert!(manifest
        .content
        .illustrations
        .iter()
        .any(|image| image.local_path == added_path(&manifest)));
}

fn added_path(manifest: &crate::epub::intermediate::EpubManifest) -> String {
    manifest
        .content
        .illustrations
        .iter()
        .find(|image| image.local_path.contains("edit-images"))
        .map(|image| image.local_path.clone())
        .unwrap_or_default()
}

#[test]
fn the_queue_sends_shelf_ids_and_refuses_old_numbers() {
    use crate::epub::intermediate::EpubWork;
    let id = LibraryId::now_v7();
    let works: Vec<EpubWork> = serde_json::from_value(json!([id.to_string()])).unwrap();
    assert_eq!(works, vec![id]);
    assert_eq!(
        serde_json::to_value(&works).unwrap(),
        json!([id.to_string()])
    );
    assert!(serde_json::from_value::<Vec<EpubWork>>(json!([7])).is_err());
}
