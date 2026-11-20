use super::*;
use crate::library::LibraryId;
use serde_json::json;

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn png(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    image::RgbImage::from_pixel(1, 1, image::Rgb([12, 34, 56]))
        .save(path)
        .unwrap();
}

fn version(storage: &Path, key: &str, number: u32, text: &str) -> PathBuf {
    let dir = storage.join("pixiv").join(key).join(format!("v{number}"));
    std::fs::create_dir_all(&dir).unwrap();
    let data = json!({"detail":{"title":format!("題 {key}"), "caption":"説明", "tags":["タグ"],
        "user":{"id":"9","name":"作者"}}, "text":text});
    std::fs::write(dir.join("data.json"), serde_json::to_vec(&data).unwrap()).unwrap();
    png(&dir.join("data_assets").join("cover").join("cover.png"));
    dir
}

#[tokio::test]
async fn folders_never_on_the_shelf_come_in_with_their_versions_and_others_are_left_alone() {
    let temp = Temp(std::env::temp_dir().join(format!("piep-reimport-{}", LibraryId::now_v7())));
    let storage = temp.0.join("downloads");
    let library = Arc::new(Library::create(&temp.0.join("shelf")).unwrap());
    version(&storage, "1", 1, "最初の本文");
    version(&storage, "1", 2, "直した本文");
    version(&storage, "2", 1, "外した作品");
    // 中身の読めない版は、その版だけ飛ばして理由を返す。
    std::fs::create_dir_all(storage.join("pixiv").join("3").join("v1")).unwrap();
    // 番号でない名前・作品でない名前は数えない。
    std::fs::create_dir_all(storage.join("pixiv").join("a b").join("v1")).unwrap();
    std::fs::create_dir_all(storage.join("pixiv").join("4").join("draft")).unwrap();

    let found = folder_works(&storage).unwrap();
    assert_eq!(
        found.iter().map(|w| w.key.as_str()).collect::<Vec<_>>(),
        ["1", "2", "3"]
    );
    assert_eq!(found[0].versions.len(), 2);

    // 作品 2 は棚に入れてから外した。外した作品は戻さない。
    crate::commands::downloader::shelf::save_folder_to_shelf(
        Arc::clone(&library),
        version_data("pixiv", &found[1].versions[0]).unwrap(),
        "pixiv".into(),
        "2".into(),
        found[1].versions[0].join("data_assets"),
    )
    .await
    .unwrap();
    let removed = library.work_by_key("pixiv", "2").unwrap().unwrap();
    library
        .remove_work(LibraryId::now_v7(), removed, 1)
        .unwrap();

    let outcome = reimport_into_shelf(Arc::clone(&library), storage.clone())
        .await
        .unwrap();
    assert_eq!(outcome.imported, 1);
    assert_eq!(outcome.skipped.len(), 1);
    assert!(outcome.skipped[0].starts_with("pixiv/3"));
    let work = library.work_by_key("pixiv", "1").unwrap().unwrap();
    // 版は番号の順に積まれ、いまの版は最後の版。表紙もフォルダーから入る。
    let card = library.work_card(work).unwrap();
    assert_eq!(card.version_number, 2);
    assert_eq!(card.title, "題 1");
    let works = library.works(10).unwrap();
    assert_eq!(works.len(), 1, "外した作品は戻さない");
    library.verify().unwrap();

    // 二度目は何もしない。
    let again = reimport_into_shelf(Arc::clone(&library), storage)
        .await
        .unwrap();
    assert_eq!(again.imported, 0);
}
