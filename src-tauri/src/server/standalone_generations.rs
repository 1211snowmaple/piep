//! 単独サーバーへ検証済みの棚の写しを置き、停止中に配信する世代を選ぶ。
use super::{
    load_config, lock_server, shipment, ReaderShelfConfig, ServerConfig, ServerPaths, CONFIG_FILE,
};
use crate::library::{protocol::Id, Library};
use piep_storage::{regular_directory, regular_file, sync_directory};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use toml_edit::{value, DocumentMut, Item, Table};
use uuid::Uuid;

pub(super) fn generations(paths: &ServerPaths) -> PathBuf {
    paths.root.join("shelf-generations")
}

pub(super) fn generation_path(paths: &ServerPaths, generation: Id) -> Result<PathBuf, String> {
    let parent = generations(paths);
    regular_directory(&parent).map_err(|error| error.to_string())?;
    Ok(parent.join(generation.get().to_string()))
}

pub(super) fn stage(paths: &ServerPaths, source: &Path) -> Result<Id, String> {
    load_config(paths)?;
    let source = Library::open(source).map_err(|error| format!("写す棚を開けません: {error}"))?;
    let parent = generations(paths);
    let server_root = paths
        .root
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if server_root.starts_with(source.root()) {
        return Err("写す棚の内側へ配信用の棚を作れません".into());
    }
    match fs::create_dir(&parent) {
        Ok(()) => sync_directory(&paths.root).map_err(|error| error.to_string())?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("配信用の置き場を作れません: {error}")),
    }
    let generation = Id::new(Uuid::now_v7()).map_err(|error| error.to_string())?;
    let destination = generation_path(paths, generation)?;
    source
        .backup(&destination)
        .map_err(|error| format!("配信用の棚を用意できません: {error}"))?;
    Ok(generation)
}

pub(super) fn activate(paths: &ServerPaths, generation: Id) -> Result<(), String> {
    let _lock = lock_server(paths)?;
    shipment::check_manual_activation(paths, generation)?;
    activate_locked(paths, generation, |_, library| {
        library
            .verify()
            .map_err(|error| format!("選んだ棚を検証できません: {error}"))?;
        Ok(())
    })
}

