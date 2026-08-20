//! ファイルマネージャーで場所を開く。
//!
//! opener プラグインの `open_path` は、フロント側から呼ぶと capability に
//! 書いた固定のスコープで弾かれる。しかし開きたい場所は固定ではない -
//! 保存先も、EPUB の出力先も、使う人が決める。静的な glob では書けない。
//!
//! そこで「どこなら開いてよいか」をアプリ自身に判断させる。Rust 側の API は
//! コマンド層のスコープを通らないので、ここが唯一の門になる。

use crate::AppState;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::Manager;
use tauri_plugin_opener::OpenerExt;

/// 開いてよいか。
///
/// アプリが面倒を見ている場所（アプリデータと保存先）なら、ファイルでも
/// フォルダーでも開く。その外側は、拡張子の無いフォルダーだけ - フォルダーを
/// 開くのはファイルマネージャーを出すことであって、何かを起動することでは
/// ない。拡張子を除くのは macOS のアプリバンドル対策で、あれは中身が
/// フォルダーのまま起動できてしまう。
fn open_request_is_allowed(canonical: &Path, is_dir: bool, roots: &[PathBuf]) -> bool {
    if roots.iter().any(|root| canonical.starts_with(root)) {
        return true;
    }
    is_dir && canonical.extension().is_none()
}

#[tauri::command]
pub async fn open_managed_path(app: tauri::AppHandle, path: String) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("開く場所が指定されていません".to_string());
    }
    let requested = PathBuf::from(&path);
    let canonical = requested
        .canonicalize()
        .map_err(|error| format!("この場所を開けません（{error}）: {path}"))?;
    let is_dir = canonical.is_dir();

    let state = app.state::<Arc<AppState>>();
    let storage = state.db.storage_dir().to_path_buf();
    let mut roots = Vec::new();
    if let Ok(resolved) = storage.canonicalize() {
        roots.push(resolved);
    }
    if let Ok(app_data) = app.path().app_data_dir() {
        if let Ok(resolved) = app_data.canonicalize() {
            roots.push(resolved);
        }
    }
    // 保存先がアプリデータの外にあっても、その親までは辿らない。開いてよいのは
    // 保存先そのものと、その下だけである。
    if !open_request_is_allowed(&canonical, is_dir, &roots) {
        return Err(format!(
            "アプリが管理していないファイルは開けません: {}",
            canonical.display()
        ));
    }
    app.opener()
        .open_path(canonical.to_string_lossy().to_string(), None::<&str>)
        .map_err(|error| format!("この場所を開けません: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roots() -> Vec<PathBuf> {
        vec![PathBuf::from("/app/data"), PathBuf::from("/library/piep")]
    }

    #[test]
    fn managed_files_and_folders_open() {
        assert!(open_request_is_allowed(
            Path::new("/app/data/profiles/pixiv/1/v1/assets/icon.jpg"),
            false,
            &roots()
        ));
        assert!(open_request_is_allowed(
            Path::new("/library/piep/pixiv/12/v1"),
            true,
            &roots()
        ));
    }

    /// 出力先は使う人が決める。フォルダーを開くだけなら、外でも構わない。
    #[test]
    fn folders_outside_the_managed_area_still_open() {
        assert!(open_request_is_allowed(
            Path::new("/home/reader/Documents/piep exports"),
            true,
            &roots()
        ));
    }

    /// 管理外のファイルは開かない。起動できてしまうものは、なおさら。
    #[test]
    fn files_outside_the_managed_area_are_refused() {
        assert!(!open_request_is_allowed(
            Path::new("/windows/system32/cmd.exe"),
            false,
            &roots()
        ));
        // 中身がフォルダーでも、拡張子を持つものは起動しうる。
        assert!(!open_request_is_allowed(
            Path::new("/Applications/Malware.app"),
            true,
            &roots()
        ));
    }
}
