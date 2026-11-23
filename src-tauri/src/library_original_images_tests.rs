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
        library.reduced_works().unwrap(),
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
    assert!(library.reduced_works().unwrap().is_empty());
    library.verify().unwrap();
}

/// うごイラの止め絵（ページ番号の無い master 版）には原寸の止め絵が無いので、縮小版に数えない。
/// ページ番号のあるイラストは数える。
#[test]
fn an_ugoira_still_is_not_counted_as_a_reduced_illustration() {
    let root = std::env::temp_dir().join(format!("piep-originals-{}", LibraryId::now_v7()));
    let _temp = Temp(root.clone());
    let library = Library::create(&root.join("shelf")).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    let saved = |key: &str, url: &str| {
        let path = root.join(format!("{key}.png"));
        png(&path, 3);
        let mut data = data();
        data["text"] = json!("本文[pixivimage:1]");
        library
            .save_readable_work(
                WorkSave {
                    request: LibraryId::now_v7(),
                    provider: "pixiv",
                    key,
                    route: "save",
                    data: &data,
                    assets: vec![StagedAsset {
                        role: "illustration".into(),
                        name: "1.png".into(),
                        path,
                        url: Some(url.into()),
                        mime_type: Some("image/png".into()),
                    }],
                },
                None,
            )
            .unwrap()
            .entity_id
    };
    saved(
        "1",
        "https://i.pximg.net/img-master/img/2020/06/26/20/07/13/82576470_master1200.jpg",
    );
    let paged = saved(
        "2",
        "https://i.pximg.net/img-master/img/2020/06/26/20/07/13/82576471_p0_master1200.jpg",
    );
    let reduced = library.reduced_works().unwrap();
    assert_eq!(
        reduced
            .iter()
            .filter(|work| work.illustrations > 0)
            .map(|work| work.work)
            .collect::<Vec<_>>(),
        vec![paged]
    );
}

const FANBOX_REMADE: &str =
    "https://pixiv.pximg.net/c/1200x630_90_a2_g5/fanbox/public/images/post/9/cover/abc.jpeg";
const FANBOX_ORIGINAL: &str = "https://pixiv.pximg.net/fanbox/public/images/post/9/cover/abc.jpeg";

#[test]
fn a_remade_fanbox_cover_is_found_and_the_original_replaces_it_as_the_same_image() {
    let root = std::env::temp_dir().join(format!("piep-originals-{}", LibraryId::now_v7()));
    let _temp = Temp(root.clone());
    let library = Library::create(&root.join("shelf")).unwrap();
    let post = json!({
        "id": "9", "title": "投稿", "type": "text", "creatorId": "c", "user": {"userId": "1", "name": "作者"},
        "publishedDatetime": "2026-01-01T00:00:00+09:00", "updatedDatetime": "2026-01-01T00:00:00+09:00",
        "coverImageUrl": FANBOX_REMADE, "body": {"text": "本文"},
    });
    let save = |data: &serde_json::Value, name: &str, url: &str, shade: u8| {
        let path = root.join(name);
        png(&path, shade);
        library
            .save_readable_work(
                WorkSave {
                    request: LibraryId::now_v7(),
                    provider: "fanbox",
                    key: "9",
                    route: "save",
                    data,
                    assets: vec![StagedAsset {
                        role: "cover".into(),
                        name: "abc.jpeg".into(),
                        path,
                        url: Some(url.into()),
                        mime_type: Some("image/png".into()),
                    }],
                },
                None,
            )
            .unwrap()
    };
    let first = save(&post, "remade.png", FANBOX_REMADE, 1);
    assert_eq!(
        library.reduced_works().unwrap(),
        vec![ReducedShelfWork {
            work: first.entity_id,
            covers: 1,
            illustrations: 0
        }]
    );
    let mut upgraded = post.clone();
    upgraded[crate::downloader::pixiv_originals::COVER_ORIGINAL_FIELD] = json!(FANBOX_ORIGINAL);
    let second = save(&upgraded, "original.png", FANBOX_ORIGINAL, 2);
    assert_eq!(second.entity_id, first.entity_id);
    assert!(!second.created_revision, "画質の違いで版を増やさない");
    assert!(library.reduced_works().unwrap().is_empty());
    let resources = library
        .resource_manifest(
            first.entity_id,
            crate::library::ResourceScope::Snapshot(second.snapshot_id),
        )
        .unwrap()
        .resources;
    let cover = resources.iter().find(|r| r.quality.is_some()).unwrap();
    assert_eq!(cover.quality.as_deref(), Some("original"));
    library.verify().unwrap();
}
