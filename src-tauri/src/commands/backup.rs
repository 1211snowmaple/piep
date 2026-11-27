//! 棚のバックアップと復元。
//!
//! バックアップは棚そのものの自立した写し（`Library::backup`）。写し元を止めずに作り、検査を通るまで
//! 棚としては開けない形で置き、通ったら名前を付けて公開する。分割 ZIP は使わない（棚は実体を指紋で持つ
//! ので、写しはそのまま開いて検査できる）。
//!
//! 復元は、開いている棚を入れ替えることになるので、その場では行わない。検査を通った写しを予約し、次に
//! 起動したとき、棚を開く前に入れ替える。検査済みの写しを隣に用意してから今の棚を退ける。
//! 途中で止まっても予約と退けた棚が残り、次の起動で入れ替えを続けられる。
//!
//! 作者・シリーズのアーカイブは、その作品だけを写した棚の部分写し（`Library::copy_works`）。
//! 棚としては開けず、入れ替えには使えない。取り込みは今の棚に無い行だけを足す（`Library::merge_part`）。

use crate::commands::shelf::{library_dir, Shelf};
use crate::library::{Library, LibraryId, PartTransfer};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::ErrorKind;
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
    /// 作者・シリーズのアーカイブ（棚の部分写し）か。入れ替えには使えず、取り込みに使う。
    pub partial: bool,
}

/// 復元の予約。棚の隣に置く。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RestoreRequest {
    pub backup: PathBuf,
    pub requested_at: i64,
    id: LibraryId,
    prepared: bool,
}

impl RestoreRequest {
    fn new(backup: PathBuf) -> Self {
        Self {
            backup,
            requested_at: now(),
            id: LibraryId::now_v7(),
            prepared: false,
        }
    }

    fn stage(&self, live: &Path) -> PathBuf {
        sidecar(live, &format!(".restore-stage-{}", self.id))
    }

    fn previous(&self, live: &Path) -> PathBuf {
        sidecar(
            live,
            &format!(".before-restore-{}-{}", self.requested_at, self.id),
        )
    }
}

/// 前回の起動での入れ替えの結果。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RestoreOutcome {
    id: LibraryId,
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

fn read_request(path: &Path) -> Result<Option<RestoreRequest>, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("復元の予約を読めません: {error}")),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("復元の予約を読めません: {error}")),
    }
}

fn ensure_restore_can_change(live: &Path, request: &Path) -> Result<(), String> {
    if let Some(pending) = read_request(request)? {
        if pending.prepared || exists(&pending.stage(live))? {
            return Err("棚の入れ替えが始まっているため、復元の予約を変更できません".into());
        }
    }
    Ok(())
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
    Ok(ShelfBackupInspection {
        path: path.to_string_lossy().into_owned(),
        works,
        files,
        bytes,
        same_library: current == Some(library.id),
        partial,
    })
}

