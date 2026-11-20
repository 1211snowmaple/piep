use super::*;
use std::fs;

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn pixiv(text: &str) -> Value {
    json!({
        "detail": {
            "title": "題",
            "caption": "説明",
            "create_date": "2026-01-01T00:00:00+09:00",
            "user": { "id": 77, "name": "作者" },
            "tags": [{ "name": "雨" }, { "name": "雨" }, "晴れ"],
            "text": text,
            "cover_url": "https://i.pximg.net/c/600x600/novel-cover-master/img/2026/01/01/00/00/00/ci1_abc_master1200.jpg"
        }
    })
}

#[test]
fn fields_follow_the_saved_response_shape() {
    let fields = work_fields("pixiv", &pixiv("本文[uploadedimage:1]"));
    assert_eq!(fields.metadata["content_type"], json!("novel"));
    assert_eq!(fields.metadata["author_key"], json!(77));
    assert_eq!(fields.metadata["tags"], json!(["晴れ", "雨"]));
    assert!(matches!(fields.body, Some(SourceBody::PixivText(_))));
    assert_eq!(fields.field_states["media"], "known");

    let fanbox = json!({ "title": "投稿", "type": "article", "creatorId": "c", "publishedDatetime": "2026-01-01T00:00:00+09:00", "body": null });
    let fields = work_fields("fanbox", &fanbox);
    assert_eq!(fields.metadata["content_type"], json!("article"));
    assert_eq!(fields.metadata["author_key"], json!("c"));
    assert!(fields.body.is_none());
}

#[test]
fn a_save_keeps_the_response_and_files_and_does_not_repeat_the_same_content() {
    let temp = Temp(std::env::temp_dir().join(format!("piep-work-save-{}", LibraryId::now_v7())));
    let library = Library::create(&temp.0.join("shelf")).unwrap();
    let staged = temp.0.join("staged");
    fs::create_dir_all(&staged).unwrap();
    fs::write(staged.join("cover.jpg"), b"cover").unwrap();
    fs::write(staged.join("1.png"), b"image").unwrap();
    fs::write(staged.join("a.pdf"), b"%PDF").unwrap();
    let assets = || {
        vec![
            StagedAsset {
                role: "cover".into(),
                name: "cover.jpg".into(),
                path: staged.join("cover.jpg"),
                url: None,
                mime_type: Some("image/jpeg".into()),
            },
            StagedAsset {
                role: "illustration".into(),
                name: "1.png".into(),
                path: staged.join("1.png"),
                url: Some(
                    "https://i.pximg.net/novel-cover-original/img/2026/01/01/00/00/00/1.png".into(),
                ),
                mime_type: Some("image/png".into()),
            },
            StagedAsset {
                role: "file".into(),
                name: "a.pdf".into(),
                path: staged.join("a.pdf"),
                url: None,
                mime_type: Some("application/pdf".into()),
            },
        ]
    };
    let data = pixiv("本文");
    let save = |data: &Value| {
        library
            .save_work(WorkSave {
                request: LibraryId::now_v7(),
                provider: "pixiv",
                key: "123",
                route: "save",
                data,
                assets: assets(),
            })
            .unwrap()
    };
    let first = save(&data);
    assert!(first.created_revision);
    let card = library.work_card(first.entity_id).unwrap();
    assert_eq!(card.title, "題");
    // 同じ中身をもう一度保存しても、版は増えない。
    let again = save(&data);
    assert_eq!(again.entity_id, first.entity_id);
    assert_eq!(again.snapshot_id, first.snapshot_id);
    assert!(!again.created_revision);
    // 本文が変われば改稿になる。
    let changed = save(&pixiv("直した本文"));
    assert!(changed.created_revision);
    assert_ne!(changed.snapshot_id, first.snapshot_id);
    assert_eq!(library.snapshots(first.entity_id).unwrap().len(), 2);
    library.verify().unwrap();
}

#[test]
fn a_failed_save_closes_its_acquisition_and_keeps_the_current_version() {
    let temp = Temp(std::env::temp_dir().join(format!("piep-work-save-{}", LibraryId::now_v7())));
    let library = Library::create(&temp.0.join("shelf")).unwrap();
    let data = pixiv("本文");
    let first = library
        .save_work(WorkSave {
            request: LibraryId::now_v7(),
            provider: "pixiv",
            key: "123",
            route: "save",
            data: &data,
            assets: Vec::new(),
        })
        .unwrap();
    // 保存の前に取得を始めた記録を作っておいてもよい（同じ要求なら二度目は同じ記録を返す）。
    let request = LibraryId::now_v7();
    library
        .begin_acquisition(request, "pixiv", EntityKind::Work, "123", "save")
        .unwrap();
    let missing = library.save_work(WorkSave {
        request,
        provider: "pixiv",
        key: "123",
        route: "save",
        data: &pixiv("直した本文"),
        assets: vec![StagedAsset {
            role: "cover".into(),
            name: "cover.jpg".into(),
            path: temp.0.join("ない.jpg"),
            url: None,
            mime_type: None,
        }],
    });
    assert!(missing.is_err());
    assert_eq!(
        library.acquisition_status(request).unwrap().state,
        super::super::AcquisitionState::Cancelled
    );
    assert_eq!(library.snapshots(first.entity_id).unwrap().len(), 1);
    library.verify().unwrap();
}

#[test]
fn saving_a_removed_work_again_puts_it_back_even_when_nothing_changed() {
    let temp = Temp(std::env::temp_dir().join(format!("piep-work-save-{}", LibraryId::now_v7())));
    let library = Library::create(&temp.0.join("shelf")).unwrap();
    let data = pixiv("本文");
    let save = || {
        library
            .save_work(WorkSave {
                request: LibraryId::now_v7(),
                provider: "pixiv",
                key: "123",
                route: "save",
                data: &data,
                assets: Vec::new(),
            })
            .unwrap()
    };
    let first = save();
    let generation = library
        .library_item_generation(first.entity_id)
        .unwrap()
        .unwrap();
    library
        .remove_work(LibraryId::now_v7(), first.entity_id, generation)
        .unwrap();
    assert!(library.work_card(first.entity_id).is_err());
    let again = save();
    assert!(!again.created_revision);
    assert_eq!(library.work_card(first.entity_id).unwrap().title, "題");
    library.verify().unwrap();
}
