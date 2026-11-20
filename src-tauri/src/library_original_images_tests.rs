use super::*;
use crate::library::{StagedAsset, WorkSave};
use serde_json::json;
use std::path::{Path, PathBuf};

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

const MASTER: &str =
    "https://i.pximg.net/c/600x600/novel-cover-master/img/2024/01/01/00/00/00/ci7_abc_master1200.jpg";
const ORIGINAL: &str =
    "https://i.pximg.net/novel-cover-original/img/2024/01/01/00/00/00/ci7_abc.png";

fn data() -> serde_json::Value {
    json!({
        "detail": {
            "id": "7",
            "title": "棚の本",
            "userId": "10",
            "userName": "作者",
            "content": "本文",
            "cover_url": MASTER,
            "createDate": "2026-01-01T00:00:00+09:00",
        },
        "text": "本文",
        "cover_url": MASTER,
    })
}

fn save(
    library: &Library,
    data: &serde_json::Value,
    cover: StagedAsset,
) -> crate::library::CaptureResult {
    library
        .save_readable_work(
            WorkSave {
                request: LibraryId::now_v7(),
                provider: "pixiv",
                key: "7",
                route: "save",
                data,
                assets: vec![cover],
            },
            None,
        )
        .unwrap()
}

#[test]
fn a_reduced_cover_is_found_and_the_original_replaces_it_without_a_new_version() {
    let root = std::env::temp_dir().join(format!("piep-originals-{}", LibraryId::now_v7()));
    let _temp = Temp(root.clone());
    let library = Library::create(&root.join("shelf")).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    let reduced = root.join("ci7_abc_master1200.jpg");
    png(&reduced, 1);
    let first = save(
        &library,
        &data(),
        StagedAsset {
            role: "cover".into(),
            name: "ci7_abc_master1200.jpg".into(),
            path: reduced,
            url: Some(MASTER.into()),
            mime_type: Some("image/png".into()),
        },
    );
    assert_eq!(
        library.reduced_pixiv_works().unwrap(),
        vec![ReducedShelfWork {
            work: first.entity_id,
            covers: 1,
            illustrations: 0
        }]
    );

    // 保存済みの応答に原寸の在り処を足し、原寸の画像で保存し直す。
    let mut upgraded = data();
    upgraded[crate::downloader::pixiv_originals::COVER_ORIGINAL_FIELD] = json!(ORIGINAL);
    let original = root.join("ci7_abc.png");
    png(&original, 2);
    let second = save(
        &library,
        &upgraded,
        StagedAsset {
            role: "cover".into(),
            name: "ci7_abc.png".into(),
            path: original.clone(),
            url: Some(ORIGINAL.into()),
            mime_type: Some("image/png".into()),
        },
    );
    assert_eq!(second.entity_id, first.entity_id);
    assert!(!second.created_revision, "画質の違いで版を増やさない");
    assert_eq!(
        library.work_card(first.entity_id).unwrap().version_number,
        1
    );
    assert_eq!(
        library
            .media_file(first.entity_id, second.snapshot_id, "cover")
            .unwrap()
            .id,
        crate::library::Fingerprint::of(&std::fs::read(&original).unwrap())
    );
    assert!(library.reduced_pixiv_works().unwrap().is_empty());
    library.verify().unwrap();
}
