use super::*;
use crate::library::{Fingerprint, ResourceScope};
use std::fs;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("piep-pixiv-supplement-{}", LibraryId::now_v7()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn image(root: &Path, name: &str, role: &str, url: &str, bytes: &[u8]) -> StagedAsset {
    let path = root.join(name);
    fs::write(&path, bytes).unwrap();
    StagedAsset {
        role: role.into(),
        name: name.into(),
        path,
        url: Some(url.into()),
        mime_type: Some("image/png".into()),
    }
}
fn save(library: &Library, data: &Value, assets: Vec<StagedAsset>) -> CaptureResult {
    library
        .save_readable_work(
            WorkSave {
                request: LibraryId::now_v7(),
                provider: "pixiv",
                key: "1",
                route: "save",
                data,
                assets,
            },
            None,
        )
        .unwrap()
}
fn data() -> Value {
    json!({"detail":{"title":"題", "caption":"説明", "tags":["一"]}, "text":"本文[uploadedimage:1][uploadedimage:2]"})
}

#[test]
fn filling_body_images_and_gate_covers_keeps_the_source_revision_and_historical_bytes() {
    let temp = Temp::new();
    let library = Library::create(&temp.0.join("shelf")).unwrap();
    let data = data();
    let gate = "https://s.pximg.net/common/images/limit_r18.png";
    let first = save(
        &library,
        &data,
        vec![
            image(&temp.0, "cover.png", "cover", gate, b"gate cover"),
            image(&temp.0, "inline_1.png", "illustration", gate, b"gate image"),
        ],
    );
    let supplemented = save(
        &library,
        &data,
        vec![
            image(
                &temp.0,
                "cover.png",
                "cover",
                "https://example.com/cover.png",
                b"cover",
            ),
            image(
                &temp.0,
                "inline_1.png",
                "illustration",
                "https://example.com/1.png",
                b"one",
            ),
            image(
                &temp.0,
                "inline_2.png",
                "illustration",
                "https://example.com/2.png",
                b"two",
            ),
        ],
    );
    assert!(!supplemented.created_revision);
    assert_ne!(first.snapshot_id, supplemented.snapshot_id);
    let snapshots = library.snapshots(first.entity_id).unwrap();
    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].revision_id, snapshots[1].revision_id);
    let before = library
        .reader_document(first.entity_id, ResourceScope::Snapshot(first.snapshot_id))
        .unwrap();
    assert!(before.bindings.iter().all(|b| b.resource.is_none()));
    let after = library
        .reader_document(
            first.entity_id,
            ResourceScope::Snapshot(supplemented.snapshot_id),
        )
        .unwrap();
    assert!(after.bindings.iter().all(|b| b.resource.is_some()));
    let mut old = Vec::new();
    library
        .read_resource(
            first.entity_id,
            ResourceScope::Snapshot(first.snapshot_id),
            &ResourceReference::Media {
                slot: "cover".into(),
            },
            None,
            &mut old,
        )
        .unwrap();
    assert_eq!(old, b"gate cover");
    let new = library
        .resource_manifest(
            first.entity_id,
            ResourceScope::Snapshot(supplemented.snapshot_id),
        )
        .unwrap();
    assert!(new.resources.iter().any(|r| r
        .file
        .as_ref()
        .is_some_and(|f| f.id == Fingerprint::of(b"one"))));
    library.backup(&temp.0.join("backup")).unwrap();
    Library::open(&temp.0.join("backup"))
        .unwrap()
        .verify()
        .unwrap();
}

#[test]
fn supplementing_an_image_never_hides_other_source_changes_or_asset_removals() {
    for change in ["title", "body", "image", "remove", "unreferenced"] {
        let temp = Temp::new();
        let library = Library::create(&temp.0.join("shelf")).unwrap();
        let mut data = data();
        let held = || {
            image(
                &temp.0,
                "inline_1.png",
                "illustration",
                "https://example.com/1.png",
                b"one",
            )
        };
        save(&library, &data, vec![held()]);
        let mut assets = vec![
            held(),
            image(
                &temp.0,
                "inline_2.png",
                "illustration",
                "https://example.com/2.png",
                b"two",
            ),
        ];
        match change {
            "title" => data["detail"]["title"] = json!("変えた題"),
            "body" => data["text"] = json!("別の本文[uploadedimage:1][uploadedimage:2]"),
            "image" => {
                assets[0] = image(
                    &temp.0,
                    "inline_1.png",
                    "illustration",
                    "https://example.com/replaced.png",
                    b"replaced",
                )
            }
            "remove" => {
                assets.remove(0);
            }
            "unreferenced" => assets.push(image(
                &temp.0,
                "other.png",
                "illustration",
                "https://example.com/other.png",
                b"other",
            )),
            _ => unreachable!(),
        }
        assert!(save(&library, &data, assets).created_revision, "{change}");
        library.verify().unwrap();
    }
}

#[test]
fn receiving_gate_images_does_not_claim_the_author_added_images_or_that_they_are_readable() {
    let temp = Temp::new();
    let library = Library::create(&temp.0.join("shelf")).unwrap();
    let data = data();
    let first = save(&library, &data, vec![]);
    let next = save(
        &library,
        &data,
        vec![
            image(
                &temp.0,
                "cover.png",
                "cover",
                "https://s.pximg.net/common/images/limit_r18.png",
                b"gate cover",
            ),
            image(
                &temp.0,
                "inline_1.png",
                "illustration",
                "https://s.pximg.net/common/images/limit_r18.png",
                b"gate image",
            ),
        ],
    );
    assert!(!next.created_revision);
    assert_ne!(first.snapshot_id, next.snapshot_id);
    let document = library
        .reader_document(first.entity_id, ResourceScope::Snapshot(next.snapshot_id))
        .unwrap();
    assert!(document.bindings.iter().all(|b| b.resource.is_none()));
    library.verify().unwrap();
}
