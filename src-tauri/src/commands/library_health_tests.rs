use super::*;

#[test]
fn the_semantic_setting_is_written_whole_and_read_back() {
    let dir = std::env::temp_dir().join(format!("piep-search-settings-{}", LibraryId::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    set_settings_dir(&dir);
    // ほかの試験が先に別のフォルダーを決めていれば、そちらで確かめる。
    let path = SETTINGS_PATH.get().unwrap().clone();
    let _ = std::fs::remove_file(&path);
    assert!(!semantic_enabled());
    store_semantic_enabled(true).unwrap();
    assert!(semantic_enabled());
    store_semantic_enabled(false).unwrap();
    assert!(!semantic_enabled());
    assert!(!path.with_extension("json.part").exists());
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(&dir);
}
