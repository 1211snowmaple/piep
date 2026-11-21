//! 新しい棚のバックアップと復元。
//!
//! バックアップは棚そのものの自立した写し（`Library::backup`）。写し元を止めずに作り、検査を通るまで
//! 棚としては開けない形で置き、通ったら名前を付けて公開する。分割 ZIP は使わない（棚は実体を指紋で持つ
//! ので、写しはそのまま開いて検査できる）。
//!
//! 復元は、開いている棚を入れ替えることになるので、その場では行わない。検査を通った写しを予約し、次に
//! 起動したとき、棚を開く前に入れ替える。今の棚は消さずに隣へ退け、写しは手元に残す（写しから
//! もう一つ写しを作って置く）。入れ替えに失敗したら、退けた棚を元に戻す。
//!
//! 作者・シリーズのアーカイブは、その作品だけを写した棚の部分写し（`Library::copy_works`）。
//! 棚としては開けず、入れ替えには使えない。取り込みは今の棚に無い行だけを足す（`Library::merge_part`）。

use crate::commands::library_v2::{library_dir, LibraryV2};
use crate::library::{Library, LibraryId, PartTransfer};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::Manager;

static RUNNING: AtomicBool = AtomicBool::new(false);

struct RunGuard;
impl Drop for RunGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::Release);
    }
}

fn claim() -> Result<RunGuard, String> {
    RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "棚のバックアップか検査がすでに実行中です".to_string())?;
    Ok(RunGuard)
}

/// 写しの中身の要約。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShelfBackupInspection {
    pub path: String,
    pub works: u64,
    pub files: u64,
    pub bytes: u64,
    /// 今の棚と同じ棚の写しか（違う棚の写しでも復元はできる）。
    pub same_library: bool,
    /// 正本に切り替えた棚の写しか。
    pub switched: bool,
    /// 作者・シリーズのアーカイブ（棚の部分写し）か。入れ替えには使えず、取り込みに使う。
    pub partial: bool,
}

/// 復元の予約。棚の隣に置く。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RestoreRequest {
    pub backup: PathBuf,
    pub requested_at: i64,
}

/// 前回の起動での入れ替えの結果。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RestoreOutcome {
    pub backup: PathBuf,
    /// 退けた前の棚（成功したとき）。
    pub previous: Option<PathBuf>,
    pub error: Option<String>,
    pub finished_at: i64,
}

fn sidecar(shelf: &Path, suffix: &str) -> PathBuf {
    let mut name = shelf.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    shelf.with_file_name(name)
}

fn request_path(shelf: &Path) -> PathBuf {
    sidecar(shelf, ".restore-request.json")
}

fn outcome_path(shelf: &Path) -> PathBuf {
    sidecar(shelf, ".restore-result.json")
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let temp = path.with_extension("json.part");
    std::fs::write(
        &temp,
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(&temp, path).map_err(|e| e.to_string())
}

/// 写しを開いて全体を検査し、中身を数える。写しは開くだけで書き換えない。
pub(crate) fn inspect(
    path: &Path,
    current: Option<LibraryId>,
) -> Result<ShelfBackupInspection, String> {
    let partial = Library::is_part(path);
    if !partial && !path.join("library.json").is_file() {
        return Err("棚のバックアップではありません（library.json がありません）".into());
    }
    let library = if partial {
        Library::open_part(path)
    } else {
        Library::open(path)
    }
    .map_err(|e| format!("バックアップを開けません: {e}"))?;
    library
        .verify()
        .map_err(|e| format!("バックアップの検査を通りません: {e}"))?;
    let conn = library.reader().map_err(|e| e.to_string())?;
    let count = |sql: &str| -> Result<u64, String> {
        conn.query_row(sql, [], |row| row.get::<_, i64>(0))
            .map(|value| value.max(0) as u64)
            .map_err(|e| e.to_string())
    };
    let works = count("SELECT count(*) FROM library_items")?;
    let files = count("SELECT count(*) FROM content_files")?;
    let bytes = count("SELECT coalesce(sum(original_size),0) FROM content_files")?;
    let switched = !partial && matches!(library.switched_at(), Ok(Some(_)));
    Ok(ShelfBackupInspection {
        path: path.to_string_lossy().into_owned(),
        works,
        files,
        bytes,
        same_library: current == Some(library.id),
        switched,
        partial,
    })
}

/// 今の棚の写しを `destination`（まだ無いフォルダー）に作る。試験中の写しからも作れる（棚は書き換えない）。
#[tauri::command]
pub async fn library_v2_backup(
    app: tauri::AppHandle,
    destination: String,
) -> Result<String, String> {
    let shelf = app.state::<Arc<LibraryV2>>().inner().clone();
    let library = shelf.library()?;
    let _guard = claim()?;
    let destination = PathBuf::from(destination);
    let shown = destination.to_string_lossy().into_owned();
    tokio::task::spawn_blocking(move || {
        library
            .backup(&destination)
            .map_err(|e| format!("棚のバックアップを作れません: {e}"))
    })
    .await
    .map_err(|e| format!("バックアップが途中で止まりました: {e}"))??;
    Ok(shown)
}

/// 作者・シリーズのアーカイブを書き出した結果。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShelfArchiveExport {
    pub path: String,
    pub works: u64,
}

