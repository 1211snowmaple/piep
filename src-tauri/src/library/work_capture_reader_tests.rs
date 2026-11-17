use super::*;
use crate::library::{
    asset_repairs::{fanbox_expected_urls, AssetRepairObservation},
    AcquisitionState, ReaderBody, ResourceScope,
};
use std::fs;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("piep-native-reader-{}", LibraryId::now_v7()));
        fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn asset(root: &Path, name: &str, role: &str, url: Option<&str>) -> StagedAsset {
    let path = root.join(name);
    fs::write(&path, name.as_bytes()).unwrap();
    StagedAsset {
        role: role.into(),
        name: name.into(),
        path,
        url: url.map(str::to_owned),
        mime_type: None,
    }
}
fn format(library: &Library, request: LibraryId) -> String {
    let info = library.acquisition_status(request).unwrap();
    let file = library.file(info.input_manifest.unwrap()).unwrap();
    let mut bytes = Vec::new();
    library.objects.export(&file, &mut bytes).unwrap();
    serde_json::from_slice::<Value>(&bytes).unwrap()["format"]
        .as_str()
        .unwrap()
        .into()
}

#[test]
fn a_native_pixiv_save_publishes_exact_bindings_and_keeps_missing_images_explicit() {
    let temp = Temp::new();
    let library = Library::create(&temp.0.join("shelf")).unwrap();
    let data = json!({"detail":{"title":"題","user":{"id":"77","name":"作者"}},"text":"一[uploadedimage:123]二[pixivimage:9-2]三[uploadedimage:404]"});
    let assets = vec![
        asset(&temp.0, "inline_123.png", "illustration", None),
        asset(&temp.0, "illust_9_p1.jpg", "illustration", None),
        asset(
            &temp.0,
            "inline_404.png",
            "illustration",
            Some("https://s.pximg.net/common/images/limit_r18.png"),
        ),
        asset(&temp.0, "404.jpg", "cover", None),
    ];
    let request = LibraryId::now_v7();
    let save = || {
        library
            .save_readable_work(
                WorkSave {
                    request,
                    provider: "pixiv",
                    key: "1",
                    route: "save",
                    data: &data,
                    assets: assets.clone(),
                },
                None,
            )
            .unwrap()
    };
    let first = save();
    let info = library.acquisition_status(request).unwrap();
    assert_eq!(info.reader.unwrap().snapshot, first.snapshot_id);
    assert_eq!(format(&library, request), "capture-request-v3");
    let document = library
        .reader_document(first.entity_id, ResourceScope::Snapshot(first.snapshot_id))
        .unwrap();
    assert!(matches!(document.body, ReaderBody::PixivText(_)));
    assert_eq!(document.bindings.len(), 3);
    assert_eq!(
        document
            .bindings
            .iter()
            .filter(|b| b.resource.is_some())
            .count(),
        2
    );
    assert!(document.bindings.iter().any(|b| b.reference
        == super::super::BodyAssetReference::PixivUploaded {
            image_id: "404".into()
        }
        && b.resource.is_none()));
    assert_eq!(save().snapshot_id, first.snapshot_id);
    assert_eq!(library.snapshots(first.entity_id).unwrap().len(), 1);
    library.backup(&temp.0.join("backup")).unwrap();
    for asset in &assets {
        fs::remove_file(&asset.path).unwrap();
    }
    let copy = Library::open(&temp.0.join("backup")).unwrap();
    let document = copy
        .reader_document(first.entity_id, ResourceScope::Snapshot(first.snapshot_id))
        .unwrap();
    let image = document
        .bindings
        .iter()
        .find_map(|b| b.resource.as_ref())
        .unwrap();
    let mut bytes = Vec::new();
    copy.read_resource(
        first.entity_id,
        ResourceScope::Snapshot(first.snapshot_id),
        &image.reference,
        None,
        &mut bytes,
    )
    .unwrap();
    assert!(!bytes.is_empty());
    copy.verify().unwrap();
    library.verify().unwrap();
}

