//! 保存フォルダーの再取り込みを新しい棚で行う。
//!
//! 旧棚の保存先（`<取得元>/<作品の番号>/v<版>/`）に残った作品のうち、棚にまだ一度も入っていない
//! ものを、取り寄せずにフォルダーの中身のまま棚へ保存する。版は番号の順に保存するので、棚の版の
//! 記録も同じ順に並ぶ。棚にある作品・棚から外した作品には触れない（外した作品を勝手に戻さない）。
//! 保存フォルダーを手で組んで戻す使い方（削除された作品を EPUB から戻すなど）も同じ道を通る。

use crate::commands::archive::{
    archive_metadata_is_link, canonical_reimport_directory, canonical_reimport_json,
    ReimportOutcome, MAX_REIMPORT_VERSIONS_PER_WORK,
};
use crate::commands::library_v2::LibraryV2;
use crate::library::{EntityKind, Library};
use crate::AppState;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::Manager;

/// 保存フォルダーに残った一作品（版は番号の順）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FolderWork {
    pub source: String,
    pub key: String,
    pub versions: Vec<PathBuf>,
}

fn safe_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// 保存先の作品のフォルダーを数える。リンクや保存先の外を指すフォルダーは断る（旧棚と同じ）。
pub(crate) fn folder_works(storage: &Path) -> Result<Vec<FolderWork>, String> {
    if !storage.try_exists().map_err(|e| e.to_string())? {
        return Ok(Vec::new());
    }
    let metadata = std::fs::symlink_metadata(storage).map_err(|e| e.to_string())?;
    if archive_metadata_is_link(&metadata) || !metadata.file_type().is_dir() {
        return Err("保存先がフォルダーではありません".into());
    }
    let storage = storage.canonicalize().map_err(|e| e.to_string())?;
    let mut works = Vec::new();
    for source in ["pixiv", "fanbox"] {
        let source_path = storage.join(source);
        if !source_path.try_exists().map_err(|e| e.to_string())? {
            continue;
        }
        let source_root = canonical_reimport_directory(&source_path, &storage)?;
        let mut entries = std::fs::read_dir(&source_root)
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let key = entry.file_name().to_string_lossy().into_owned();
            if !safe_key(&key) {
                continue;
            }
            let Ok(work_root) = canonical_reimport_directory(&entry.path(), &source_root) else {
                continue;
            };
            let mut versions = Vec::new();
            for version in std::fs::read_dir(&work_root).map_err(|e| e.to_string())? {
                let version = version.map_err(|e| e.to_string())?;
                let name = version.file_name().to_string_lossy().into_owned();
                let Some(number) = name
                    .strip_prefix('v')
                    .and_then(|n| n.parse::<i64>().ok())
                    .filter(|n| *n > 0 && name == format!("v{n}"))
                else {
                    continue;
                };
                let path = canonical_reimport_directory(&version.path(), &work_root)?;
                versions.push((number, path));
                if versions.len() > MAX_REIMPORT_VERSIONS_PER_WORK {
                    return Err(format!("{source}/{key} の版が多すぎます"));
                }
            }
            if versions.is_empty() {
                continue;
            }
            versions.sort_by_key(|(number, _)| *number);
            works.push(FolderWork {
                source: source.into(),
                key,
                versions: versions.into_iter().map(|(_, path)| path).collect(),
            });
        }
    }
    Ok(works)
}

/// 版のフォルダーの応答（`original.json`、無ければ `data.json`）。FANBOX は投稿の形に戻す。
pub(crate) fn version_data(source: &str, version: &Path) -> Result<serde_json::Value, String> {
    let original = version.join("original.json");
    let json = if original.try_exists().map_err(|e| e.to_string())? {
        canonical_reimport_json(&original, version)?
    } else {
        let data = version.join("data.json");
        if !data.try_exists().map_err(|e| e.to_string())? {
            return Err("original.json も data.json もありません".into());
        }
        canonical_reimport_json(&data, version)?
    };
    let text = std::fs::read_to_string(&json).map_err(|e| e.to_string())?;
    let data: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if source == "fanbox" {
        crate::fanbox_api::payload::into_post(data)
            .ok_or_else(|| "FANBOX の投稿の形を読めません".to_string())
    } else {
        Ok(data)
    }
}

/// 棚にまだ一度も入っていない作品か。外した作品も棚には記録が残るので、ここでは対象にしない。
fn never_on_shelf(library: &Library, work: &FolderWork) -> Result<bool, String> {
    Ok(library
        .entity_by_key(EntityKind::Work, &work.source, &work.key)
        .map_err(|e| e.to_string())?
        .is_none())
}

/// 保存フォルダーに残った作品のうち、棚にまだ無いものを棚へ保存する。
pub(crate) async fn reimport_into_shelf(
    library: Arc<Library>,
    storage: PathBuf,
) -> Result<ReimportOutcome, String> {
    let works = {
        let library = Arc::clone(&library);
        tokio::task::spawn_blocking(move || {
            let works = folder_works(&storage)?;
            let mut missing = Vec::new();
            for work in works {
                if never_on_shelf(&library, &work)? {
                    missing.push(work);
                }
            }
            Ok::<_, String>(missing)
        })
        .await
        .map_err(|e| e.to_string())??
    };
    let mut outcome = ReimportOutcome {
        imported: 0,
        skipped: Vec::new(),
    };
    for work in works {
        let mut saved = 0;
        let mut failure = None;
        for version in &work.versions {
            let data = match version_data(&work.source, version) {
                Ok(data) => data,
                Err(error) => {
                    failure = Some(format!("{}: {error}", version.display()));
                    continue;
                }
            };
            match crate::commands::downloader::shelf::save_folder_to_shelf(
                Arc::clone(&library),
                data,
                work.source.clone(),
                work.key.clone(),
                version.join("data_assets"),
            )
            .await
            {
                Ok(_) => saved += 1,
                Err(error) => failure = Some(format!("{}: {error}", version.display())),
            }
        }
        if saved > 0 {
            outcome.imported += 1;
        }
        if let Some(reason) = failure {
            outcome
                .skipped
                .push(format!("{}/{}（{reason}）", work.source, work.key));
        }
    }
    if outcome.imported > 0 {
        let library = Arc::clone(&library);
        tokio::task::spawn_blocking(move || {
            if let Err(error) = library.sync_search_index(&mut |_, _| {}) {
                log::error!("取り込んだ作品を検索の索引へ入れられません: {error}");
            }
            crate::commands::library_health::schedule_semantic_sync(library);
        })
        .await
        .map_err(|e| e.to_string())?;
    }
    Ok(outcome)
}

/// 保存フォルダーを調べ、棚にまだ無い作品を棚へ戻す（切り替えた棚だけ）。
#[tauri::command]
pub async fn library_v2_reimport_folders(app: tauri::AppHandle) -> Result<ReimportOutcome, String> {
    let shelf = app.state::<Arc<LibraryV2>>().inner().clone();
    let library = shelf.writable()?;
    let state = app.state::<Arc<AppState>>().inner().clone();
    let storage = state.db.storage_dir().to_path_buf();
    let outcome = reimport_into_shelf(library, storage).await?;
    shelf.forget_derived();
    Ok(outcome)
}

#[cfg(test)]
#[path = "reimport_shelf_tests.rs"]
mod tests;