/// `parent` の中の、まだ無い名前のフォルダー。同じ名前があれば「名前 (2)」のように番号を足す。
pub(crate) fn unused_folder(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_control() || r#"\/:*?"<>|"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = name.trim().trim_end_matches('.').trim();
    let name = if name.is_empty() {
        "piep-archive"
    } else {
        name
    };
    for n in 1..1000 {
        let candidate = if n == 1 {
            parent.join(name)
        } else {
            parent.join(format!("{name} ({n})"))
        };
        if !candidate.try_exists().map_err(|e| e.to_string())? {
            return Ok(candidate);
        }
    }
    Err("書き出し先の名前を決められません".into())
}

/// 作者・シリーズの作品を、棚の部分写しとして `parent` の中の新しいフォルダーへ書き出す。
/// 棚は読むだけなので、切り替える前の（読むだけの）棚からも書き出せる。
#[tauri::command]
pub async fn library_v2_export_entity_archive(
    app: tauri::AppHandle,
    kind: String,
    provider: String,
    key: String,
    parent: String,
    name: String,
) -> Result<ShelfArchiveExport, String> {
    let shelf = app.state::<Arc<LibraryV2>>().inner().clone();
    let library = shelf.library()?;
    let kind = crate::commands::library_v2::entity_kind(&kind)?;
    let _guard = claim()?;
    tokio::task::spawn_blocking(move || {
        let entity = library
            .entity_by_key(kind, &provider, &key)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "棚にない作者・シリーズです".to_string())?;
        let works = library
            .entity_works(kind, entity)
            .map_err(|e| e.to_string())?;
        if works.is_empty() {
            return Err("書き出す作品がありません".into());
        }
        let destination = unused_folder(Path::new(&parent), &name)?;
        let outcome = library
            .copy_works(&works, &destination)
            .map_err(|e| format!("アーカイブを書き出せません: {e}"))?;
        Ok(ShelfArchiveExport {
            path: destination.to_string_lossy().into_owned(),
            works: outcome.added,
        })
    })
    .await
    .map_err(|e| format!("書き出しが途中で止まりました: {e}"))?
}

/// 作者・シリーズのアーカイブの作品を、今の棚へ足す。今の棚にある作品・記録はそのまま。
#[tauri::command]
pub async fn library_v2_import_archive(
    app: tauri::AppHandle,
    path: String,
) -> Result<PartTransfer, String> {
    let shelf = app.state::<Arc<LibraryV2>>().inner().clone();
    let library = shelf.library()?;
    let _guard = claim()?;
    let outcome = tokio::task::spawn_blocking(move || {
        let path = PathBuf::from(path);
        if !Library::is_part(&path) {
            return Err("作者・シリーズのアーカイブではありません".to_string());
        }
        let part = Library::open_part(&path).map_err(|e| format!("アーカイブを開けません: {e}"))?;
        part.verify()
            .map_err(|e| format!("アーカイブの検査を通りません: {e}"))?;
        let outcome = library
            .merge_part(&part)
            .map_err(|e| format!("アーカイブを取り込めません: {e}"))?;
        if outcome.added + outcome.restored > 0 {
            if let Err(error) = library.sync_search_index(&mut |_, _| {}) {
                log::error!("取り込んだ作品を検索の索引へ載せられません: {error}");
            }
            super::library_health::schedule_semantic_sync(Arc::clone(&library));
        }
        Ok(outcome)
    })
    .await
    .map_err(|e| format!("取り込みが途中で止まりました: {e}"))??;
    shelf.forget_derived();
    Ok(outcome)
}

/// 棚の写しを検査して、中身を数える。
#[tauri::command]
pub async fn library_v2_inspect_backup(
    app: tauri::AppHandle,
    path: String,
) -> Result<ShelfBackupInspection, String> {
    let shelf = app.state::<Arc<LibraryV2>>().inner().clone();
    let current = shelf.library().ok().map(|library| library.id);
    let _guard = claim()?;
    tokio::task::spawn_blocking(move || inspect(Path::new(&path), current))
        .await
        .map_err(|e| format!("検査が途中で止まりました: {e}"))?
}