#[test]
fn fanbox_files_reader_readiness_and_attempts_commit_together_and_roll_back_together() {
    let temp = Temp::new();
    let library = Library::create(&temp.0.join("shelf")).unwrap();
    let data = json!({"title":"投稿","type":"article","body":{"blocks":[{"type":"p","text":"本文"},{"type":"file","fileId":"a"},{"type":"image","imageId":"missing"}],"fileMap":{"a":{"id":"a","name":"alpha","url":"https://example.invalid/a.txt"}},"imageMap":{"missing":{"id":"missing","originalUrl":"https://example.invalid/missing.jpg"}}}});
    let assets = vec![asset(
        &temp.0,
        "alpha.txt",
        "file",
        Some("https://example.invalid/a.txt"),
    )];
    let observed = AssetRepairObservation {
        expected_urls: fanbox_expected_urls(&data),
        complete: false,
    };
    let request = LibraryId::now_v7();
    let first = library
        .save_readable_work(
            WorkSave {
                request,
                provider: "fanbox",
                key: "1",
                route: "save",
                data: &data,
                assets: assets.clone(),
            },
            Some(observed.clone()),
        )
        .unwrap();
    assert_eq!(format(&library, request), "capture-request-v6");
    let info = library.acquisition_status(request).unwrap();
    assert_eq!(info.state, AcquisitionState::Complete);
    assert_eq!(info.reader.unwrap().snapshot, first.snapshot_id);
    let document = library
        .reader_document(first.entity_id, ResourceScope::Snapshot(first.snapshot_id))
        .unwrap();
    assert_eq!(document.bindings.len(), 2);
    assert_eq!(
        document
            .bindings
            .iter()
            .filter(|b| b.resource.is_some())
            .count(),
        1
    );
    assert_eq!(
        library
            .asset_repair_plan(first.entity_id, first.snapshot_id, &observed.expected_urls)
            .unwrap()
            .attempts,
        1
    );
    library.conn.lock().unwrap().execute_batch("CREATE TRIGGER reject_reader_output BEFORE INSERT ON acquisition_reader_outputs BEGIN SELECT RAISE(ABORT,'injected'); END").unwrap();
    let mut changed = data.clone();
    changed["body"]["blocks"][0]["text"] = json!("変更した本文");
    let request = LibraryId::now_v7();
    assert!(library
        .save_readable_work(
            WorkSave {
                request,
                provider: "fanbox",
                key: "1",
                route: "save",
                data: &changed,
                assets: assets.clone()
            },
            Some(observed.clone())
        )
        .is_err());
    assert_eq!(
        library.acquisition_status(request).unwrap().state,
        AcquisitionState::Cancelled
    );
    assert!(library
        .acquisition_status(request)
        .unwrap()
        .reader
        .is_none());
    assert_eq!(
        library.acquisition_status(request).unwrap().payloads.len(),
        1
    );
    assert_eq!(
        library.current_snapshot(first.entity_id).unwrap(),
        first.snapshot_id
    );
    assert_eq!(library.snapshots(first.entity_id).unwrap().len(), 1);
    assert_eq!(
        library
            .asset_repair_plan(first.entity_id, first.snapshot_id, &observed.expected_urls)
            .unwrap()
            .attempts,
        1
    );
    library.verify().unwrap();
    library.conn.lock().unwrap().execute_batch("DROP TRIGGER reject_reader_output; CREATE TRIGGER reject_asset_state BEFORE UPDATE ON asset_repair_state BEGIN SELECT RAISE(ABORT,'injected'); END").unwrap();
    let request = LibraryId::now_v7();
    assert!(library
        .save_readable_work(
            WorkSave {
                request,
                provider: "fanbox",
                key: "1",
                route: "save",
                data: &changed,
                assets
            },
            Some(observed.clone())
        )
        .is_err());
    assert_eq!(
        library.acquisition_status(request).unwrap().state,
        AcquisitionState::Cancelled
    );
    assert!(library
        .acquisition_status(request)
        .unwrap()
        .reader
        .is_none());
    assert_eq!(
        library.current_snapshot(first.entity_id).unwrap(),
        first.snapshot_id
    );
    assert_eq!(
        library
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM reader_binding_sets", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        library
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM acquisition_reader_outputs", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        library
            .asset_repair_plan(first.entity_id, first.snapshot_id, &observed.expected_urls)
            .unwrap()
            .attempts,
        1
    );
    library.verify().unwrap();
}

#[test]
fn an_ambiguous_binding_cancels_preparation_and_keeps_the_previous_readable_snapshot() {
    let temp = Temp::new();
    let library = Library::create(&temp.0.join("shelf")).unwrap();
    let first = library
        .save_readable_work(
            WorkSave {
                request: LibraryId::now_v7(),
                provider: "pixiv",
                key: "1",
                route: "save",
                data: &json!({"text":"本文"}),
                assets: vec![],
            },
            None,
        )
        .unwrap();
    let request = LibraryId::now_v7();
    let assets = vec![
        asset(&temp.0, "inline_1.png", "illustration", None),
        asset(&temp.0, "1.jpg", "illustration", None),
    ];
    assert!(library
        .save_readable_work(
            WorkSave {
                request,
                provider: "pixiv",
                key: "1",
                route: "save",
                data: &json!({"text":"[uploadedimage:1]"}),
                assets
            },
            None
        )
        .is_err());
    assert_eq!(
        library.acquisition_status(request).unwrap().state,
        AcquisitionState::Cancelled
    );
    let received = library.acquisition_status(request).unwrap().payloads;
    assert_eq!(received.len(), 1);
    let mut input = Vec::new();
    library
        .objects
        .export(&library.file(received[0].file).unwrap(), &mut input)
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&input).unwrap()["text"],
        json!("[uploadedimage:1]")
    );
    assert_eq!(
        library.current_snapshot(first.entity_id).unwrap(),
        first.snapshot_id
    );
    library
        .reader_document(first.entity_id, ResourceScope::Snapshot(first.snapshot_id))
        .unwrap();
    library.verify().unwrap();
}
