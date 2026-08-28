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

/// 起動してよいファイルの種類。
///
/// **「管理下にある」は「安全」ではない。** 保存先の中身は取得元から来た
/// ものと、書庫から復元したものである。他人の作った書庫を取り込めば、その
/// 中身が保存先に置かれる。管理下かどうかだけで判断していたころは、
/// `.exe` を仕込んだ書庫を復元させ、作品ページの「この端末のJSONを開く」を
/// 押させれば起動できた。piep が実際に作る種類だけを通す。
const OPENABLE_FILE_EXTENSIONS: &[&str] = &[
    "json", "txt", "md", "csv", "log", "epub", "zip", "html", "xhtml", "css", "opf", "ncx", "png",
    "jpg", "jpeg", "webp", "gif", "avif", "bmp", "svg",
];

pub(crate) fn file_extension_is_openable(canonical: &Path) -> bool {
    canonical
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            let lowered = extension.to_ascii_lowercase();
            OPENABLE_FILE_EXTENSIONS.contains(&lowered.as_str())
        })
}

/// 開いてよいか。
///
/// フォルダーは、アプリが面倒を見ている場所（アプリデータと保存先）なら開く。
/// その外側は、拡張子の無いフォルダーだけ - フォルダーを開くのはファイル
/// マネージャーを出すことであって、何かを起動することではない。拡張子を
/// 除くのは macOS のアプリバンドル対策で、あれは中身がフォルダーのまま
/// 起動できてしまう。
///
/// ファイルは、管理下にあることに加えて**種類でも絞る**。
fn open_request_is_allowed(canonical: &Path, is_dir: bool, roots: &[PathBuf]) -> bool {
    let managed = roots.iter().any(|root| canonical.starts_with(root));
    if is_dir {
        return managed || canonical.extension().is_none();
    }
    managed && file_extension_is_openable(canonical)
}