/// 検査を通った写しで、次の起動のときに棚を入れ替えるよう予約する。今の棚は消さずに隣へ退ける。
#[tauri::command]
pub async fn library_v2_schedule_restore(
    app: tauri::AppHandle,
    path: String,
) -> Result<ShelfBackupInspection, String> {
    let shelf = app.state::<Arc<LibraryV2>>().inner().clone();
    let current = shelf.library().ok().map(|library| library.id);
    let app_data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let live = library_dir(&app_data);
    let backup = PathBuf::from(&path);
    let canonical_backup = std::fs::canonicalize(&backup).map_err(|e| e.to_string())?;
    if let Ok(canonical_live) = std::fs::canonicalize(&live) {
        if canonical_backup.starts_with(&canonical_live)
            || canonical_live.starts_with(&canonical_backup)
        {
            return Err("今の棚そのもの（またはその中）は復元に使えません".into());
        }
    }
    let _guard = claim()?;
    let inspection = tokio::task::spawn_blocking(move || inspect(&backup, current))
        .await
        .map_err(|e| format!("検査が途中で止まりました: {e}"))??;
    if inspection.partial {
        return Err(
            "作者・シリーズのアーカイブは、棚の入れ替えには使えません。取り込みを使ってください"
                .into(),
        );
    }
    // 切り替える前の写しは、もう開けない（棚は切り替えた棚しか開かない）。
    if !inspection.switched {
        return Err("正本に切り替える前の写しは、棚の入れ替えに使えません".into());
    }
    write_json(
        &request_path(&live),
        &RestoreRequest {
            backup: canonical_backup,
            requested_at: now(),
        },
    )?;
    Ok(inspection)
}

/// 予約した復元と、前回の入れ替えの結果。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RestoreState {
    pub pending: Option<RestoreRequest>,
    pub last: Option<RestoreOutcome>,
}

/// 予約した復元と、前回の起動での入れ替えの結果を読む（設定の画面が出す）。
#[tauri::command]
pub fn library_v2_restore_state(app: tauri::AppHandle) -> Result<RestoreState, String> {
    let app_data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let live = library_dir(&app_data);
    fn read<T: serde::de::DeserializeOwned>(path: PathBuf) -> Option<T> {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    }
    Ok(RestoreState {
        pending: read(request_path(&live)),
        last: read(outcome_path(&live)),
    })
}

/// 予約を取り消す。
#[tauri::command]
pub fn library_v2_cancel_restore(app: tauri::AppHandle) -> Result<bool, String> {
    let app_data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    match std::fs::remove_file(request_path(&library_dir(&app_data))) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

/// 起動のとき、棚を開く前に呼ぶ。予約があれば入れ替える。結果は隣の記録に残し、予約は必ず消す
/// （失敗した予約で起動のたびに止まらない）。
pub fn apply_pending_restore(live: &Path) {
    let request = request_path(live);
    let Ok(bytes) = std::fs::read(&request) else {
        return;
    };
    let _ = std::fs::remove_file(&request);
    let outcome = match serde_json::from_slice::<RestoreRequest>(&bytes) {
        Ok(pending) => {
            let result = swap(live, &pending.backup);
            if let Err(error) = &result {
                log::error!("棚の復元を行えませんでした（今の棚のまま開きます）: {error}");
            } else {
                log::info!(
                    "棚をバックアップから復元しました: {}",
                    pending.backup.display()
                );
            }
            RestoreOutcome {
                backup: pending.backup,
                previous: result.as_ref().ok().cloned().flatten(),
                error: result.err(),
                finished_at: now(),
            }
        }
        Err(error) => RestoreOutcome {
            backup: PathBuf::new(),
            previous: None,
            error: Some(format!("復元の予約を読めません: {error}")),
            finished_at: now(),
        },
    };
    if let Err(error) = write_json(&outcome_path(live), &outcome) {
        log::warn!("復元の結果を残せません: {error}");
    }
}

/// 今の棚を隣へ退け、写しからもう一つ写しを作って棚の場所に置く。返すのは退けた場所。
fn swap(live: &Path, backup: &Path) -> Result<Option<PathBuf>, String> {
    // 先に写しを開いて検査する（壊れた写しで今の棚を退けない）。
    let source = Library::open(backup).map_err(|e| format!("バックアップを開けません: {e}"))?;
    source
        .verify()
        .map_err(|e| format!("バックアップの検査を通りません: {e}"))?;
    let previous = if live.try_exists().map_err(|e| e.to_string())? {
        let aside = sidecar(live, &format!(".before-restore-{}", now()));
        std::fs::rename(live, &aside).map_err(|e| format!("今の棚を退けられません: {e}"))?;
        Some(aside)
    } else {
        None
    };
    if let Err(error) = source.backup(live) {
        // 入れ替えに失敗したら、退けた棚を元へ戻す。
        let _ = std::fs::remove_dir_all(live);
        if let Some(aside) = &previous {
            if let Err(back) = std::fs::rename(aside, live) {
                return Err(format!(
                    "バックアップを棚の場所へ写せず、退けた棚も戻せません（{}にあります）: {error}; {back}",
                    aside.display()
                ));
            }
        }
        return Err(format!("バックアップを棚の場所へ写せません: {error}"));
    }
    Ok(previous)
}

#[cfg(test)]
#[path = "backup_shelf_tests.rs"]
mod tests;
