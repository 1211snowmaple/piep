//! 棚の数と手入れの命令（ホーム・横の欄・設定・診断の画面）。
//!
//! 読む命令は、試験中の写しでも開いた棚から数える（一覧と同じく画面は新しい棚を見ている）。
//! 書き換える命令（圧縮・索引の統合・回収・意味検索の入切）は、正本に切り替えた棚だけに効く。
//!
//! 意味検索を使うかは棚のデータではなくアプリの設定なので、アプリのフォルダーの
//! `search-settings.json` に置く。無いうちは旧棚の設定を一度だけ写す。

use super::library_v2::LibraryV2;
use crate::database::models::{
    DashboardSummary, LibraryMaintenanceResult, LibraryShelfCounts, SearchIndexOptimizationResult,
    SearchIndexStatus,
};
use crate::library::{Library, LibraryId, ReclaimReport};
use crate::library_health::{ReadingSet, ShelfDiagnostics};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{Emitter, State};

const SETTINGS_FILE: &str = "search-settings.json";

static SETTINGS_PATH: OnceLock<PathBuf> = OnceLock::new();
/// 意味検索の索引へ書くのは一度に一つ（背景の追いつきと作り直し）。
pub(crate) static SEMANTIC_TURN: Mutex<()> = Mutex::new(());
static SEMANTIC_RUNNING: AtomicBool = AtomicBool::new(false);
static SEMANTIC_AGAIN: AtomicBool = AtomicBool::new(false);
/// 意味検索を切ったら、走っている追いつきを途中で降ろす。
static SEMANTIC_CANCEL: AtomicBool = AtomicBool::new(false);

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchSettings {
    semantic_enabled: bool,
}

/// 起動のときに一度呼ぶ。
pub(crate) fn set_settings_dir(app_data: &Path) {
    let _ = SETTINGS_PATH.set(app_data.join(SETTINGS_FILE));
}

fn read_settings() -> Option<SearchSettings> {
    let path = SETTINGS_PATH.get()?;
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// 意味検索を使うか。設定がまだ無ければ使わない（旧棚の設定を写すまで）。
pub(crate) fn semantic_enabled() -> bool {
    read_settings().is_some_and(|settings| settings.semantic_enabled)
}

pub(crate) fn has_settings() -> bool {
    read_settings().is_some()
}

pub(crate) fn store_semantic_enabled(enabled: bool) -> Result<(), String> {
    let path = SETTINGS_PATH
        .get()
        .ok_or_else(|| "アプリのフォルダーが決まっていません".to_string())?;
    let text = serde_json::to_string_pretty(&SearchSettings {
        semantic_enabled: enabled,
    })
    .map_err(|e| e.to_string())?;
    // 途中で落ちても半端な設定を残さない。
    let stage = path.with_extension("json.part");
    std::fs::write(&stage, text).map_err(|e| e.to_string())?;
    std::fs::rename(&stage, path).map_err(|e| e.to_string())
}

/// 意味検索の索引を棚に追いつかせる。走っていれば、終わったあとにもう一度回す。
/// 意味検索を切っていれば何もしない。モデルを読み込むので、別のスレッドで走らせる。
pub(crate) fn schedule_semantic_sync(library: Arc<Library>) {
    if !semantic_enabled() {
        return;
    }
    SEMANTIC_AGAIN.store(true, Ordering::Release);
    if SEMANTIC_RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("piep-semantic-sync".into())
        .spawn(move || {
            while SEMANTIC_AGAIN.swap(false, Ordering::AcqRel) && semantic_enabled() {
                SEMANTIC_CANCEL.store(false, Ordering::Release);
                let Ok(_turn) = SEMANTIC_TURN.lock() else {
                    break;
                };
                let started = std::time::Instant::now();
                match library.sync_semantic_index(
                    None,
                    &|| SEMANTIC_CANCEL.load(Ordering::Acquire),
                    &mut |_, _| {},
                ) {
                    Ok(report) => log::info!(
                        "新しい棚の意味検索の索引を合わせました: {} ms, {report:?}",
                        started.elapsed().as_millis()
                    ),
                    Err(error) => log::warn!("新しい棚の意味検索の索引を合わせられません: {error}"),
                }
            }
            SEMANTIC_RUNNING.store(false, Ordering::Release);
        });
    if spawned.is_err() {
        SEMANTIC_RUNNING.store(false, Ordering::Release);
        log::error!("意味検索の索引を合わせるスレッドを作れません");
    }
}

/// 意味検索を入れる・切る。切ると索引を消し、走っている追いつきを止める。
pub(crate) fn apply_semantic_enabled(library: &Arc<Library>, enabled: bool) -> Result<(), String> {
    store_semantic_enabled(enabled)?;
    if enabled {
        schedule_semantic_sync(Arc::clone(library));
        return Ok(());
    }
    SEMANTIC_CANCEL.store(true, Ordering::Release);
    let _turn = SEMANTIC_TURN.lock().map_err(|e| e.to_string())?;
    library.clear_semantic_index().map_err(|e| e.to_string())
}

async fn on_library<T: Send + 'static>(
    state: &Arc<LibraryV2>,
    f: impl FnOnce(Arc<Library>) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let library = state.library()?;
    tauri::async_runtime::spawn_blocking(move || f(library))
        .await
        .map_err(|e| e.to_string())?
}