/// 今の棚の写しを `destination`（まだ無いフォルダー）に作る。棚は書き換えない。
#[tauri::command]
pub async fn shelf_backup(app: tauri::AppHandle, destination: String) -> Result<String, String> {
    let shelf = app.state::<Arc<Shelf>>().inner().clone();
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
/// 棚は読むだけで、書き換えない。
#[tauri::command]
pub async fn shelf_export_entity_archive(
    app: tauri::AppHandle,
    kind: String,
    provider: String,
    key: String,
    parent: String,
    name: String,
) -> Result<ShelfArchiveExport, String> {
    let shelf = app.state::<Arc<Shelf>>().inner().clone();
    let library = shelf.library()?;
    let kind = crate::commands::shelf::entity_kind(&kind)?;
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
pub async fn shelf_import_archive(
    app: tauri::AppHandle,
    path: String,
) -> Result<PartTransfer, String> {
    let shelf = app.state::<Arc<Shelf>>().inner().clone();
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
pub async fn shelf_inspect_backup(
    app: tauri::AppHandle,
    path: String,
) -> Result<ShelfBackupInspection, String> {
    let shelf = app.state::<Arc<Shelf>>().inner().clone();
    let current = shelf.library().ok().map(|library| library.id);
    let _guard = claim()?;
    tokio::task::spawn_blocking(move || inspect(Path::new(&path), current))
        .await
        .map_err(|e| format!("検査が途中で止まりました: {e}"))?
}

/// 検査を通った写しで、次の起動のときに棚を入れ替えるよう予約する。今の棚は消さずに隣へ退ける。
#[tauri::command]
pub async fn shelf_schedule_restore(
    app: tauri::AppHandle,
    path: String,
) -> Result<ShelfBackupInspection, String> {
    let shelf = app.state::<Arc<Shelf>>().inner().clone();
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
    let request = request_path(&live);
    ensure_restore_can_change(&live, &request)?;
    let inspection = tokio::task::spawn_blocking(move || inspect(&backup, current))
        .await
        .map_err(|e| format!("検査が途中で止まりました: {e}"))??;
    if inspection.partial {
        return Err(
            "作者・シリーズのアーカイブは、棚の入れ替えには使えません。取り込みを使ってください"
                .into(),
        );
    }
    write_json(&request, &RestoreRequest::new(canonical_backup))?;
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
pub fn shelf_restore_state(app: tauri::AppHandle) -> Result<RestoreState, String> {
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
pub fn shelf_cancel_restore(app: tauri::AppHandle) -> Result<bool, String> {
    let _guard = claim()?;
    let app_data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let live = library_dir(&app_data);
    let request = request_path(&live);
    ensure_restore_can_change(&live, &request)?;
    match fs::remove_file(request) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

/// 起動のとき、棚を開く前に呼ぶ。入れ替えの途中で止まったら、残った予約から続ける。
pub fn apply_pending_restore(live: &Path) -> Result<(), String> {
    let _guard = claim()?;
    recover_pending_restore(live)
}

fn recover_pending_restore(live: &Path) -> Result<(), String> {
    let request = request_path(live);
    let Some(mut pending) = read_request(&request)? else {
        return Ok(());
    };
    let result_path = outcome_path(live);
    let recorded = fs::read(&result_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<RestoreOutcome>(&bytes).ok())
        .is_some_and(|outcome| outcome.id == pending.id);
    if !recorded {
        let stage = pending.stage(live);
        let result = if pending.prepared {
            Ok(())
        } else {
            prepare_restore(&pending, &stage).and_then(|()| {
                pending.prepared = true;
                write_json(&request, &pending)
            })
        };
        let (previous, error) = match result {
            Ok(()) => (
                install_restore(live, &stage, &pending.previous(live))?,
                None,
            ),
            Err(error)
                if !pending.prepared && !exists(&stage)? && exists(&live.join("library.json"))? =>
            {
                log::error!("棚の復元を行えませんでした（今の棚のまま開きます）: {error}");
                (None, Some(error))
            }
            Err(error) => return Err(error),
        };
        write_json(
            &result_path,
            &RestoreOutcome {
                id: pending.id,
                backup: pending.backup,
                previous,
                error,
                finished_at: now(),
            },
        )
        .map_err(|error| format!("復元の結果を残せません: {error}"))?;
    }
    if !exists(&live.join("library.json"))? {
        return Err("入れ替え後の棚が見つかりません".into());
    }
    fs::remove_file(&request).map_err(|error| format!("復元の予約を片付けられません: {error}"))
}

fn exists(path: &Path) -> Result<bool, String> {
    path.try_exists().map_err(|error| {
        format!(
            "復元に使う場所を確認できません（{}）: {error}",
            path.display()
        )
    })
}

fn prepare_restore(pending: &RestoreRequest, stage: &Path) -> Result<(), String> {
    if !exists(stage)? {
        let source = Library::open(&pending.backup)
            .map_err(|error| format!("バックアップを開けません: {error}"))?;
        source
            .verify()
            .map_err(|error| format!("バックアップの検査を通りません: {error}"))?;
        source
            .backup(stage)
            .map_err(|error| format!("復元用の写しを作れません: {error}"))?;
    }
    Ok(())
}

fn install_restore(live: &Path, stage: &Path, previous: &Path) -> Result<Option<PathBuf>, String> {
    if exists(stage)? {
        if !exists(&stage.join("library.json"))? {
            return Err("復元用の写しが完成していません".into());
        }
        if exists(live)? {
            if exists(previous)? {
                return Err("退けた棚の場所がすでに使われています".into());
            }
            fs::rename(live, previous)
                .map_err(|error| format!("今の棚を退けられません: {error}"))?;
        }
        fs::rename(stage, live)
            .map_err(|error| format!("復元用の写しを棚の場所に置けません: {error}"))?;
    }
    // 準備済みの予約で写しが無ければ、棚の場所への改名が済んでいる。
    if !exists(&live.join("library.json"))? {
        return Err("入れ替え後の棚が見つかりません".into());
    }
    Ok(exists(previous)?.then(|| previous.to_path_buf()))
}

#[cfg(test)]
#[path = "backup_tests.rs"]
mod tests;