/// 置き場の錠を持ったまま、棚の検査や引き継ぎを済ませてから配る世代を選ぶ。
pub(super) fn activate_locked(
    paths: &ServerPaths,
    generation: Id,
    prepare: impl FnOnce(Option<&ReaderShelfConfig>, &Library) -> Result<(), String>,
) -> Result<(), String> {
    if paths
        .config_trial()
        .try_exists()
        .map_err(|error| error.to_string())?
    {
        return Err(
            "接続設定の確認が残っています。一度サーバーを起動して結果を確かめてください".into(),
        );
    }
    let path = paths.config();
    regular_file(&path).map_err(|error| error.to_string())?;
    let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let config: ServerConfig = toml::from_str(&text).map_err(|error| error.to_string())?;
    config.validate()?;
    let root = generation_path(paths, generation)?;
    let library = match &config.shelf {
        Some(settings) => Library::open_matching(&root, settings.library_id.get()),
        None => Library::open(&root),
    }
    .map_err(|error| format!("選んだ棚を開けません: {error}"))?;
    prepare(config.shelf.as_ref(), &library)?;
    if config
        .shelf
        .as_ref()
        .is_some_and(|settings| settings.generation == Some(generation))
    {
        return sync_directory(&paths.root).map_err(|error| error.to_string());
    }
    let mut document: DocumentMut = text
        .parse()
        .map_err(|error: toml_edit::TomlError| error.to_string())?;
    let settings = document
        .entry("shelf")
        .or_insert(Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or("棚の配信設定が不正です")?;
    settings.insert("library_id", value(library.id.to_string()));
    settings.insert("generation", value(generation.get().to_string()));
    // 以前の写しに戻す場合も、端末の同期の続きは新しい世代に切り替える。
    settings.insert("sync_epoch", value(Uuid::now_v7().to_string()));
    let written = document.to_string();
    let next: ServerConfig = toml::from_str(&written).map_err(|error| error.to_string())?;
    next.validate()?;
    if fs::read(&path).map_err(|error| error.to_string())? != text.as_bytes() {
        return Err("確認中に設定が変更されました。配信する棚の指定をやり直してください".into());
    }
    write_config(paths, written.as_bytes())
}

pub(super) fn write_config(paths: &ServerPaths, content: &[u8]) -> Result<(), String> {
    let temporary = paths
        .root
        .join(format!(".{CONFIG_FILE}-{}.part", Uuid::now_v7()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    let prepared = file.write_all(content).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = prepared.and_then(|()| fs::rename(&temporary, paths.config())) {
        let _ = fs::remove_file(temporary);
        return Err(format!("配信する棚の設定を確定できません: {error}"));
    }
    sync_directory(&paths.root).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::WorkSave;
    use crate::server::{store::ServerStore, tests::TestRoot};
    use serde_json::json;
    use std::sync::Arc;

    fn fixture() -> (TestRoot, ServerPaths, PathBuf) {
        let temp = TestRoot::created("v2-publish");
        let paths = ServerPaths::new(&temp.0.join("server")).unwrap();
        fs::create_dir_all(paths.state()).unwrap();
        fs::write(paths.config(), "# 設定を保つ\n[update]\nauto = false\n").unwrap();
        let source = temp.0.join("source");
        Library::create(&source).unwrap();
        (temp, paths, source)
    }

    fn add_work(source: &Path) {
        Library::open(source)
            .unwrap()
            .save_readable_work(
                WorkSave {
                    request: Uuid::now_v7(),
                    provider: "pixiv",
                    key: "1",
                    route: "test",
                    data: &json!({"detail":{"title":"配信する作品"},"text":"検証する本文"}),
                    assets: vec![],
                },
                None,
            )
            .unwrap();
    }

    #[test]
    fn publication_selects_a_verified_copy_and_rollback_uses_a_fresh_sync_epoch() {
        let (_temp, paths, source) = fixture();
        let first = stage(&paths, &source).unwrap();
        assert!(load_config(&paths).unwrap().shelf.is_none());
        add_work(&source);
        let second = stage(&paths, &source).unwrap();
        assert_ne!(first, second);
        activate(&paths, first).unwrap();
        let original = load_config(&paths).unwrap().shelf.unwrap();
        assert_eq!(original.generation, Some(first));
        let text = fs::read_to_string(paths.config()).unwrap();
        let text = format!(
            "{text}\n[shelf.readers]\nphone = \"{}\"\n",
            original.library_id.get()
        );
        fs::write(paths.config(), text).unwrap();
        let existing = lock_server(&paths).unwrap();
        assert!(activate(&paths, second).is_err());
        drop(existing);
        activate(&paths, second).unwrap();
        let next = load_config(&paths).unwrap();
        assert!(!next.update.auto);
        assert!(fs::read_to_string(paths.config())
            .unwrap()
            .contains("# 設定を保つ"));
        let next = next.shelf.unwrap();
        assert_eq!(next.generation, Some(second));
        assert_eq!(next.readers.get("phone"), Some(&original.library_id));
        assert_ne!(next.sync_epoch, original.sync_epoch);
        let store = Arc::new(ServerStore::open(&paths.store()).unwrap());
        let opened = next.open(&paths, Arc::clone(&store)).unwrap();
        assert_eq!(
            serde_json::to_value(opened.shelf_status().unwrap()).unwrap()["works"],
            "1"
        );
        drop(opened);
        activate(&paths, second).unwrap();
        assert_eq!(
            load_config(&paths).unwrap().shelf.unwrap().sync_epoch,
            next.sync_epoch
        );
        activate(&paths, first).unwrap();
        let restored = load_config(&paths).unwrap().shelf.unwrap();
        assert_eq!(restored.generation, Some(first));
        assert_ne!(restored.sync_epoch, next.sync_epoch);
        assert_ne!(restored.sync_epoch, original.sync_epoch);
        let opened = restored.open(&paths, store).unwrap();
        assert_eq!(
            serde_json::to_value(opened.shelf_status().unwrap()).unwrap()["works"],
            "0"
        );
        assert!(generation_path(&paths, second)
            .unwrap()
            .join("library.json")
            .is_file());
    }

    #[test]
    fn corrupt_or_unrelated_generations_cannot_change_the_selection() {
        let (_temp, paths, source) = fixture();
        let first = stage(&paths, &source).unwrap();
        activate(&paths, first).unwrap();
        let previous = fs::read(paths.config()).unwrap();
        let other = source.with_file_name("other");
        Library::create(&other).unwrap();
        let unrelated = stage(&paths, &other).unwrap();
        assert!(activate(&paths, unrelated).is_err());
        let broken = stage(&paths, &source).unwrap();
        fs::write(
            generation_path(&paths, broken).unwrap().join("library.db"),
            b"incomplete",
        )
        .unwrap();
        assert!(activate(&paths, broken).is_err());
        let unknown = Id::new(Uuid::now_v7()).unwrap();
        assert!(activate(&paths, unknown).is_err());
        assert_eq!(fs::read(paths.config()).unwrap(), previous);
    }
}