async fn on_switched<T: Send + 'static>(
    state: &Arc<LibraryV2>,
    f: impl FnOnce(Arc<Library>) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let library = state.writable()?;
    tauri::async_runtime::spawn_blocking(move || f(library))
        .await
        .map_err(|e| e.to_string())?
}

/// 検索の索引の状態（ホーム・設定・一覧の索引の表示）。
#[tauri::command]
pub async fn library_v2_search_index_status(
    state: State<'_, Arc<LibraryV2>>,
) -> Result<SearchIndexStatus, String> {
    let state = Arc::clone(&state);
    on_library(&state, |library| {
        library
            .search_index_status(semantic_enabled())
            .map_err(|e| e.to_string())
    })
    .await
}

/// ホームの集計。最近の保存は含めない（画面が棚の一覧で引く）。
#[tauri::command]
pub async fn library_v2_dashboard(
    state: State<'_, Arc<LibraryV2>>,
) -> Result<DashboardSummary, String> {
    let state = Arc::clone(&state);
    on_library(&state, |library| {
        let index = library
            .search_index_status(semantic_enabled())
            .map_err(|e| e.to_string())?;
        library.dashboard(&index).map_err(|e| e.to_string())
    })
    .await
}

/// 横の欄の件数。読みかけは、端末の記録を棚へ移したあとは棚の読書位置で、移す前は
/// 端末に残る旧棚の番号（`legacyReading`）で数える。
#[tauri::command]
pub async fn library_v2_shelf_counts(
    state: State<'_, Arc<LibraryV2>>,
    legacy_reading: Option<Vec<i64>>,
) -> Result<LibraryShelfCounts, String> {
    let state = Arc::clone(&state);
    let numbers_state = Arc::clone(&state);
    on_library(&state, move |library| {
        let reading = match legacy_reading {
            None => ReadingSet::Shelf,
            Some(numbers) => {
                let known = numbers_state.legacy_numbers(&library)?;
                let wanted: std::collections::HashSet<i64> = numbers.into_iter().collect();
                ReadingSet::Legacy(
                    known
                        .iter()
                        .filter(|(_, number)| wanted.contains(number))
                        .map(|(work, _)| *work)
                        .collect::<Vec<LibraryId>>(),
                )
            }
        };
        library.shelf_counts(reading).map_err(|e| e.to_string())
    })
    .await
}

/// 診断の数を測る。
#[tauri::command]
pub async fn library_v2_diagnostics(
    state: State<'_, Arc<LibraryV2>>,
) -> Result<ShelfDiagnostics, String> {
    let state = Arc::clone(&state);
    on_library(&state, |library| {
        let index = library
            .search_index_status(semantic_enabled())
            .map_err(|e| e.to_string())?;
        library.diagnostics(index).map_err(|e| e.to_string())
    })
    .await
}

/// 台帳の統計を整え、`compact` なら空き領域を回収する。
#[tauri::command]
pub async fn library_v2_maintain(
    app: tauri::AppHandle,
    state: State<'_, Arc<LibraryV2>>,
    compact: bool,
) -> Result<LibraryMaintenanceResult, String> {
    let state = Arc::clone(&state);
    let _ = app.emit(
        "library-maintenance-progress",
        serde_json::json!({ "phase": if compact { "compacting" } else { "optimizing" } }),
    );
    let result = on_switched(&state, move |library| {
        library
            .maintain_database(compact)
            .map_err(|e| e.to_string())
    })
    .await;
    let _ = app.emit(
        "library-maintenance-progress",
        serde_json::json!({
            "phase": if result.is_ok() { "complete" } else { "failed" },
            "error": result.as_ref().err(),
        }),
    );
    result
}

/// 全文検索の索引の断片をまとめる。進み具合は旧棚と同じ知らせで流す。
#[tauri::command]
pub async fn library_v2_optimize_search_index(
    app: tauri::AppHandle,
    state: State<'_, Arc<LibraryV2>>,
) -> Result<SearchIndexOptimizationResult, String> {
    let state = Arc::clone(&state);
    let emit = |phase: &str, error: Option<&String>| {
        let _ = app.emit(
            "search-index-optimization-progress",
            serde_json::json!({ "phase": phase, "segments": 0, "error": error }),
        );
    };
    emit("preflight", None);
    emit("merging", None);
    let result = on_switched(&state, |library| {
        library.optimize_search_index().map_err(|e| e.to_string())
    })
    .await;
    emit(
        if result.is_ok() { "complete" } else { "failed" },
        result.as_ref().err(),
    );
    result
}

/// 台帳と全ファイルを照合する（壊れていれば最初に見つけた問題を誤りで返す）。
#[tauri::command]
pub async fn library_v2_verify(state: State<'_, Arc<LibraryV2>>) -> Result<u64, String> {
    let state = Arc::clone(&state);
    on_library(&state, |library| {
        library.verify().map_err(|e| e.to_string())
    })
    .await
}

/// どこからも参照されない実体を片付ける（使われてから 1 日は残し、隔離して 7 日後に消す）。
#[tauri::command]
pub async fn library_v2_reclaim(state: State<'_, Arc<LibraryV2>>) -> Result<ReclaimReport, String> {
    let state = Arc::clone(&state);
    on_switched(&state, |library| {
        library.reclaim_now().map_err(|e| e.to_string())
    })
    .await
}

#[cfg(test)]
#[path = "library_health_tests.rs"]
mod tests;
