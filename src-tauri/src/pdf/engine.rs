//! pdfium の束ね方。
//!
//! pdfium は一度だけ初期化して、そのまま持ち続ける。`FPDF_InitLibrary` と
//! `FPDF_DestroyLibrary` は対にして呼ぶものだが、途中で壊すと、まだ開いている
//! 文書のハンドルが無効になる。プロセスが終わるまで生かしておく。
//!
//! 高水準 API（`PdfDocument` など）は使わない。構造ツリーと marked content は
//! 生の `FPDF_*` でしか辿れず、両方を混ぜると束ねた本体の所有権が二重になる。
//! ここでは束ねた本体だけを持ち、開け閉めは `extract` 側の番人が受け持つ。

use pdfium_render::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// 実行時に読む `pdfium.dll` の場所。
///
/// 配布物では実行ファイルの隣（Tauri の `bundle.resources`）にあり、`lib.rs` の
/// 起動時に入れる。テストは `vendor/pdfium/` を指す。
static LIBRARY_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

static BINDINGS: OnceLock<Result<Bindings, String>> = OnceLock::new();

/// 束ねた pdfium。`thread_safe` により、`FPDF_*` の呼び出しは中で直列化される。
struct Bindings(Box<dyn PdfiumLibraryBindings>);

// SAFETY: `pdfium-render` の `thread_safe` 機能が、すべての `FPDF_*` 呼び出しを
// 一つのミューテックスの内側に入れる。束そのものは読むだけで書き換えない。
unsafe impl Send for Bindings {}
unsafe impl Sync for Bindings {}

/// `pdfium.dll` の場所を決める。起動時に一度だけ呼ぶ。
///
/// 束ね終わったあとに変えても効かない。**先に呼ばれなかったときは
/// `vendor/pdfium/` を探しに行く**ので、`cargo test` と `cargo run` は
/// 設定しなくても動く。
pub fn set_library_path(path: impl Into<PathBuf>) {
    if let Ok(mut slot) = LIBRARY_PATH.lock() {
        *slot = Some(path.into());
    }
}

/// いま使う `pdfium.dll` の場所。見つからなければ `None`。
pub fn library_path() -> Option<PathBuf> {
    if let Ok(slot) = LIBRARY_PATH.lock() {
        if let Some(path) = slot.as_ref() {
            return Some(path.clone());
        }
    }
    fallback_library_path()
}

/// 開発と試験のための置き場所。リポジトリに入れてある版を指す。
fn fallback_library_path() -> Option<PathBuf> {
    let name = Pdfium::pdfium_platform_library_name();
    let candidates = [
        // `cargo test` / `cargo run` は src-tauri を作業ディレクトリにする。
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("vendor")
            .join("pdfium")
            .join(&name),
        // 配布物では実行ファイルの隣に配られる。
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(&name)))
            .unwrap_or_default(),
    ];
    candidates.into_iter().find(|path| path.is_file())
}

/// 束ねた pdfium を借りる。最初の呼び出しで読み込み、以後は同じものを返す。
///
/// 読み込みに失敗した理由も覚える。**毎回読み込みを試すと、DLL の無い環境で
/// 作品を開くたびに数十ミリ秒を捨てる。**
pub(super) fn bindings() -> Result<&'static dyn PdfiumLibraryBindings, String> {
    let slot = BINDINGS.get_or_init(|| {
        let path = library_path().ok_or_else(|| {
            "pdfium.dll が見つかりません。配布物では実行ファイルの隣に置かれます".to_string()
        })?;
        bind(&path).map(Bindings)
    });
    match slot {
        Ok(bindings) => Ok(bindings.0.as_ref()),
        Err(error) => Err(error.clone()),
    }
}

fn bind(path: &Path) -> Result<Box<dyn PdfiumLibraryBindings>, String> {
    let bindings = Pdfium::bind_to_library(path)
        .map_err(|error| format!("pdfium.dll を読み込めません（{}）: {error}", path.display()))?;
    // 高水準 API を通さないので、初期化はこちらで行う。二度呼んでも pdfium 側は
    // 数えているだけだが、ここは一度しか通らない。
    unsafe { bindings.FPDF_InitLibrary() };
    Ok(bindings)
}