fn resolve_allowed_path(app: &tauri::AppHandle, path: &str) -> Result<PathBuf, String> {
    if path.trim().is_empty() {
        return Err("開く場所が指定されていません".to_string());
    }
    let canonical = PathBuf::from(path)
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
    if !open_request_is_allowed(&canonical, is_dir, &roots) {
        return Err(format!(
            "アプリが管理していないファイルは開けません: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

#[tauri::command]
pub async fn open_managed_path(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let canonical = resolve_allowed_path(&app, &path)?;
    app.opener()
        .open_path(canonical.to_string_lossy().to_string(), None::<&str>)
        .map_err(|error| format!("この場所を開けません: {error}"))
}

/// 管理しているファイルを、起動せずにファイルマネージャー上で選択する。
/// WebView へ opener の無制限 reveal 権限は渡さず、この検証を唯一の入口にする。
#[tauri::command]
pub async fn reveal_managed_path(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let canonical = resolve_allowed_path(&app, &path)?;
    app.opener()
        .reveal_item_in_dir(&canonical)
        .map_err(|error| format!("この場所をファイルマネージャーで表示できません: {error}"))
}

/// 配布しているのは Windows 版だけで、`resolve_allowed_path` は判定の前に
/// `canonicalize` を通す。Windows の `canonicalize` が返すのは `\\?\C:\...`
/// という verbatim 接頭辞つきのパスであって、`/app/data` のような形ではない。
/// ここの試験は、**実際に判定へ渡る形**で書く。
#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    /// canonicalize が返す形。接頭辞まで含めて根に持つ。
    fn roots() -> Vec<PathBuf> {
        vec![
            PathBuf::from(r"\\?\C:\Users\reader\AppData\Roaming\com.hiron.piep"),
            PathBuf::from(r"\\?\D:\piep-library"),
        ]
    }

    #[test]
    fn managed_files_and_folders_open() {
        assert!(open_request_is_allowed(
            Path::new(
                r"\\?\C:\Users\reader\AppData\Roaming\com.hiron.piep\profiles\pixiv\1\v1\assets\icon.jpg"
            ),
            false,
            &roots()
        ));
        assert!(open_request_is_allowed(
            Path::new(r"\\?\D:\piep-library\pixiv\12\v1"),
            true,
            &roots()
        ));
    }

    /// 前方一致で判定していると通ってしまう形。`Path::starts_with` は文字列
    /// ではなく構成要素ごとに比べるので、隣のフォルダーは根の内側にならない。
    #[test]
    fn a_sibling_directory_sharing_the_prefix_is_refused() {
        assert!(!open_request_is_allowed(
            Path::new(r"\\?\D:\piep-library-backup\secrets.txt"),
            false,
            &roots()
        ));
        assert!(!open_request_is_allowed(
            Path::new(r"\\?\C:\Users\reader\AppData\Roaming\com.hiron.piep.old\notes.txt"),
            false,
            &roots()
        ));
    }

    #[test]
    fn another_drive_is_not_inside_the_library() {
        assert!(!open_request_is_allowed(
            Path::new(r"\\?\E:\piep-library\pixiv\12\v1\body.txt"),
            false,
            &roots()
        ));
    }

    /// Windows のパスは大小を区別しないが、`Path::starts_with` は区別する。
    /// 判定が成り立つのは、**両側とも canonicalize を通しているから**である。
    /// 片側だけ生のパスを渡すと、管理下のファイルが開けなくなる（危険な側では
    /// なく、閉じる側に倒れる）。その前提をここに残しておく。
    #[test]
    fn both_sides_must_come_from_canonicalize() {
        assert!(!open_request_is_allowed(
            Path::new(r"\\?\D:\PIEP-LIBRARY\pixiv\12\v1\body.txt"),
            false,
            &roots()
        ));
    }

    /// 出力先は使う人が決める。フォルダーを開くだけなら、外でも構わない。
    #[test]
    fn folders_outside_the_managed_area_still_open() {
        assert!(open_request_is_allowed(
            Path::new(r"\\?\C:\Users\reader\Documents\piep exports"),
            true,
            &roots()
        ));
    }

    /// 管理外のファイルは開かない。起動できてしまうものは、なおさら。
    #[test]
    fn files_outside_the_managed_area_are_refused() {
        assert!(!open_request_is_allowed(
            Path::new(r"\\?\C:\Windows\System32\cmd.exe"),
            false,
            &roots()
        ));
        // 中身がフォルダーでも、拡張子を持つものは起動しうる。
        assert!(!open_request_is_allowed(
            Path::new(r"\\?\C:\Program Files\Malware.app"),
            true,
            &roots()
        ));
    }
}

/// どの OS でも同じでなければならない規則だけを、ここに置く。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_extensionless_directory_outside_every_root_still_opens() {
        assert!(open_request_is_allowed(
            Path::new("exports"),
            true,
            &[PathBuf::from("library")]
        ));
    }

    #[test]
    fn a_file_outside_every_root_is_refused() {
        assert!(!open_request_is_allowed(
            Path::new("payload.exe"),
            false,
            &[PathBuf::from("library")]
        ));
    }

    /// 拡張子を持つものは、中身がフォルダーでも開かない。macOS の
    /// アプリバンドルは、フォルダーのまま起動できてしまう。
    #[test]
    fn a_directory_with_an_extension_is_refused() {
        assert!(!open_request_is_allowed(
            Path::new("Malware.app"),
            true,
            &[PathBuf::from("library")]
        ));
    }

    /// 管理下にあることは、起動してよい理由にならない。保存先の中身には
    /// 他人の作った書庫から復元したものが混じりうる。
    #[test]
    fn an_executable_inside_the_library_is_refused() {
        let roots = [PathBuf::from("library")];
        assert!(!open_request_is_allowed(
            Path::new("library/pixiv/1/v1/original.json.exe"),
            false,
            &roots
        ));
        assert!(!open_request_is_allowed(
            Path::new("library/pixiv/1/v1/run.bat"),
            false,
            &roots
        ));
        assert!(!open_request_is_allowed(
            Path::new("library/pixiv/1/v1/no-extension"),
            false,
            &roots
        ));
    }

    /// piep が実際に作るものは、これまでどおり開ける。大文字でも同じ。
    #[test]
    fn the_kinds_piep_writes_still_open() {
        let roots = [PathBuf::from("library")];
        for name in [
            "library/pixiv/1/v1/original.json",
            "library/pixiv/1/v1/data_assets/image/cover.PNG",
            "library/exports/book.epub",
        ] {
            assert!(
                open_request_is_allowed(Path::new(name), false, &roots),
                "should still open: {name}"
            );
        }
    }
}
