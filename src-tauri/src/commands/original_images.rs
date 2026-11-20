//! 縮小版で保存された pixiv の表紙・イラストを、原寸に取り直す。
//!
//! 取りに行くのは画像サーバー（`i.pximg.net`）だけで、pixiv の API も
//! アカウントも使わない。原寸の在り処は保存済みの縮小版の URL から決まる。
//! 決め方は `downloader::pixiv_originals` にある。
//!
//! 版は増やさず、今の版の画像をその場で差し替える。作者が何かを変えたわけ
//! ではないので、版の履歴に「更新」を残すと読み手を誤らせる。
//!
//! 再開位置は持たない。差し替えた画像は縮小版の URL ではなくなるので、
//! 次に走らせたときは残りだけが対象になる。

use crate::downloader::asset_downloader::{
    extract_extension, extract_filename, sanitize_filename, save_response_atomically,
    MAX_ASSET_DOWNLOAD_BYTES,
};
use crate::downloader::pixiv_originals::{cdn_client, find_original, OriginalLookup};
use crate::AppState;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use tauri::{Emitter, Manager};

static RUNNING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);
static PROGRESS: LazyLock<std::sync::Mutex<Option<OriginalImageProgress>>> =
    LazyLock::new(|| std::sync::Mutex::new(None));

/// 一枚ごとの間隔。画像サーバーへの問い合わせは一枚につき 1〜3 回の確認と
/// 1 回の取得になるので、この間隔で 1 秒あたり数回に収まる。
const PACE: Duration = Duration::from_millis(500);
/// 接続の失敗がこれだけ続いたら、回線か相手の不調とみて止める。
const MAX_CONSECUTIVE_FAILURES: usize = 5;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginalImageProgress {
    /// 段階。`running` / `checked` / `complete` / `canceled` / `paused` のどれか。
    phase: String,
    completed: usize,
    total: usize,
    upgraded: usize,
    /// 原寸が公開元に無かった（消された作品・イラスト）。縮小版のまま残る。
    unavailable: usize,
    failed: usize,
    active_label: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginalImageStatus {
    covers: usize,
    illustrations: usize,
    running: bool,
    progress: Option<OriginalImageProgress>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginalImageResult {
    attempted: usize,
    upgraded: usize,
    unavailable: usize,
    failed: usize,
    canceled: bool,
    stopped_reason: Option<String>,
    remaining: usize,
}

struct RunGuard;

impl Drop for RunGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::Release);
        CANCEL.store(false, Ordering::Release);
    }
}

fn emit_progress(app: &tauri::AppHandle, progress: OriginalImageProgress) {
    *PROGRESS.lock().unwrap_or_else(|error| error.into_inner()) = Some(progress.clone());
    let _ = app.emit("original-image-upgrade-progress", progress);
}

async fn reduced_images(
    state: Arc<AppState>,
) -> Result<Vec<crate::database::queries::ReducedPixivImage>, String> {
    tokio::task::spawn_blocking(move || state.db.reduced_pixiv_images())
        .await
        .map_err(|error| format!("Database task failed: {error}"))?
}

/// 原寸になっていない画像の数と、走っている取り直しの進み具合。
#[tauri::command]
pub async fn get_original_image_status(
    app: tauri::AppHandle,
) -> Result<OriginalImageStatus, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let images = reduced_images(state).await?;
    let covers = images
        .iter()
        .filter(|image| image.asset_type == "cover")
        .count();
    Ok(OriginalImageStatus {
        covers,
        illustrations: images.len() - covers,
        running: RUNNING.load(Ordering::Acquire),
        progress: PROGRESS
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone(),
    })
}

/// 走っている取り直しに中止を頼む。今の一枚が終わったところで止まる。
#[tauri::command]
pub fn cancel_original_image_upgrade() -> bool {
    if !RUNNING.load(Ordering::Acquire) {
        return false;
    }
    CANCEL.store(true, Ordering::Release);
    true
}

/// 保存先の名前。表紙は原寸の URL の名前、イラストは今の名前のまま拡張子だけ
/// 原寸に合わせる（本文の `[pixivimage:]` は拡張子を見ずに名前で引く）。
fn destination_for(
    image: &crate::database::queries::ReducedPixivImage,
    url: &str,
) -> Option<PathBuf> {
    let current = Path::new(&image.local_path);
    let folder = current.parent()?;
    let name = if image.asset_type == "cover" {
        sanitize_filename(&extract_filename(url, "cover.jpg"))
    } else {
        let stem = current.file_stem()?.to_str()?;
        sanitize_filename(&format!("{stem}.{}", extract_extension(url, "jpg")))
    };
    Some(folder.join(name))
}

fn mime_for(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("png") => Some("image/png"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        _ => None,
    }
}

enum Outcome {
    Upgraded,
    Unavailable,
    /// 行が先に変わっていた（その間に保存し直された）。数えずに飛ばす。
    Skipped,
}

async fn upgrade_one(
    state: &Arc<AppState>,
    client: &reqwest::Client,
    image: &crate::database::queries::ReducedPixivImage,
) -> Result<Outcome, String> {
    let url = match find_original(client, &image.original_url).await? {
        OriginalLookup::Found(url) => url,
        OriginalLookup::Unavailable => return Ok(Outcome::Unavailable),
    };
    let destination = destination_for(image, &url).ok_or("保存先を決められません")?;
    let response = client
        .get(&url)
        .header("Referer", "https://www.pixiv.net/")
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        )
        .send()
        .await
        .map_err(|error| format!("画像サーバーに接続できません: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("画像サーバーが {} を返しました", response.status()));
    }
    // 同じ名前（拡張子が変わらない）なら、検査を通ったものだけがその場で
    // 置き換わる。名前が変わるなら、古いファイルは記録が移ってから消す。
    let bytes =
        save_response_atomically(response, &destination, MAX_ASSET_DOWNLOAD_BYTES, true).await?;
    let new_path = destination.to_string_lossy().to_string();
    let replacement = {
        let _guard = state.library_gate.write().await;
        let state = state.clone();
        let image = image.clone();
        let new_path = new_path.clone();
        let url = url.clone();
        tokio::task::spawn_blocking(move || {
            state.db.replace_reduced_pixiv_image(
                image.asset_id,
                image.download_id,
                &image.local_path,
                &new_path,
                &url,
                mime_for(Path::new(&new_path)),
                bytes as i64,
            )
        })
        .await
        .map_err(|error| format!("Database task failed: {error}"))??
    };
    if !replacement.replaced {
        if new_path != image.local_path {
            let _ = std::fs::remove_file(&destination);
        }
        return Ok(Outcome::Skipped);
    }
    if replacement.old_file_unreferenced {
        if let Err(error) = std::fs::remove_file(&image.local_path) {
            log::warn!(
                "縮小版の画像を消せませんでした: {}: {error}",
                image.local_path
            );
        }
    }
    Ok(Outcome::Upgraded)
}

/// 縮小版で保存された表紙・イラストを、原寸に取り直す。
#[tauri::command]
pub async fn upgrade_pixiv_images_to_original(
    app: tauri::AppHandle,
) -> Result<OriginalImageResult, String> {
    RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "画像の取り直しはすでに実行中です".to_string())?;
    let _run_guard = RunGuard;
    CANCEL.store(false, Ordering::Release);

    let state = app.state::<Arc<AppState>>().inner().clone();
    let client = cdn_client()?;
    let images = reduced_images(state.clone()).await?;
    let total = images.len();
    let (mut completed, mut upgraded, mut unavailable, mut failed) = (0, 0, 0, 0);
    let mut consecutive_failures = 0usize;
    let mut stopped_reason = None;
    let mut changed: Vec<i64> = Vec::new();
    let progress = |phase: &str,
                    completed,
                    upgraded,
                    unavailable,
                    failed,
                    active_label: Option<String>,
                    error: Option<String>| OriginalImageProgress {
        phase: phase.to_string(),
        completed,
        total,
        upgraded,
        unavailable,
        failed,
        active_label,
        error,
    };
    emit_progress(&app, progress("running", 0, 0, 0, 0, None, None));

    'images: for image in &images {
        // 作品の保存や更新が走っている間は待つ。同じ作品の画像を同時に
        // 書き換えないため。
        loop {
            if CANCEL.load(Ordering::Acquire) {
                break 'images;
            }
            match state.jobs().has_active_update_job() {
                Ok(false) => break,
                Ok(true) => tokio::time::sleep(Duration::from_secs(2)).await,
                Err(error) => {
                    stopped_reason = Some(format!("保存処理の状態を確認できません: {error}"));
                    break 'images;
                }
            }
        }
        emit_progress(
            &app,
            progress(
                "running",
                completed,
                upgraded,
                unavailable,
                failed,
                Some(image.title.clone()),
                None,
            ),
        );
        let result = upgrade_one(&state, &client, image).await;
        completed += 1;
        let error = match result {
            Ok(Outcome::Upgraded) => {
                upgraded += 1;
                consecutive_failures = 0;
                if !changed.contains(&image.download_id) {
                    changed.push(image.download_id);
                }
                None
            }
            Ok(Outcome::Unavailable) => {
                unavailable += 1;
                consecutive_failures = 0;
                None
            }
            Ok(Outcome::Skipped) => {
                consecutive_failures = 0;
                None
            }
            Err(error) => {
                failed += 1;
                consecutive_failures += 1;
                log::warn!(
                    "pixiv の画像を原寸に取り直せません（作品 {}）: {error}",
                    image.download_id
                );
                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    stopped_reason = Some(format!(
                        "画像サーバーへの接続が{MAX_CONSECUTIVE_FAILURES}回続けて失敗したので一時停止しました。取り直した分はそのまま残っています。{error}"
                    ));
                }
                Some(error)
            }
        };
        emit_progress(
            &app,
            progress(
                "checked",
                completed,
                upgraded,
                unavailable,
                failed,
                Some(image.title.clone()),
                error,
            ),
        );
        if changed.len() >= 10 {
            let _ = app.emit("covers-repaired", std::mem::take(&mut changed));
        }
        if stopped_reason.is_some() {
            break;
        }
        tokio::time::sleep(PACE).await;
    }
    if !changed.is_empty() {
        let _ = app.emit("covers-repaired", &changed);
    }

    let canceled = CANCEL.load(Ordering::Acquire);
    let remaining = reduced_images(state).await?.len();
    let phase = if canceled {
        "canceled"
    } else if stopped_reason.is_some() {
        "paused"
    } else {
        "complete"
    };
    emit_progress(
        &app,
        progress(
            phase,
            completed,
            upgraded,
            unavailable,
            failed,
            None,
            stopped_reason.clone(),
        ),
    );
    log::info!(
        "pixiv の画像の取り直し: 原寸{upgraded}件、公開元に無し{unavailable}件、失敗{failed}件、残り{remaining}件"
    );
    Ok(OriginalImageResult {
        attempted: completed,
        upgraded,
        unavailable,
        failed,
        canceled,
        stopped_reason,
        remaining,
    })
}

// ---- 新しい棚 ----
//
// 新しい棚では、作品ごとに保存し直す。保存済みの応答に原寸の在り処を足し、同じ応答の保存と
// 同じ道で保存する（URL の変わらない画像・添付は棚から戻し、原寸に変わった画像だけを取りに行く）。
// 棚は画質の違いを版の違いとして扱わないので、版は増えない。

/// 一作品ぶんの、原寸を探した結果。
#[derive(Default)]
struct Lookups {
    found: usize,
    unavailable: usize,
    failed: usize,
    last_error: Option<String>,
}

async fn shelf_reduced_works(
    shelf: Arc<crate::commands::library_v2::LibraryV2>,
) -> Result<Vec<crate::library_original_images::ReducedShelfWork>, String> {
    tokio::task::spawn_blocking(move || {
        shelf
            .library()?
            .reduced_pixiv_works()
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("棚の読み込みが途中で止まりました: {error}"))?
}

/// 新しい棚の、原寸になっていない画像の数と、走っている取り直しの進み具合。
#[tauri::command]
pub async fn library_v2_original_image_status(
    app: tauri::AppHandle,
) -> Result<OriginalImageStatus, String> {
    let shelf = app
        .state::<Arc<crate::commands::library_v2::LibraryV2>>()
        .inner()
        .clone();
    let works = shelf_reduced_works(shelf).await?;
    Ok(OriginalImageStatus {
        covers: works.iter().map(|work| work.covers).sum(),
        illustrations: works.iter().map(|work| work.illustrations).sum(),
        running: RUNNING.load(Ordering::Acquire),
        progress: PROGRESS
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone(),
    })
}

/// 一作品の縮小版を原寸に取り直す。原寸が一枚も見つからなければ保存しない。
async fn upgrade_shelf_work(
    shelf: &Arc<crate::commands::library_v2::LibraryV2>,
    library: &Arc<crate::library::Library>,
    client: &reqwest::Client,
    work: crate::library::LibraryId,
) -> Result<(Lookups, String), String> {
    let (card, raw) = {
        let library = Arc::clone(library);
        tokio::task::spawn_blocking(move || {
            let card = library.work_card(work).map_err(|e| e.to_string())?;
            let raw = library
                .work_original(work, card.snapshot_id)
                .map_err(|e| e.to_string())?;
            Ok::<_, String>((card, raw))
        })
        .await
        .map_err(|error| format!("棚の読み込みが途中で止まりました: {error}"))??
    };
    if card.provider != "pixiv" {
        return Ok((Lookups::default(), card.title));
    }
    let raw = raw.ok_or("取得元の応答が残っていないため、原寸を探せません")?;
    let mut data: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let lookups = std::sync::Mutex::new(Lookups::default());
    crate::downloader::pixiv_originals::add_original_urls_with(&mut data, |url| {
        let lookups = &lookups;
        async move {
            let result = find_original(client, &url).await;
            let mut lookups = lookups.lock().unwrap_or_else(|error| error.into_inner());
            match result {
                Ok(OriginalLookup::Found(found)) => {
                    lookups.found += 1;
                    Some(found)
                }
                Ok(OriginalLookup::Unavailable) => {
                    lookups.unavailable += 1;
                    None
                }
                Err(error) => {
                    lookups.failed += 1;
                    lookups.last_error = Some(error);
                    None
                }
            }
        }
    })
    .await;
    let mut lookups = lookups
        .into_inner()
        .unwrap_or_else(|error| error.into_inner());
    if lookups.found == 0 {
        return Ok((lookups, card.title));
    }
    if let Err(error) = crate::commands::downloader::shelf::save_originals_to_shelf(
        Arc::clone(shelf),
        Arc::clone(library),
        data,
        card.external_key.clone(),
    )
    .await
    {
        // 見つけた原寸は保存できていない。縮小版のまま残る。
        lookups.failed += lookups.found;
        lookups.found = 0;
        lookups.last_error = Some(error);
    }
    Ok((lookups, card.title))
}

/// 新しい棚で、縮小版で保存された表紙・イラストを原寸に取り直す。
#[tauri::command]
pub async fn library_v2_upgrade_pixiv_images_to_original(
    app: tauri::AppHandle,
) -> Result<OriginalImageResult, String> {
    let shelf = app
        .state::<Arc<crate::commands::library_v2::LibraryV2>>()
        .inner()
        .clone();
    // 書き換えるのは正本に切り替えた棚だけ。
    let library = shelf.writable()?;
    RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "画像の取り直しはすでに実行中です".to_string())?;
    let _run_guard = RunGuard;
    CANCEL.store(false, Ordering::Release);

    let state = app.state::<Arc<AppState>>().inner().clone();
    let client = cdn_client()?;
    let works = shelf_reduced_works(Arc::clone(&shelf)).await?;
    let total: usize = works
        .iter()
        .map(|work| work.covers + work.illustrations)
        .sum();
    let (mut completed, mut upgraded, mut unavailable, mut failed) = (0, 0, 0, 0);
    let mut consecutive_failures = 0usize;
    let mut stopped_reason = None;
    let progress = |phase: &str,
                    completed,
                    upgraded,
                    unavailable,
                    failed,
                    active_label: Option<String>,
                    error: Option<String>| OriginalImageProgress {
        phase: phase.to_string(),
        completed,
        total,
        upgraded,
        unavailable,
        failed,
        active_label,
        error,
    };
    emit_progress(&app, progress("running", 0, 0, 0, 0, None, None));

    'works: for reduced in &works {
        // 作品の保存や更新が走っている間は待つ（同じ作品を同時に書き換えない）。
        loop {
            if CANCEL.load(Ordering::Acquire) {
                break 'works;
            }
            match state.jobs().has_active_update_job() {
                Ok(false) => break,
                Ok(true) => tokio::time::sleep(Duration::from_secs(2)).await,
                Err(error) => {
                    stopped_reason = Some(format!("保存処理の状態を確認できません: {error}"));
                    break 'works;
                }
            }
        }
        emit_progress(
            &app,
            progress(
                "running",
                completed,
                upgraded,
                unavailable,
                failed,
                None,
                None,
            ),
        );
        let images = reduced.covers + reduced.illustrations;
        let (lookups, title) =
            match upgrade_shelf_work(&shelf, &library, &client, reduced.work).await {
                Ok(result) => result,
                Err(error) => (
                    Lookups {
                        failed: images,
                        last_error: Some(error),
                        ..Lookups::default()
                    },
                    String::new(),
                ),
            };
        completed += images;
        upgraded += lookups.found;
        unavailable += lookups.unavailable;
        failed += lookups.failed;
        if lookups.failed > 0 && lookups.found == 0 && lookups.unavailable == 0 {
            consecutive_failures += 1;
            log::warn!(
                "pixiv の画像を原寸に取り直せません（作品 {}）: {}",
                reduced.work,
                lookups.last_error.as_deref().unwrap_or_default()
            );
            if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                stopped_reason = Some(format!(
                    "画像サーバーへの接続が{MAX_CONSECUTIVE_FAILURES}回続けて失敗したので一時停止しました。取り直した分はそのまま残っています。{}",
                    lookups.last_error.clone().unwrap_or_default()
                ));
            }
        } else {
            consecutive_failures = 0;
        }
        emit_progress(
            &app,
            progress(
                "checked",
                completed,
                upgraded,
                unavailable,
                failed,
                (!title.is_empty()).then_some(title),
                lookups.last_error,
            ),
        );
        if stopped_reason.is_some() {
            break;
        }
        tokio::time::sleep(PACE).await;
    }
    // 表紙の問い合わせは画面が取り直しの完了で捨てる（棚の作品は ID で引くので、旧棚の通知は使わない）。

    let canceled = CANCEL.load(Ordering::Acquire);
    let remaining = shelf_reduced_works(shelf)
        .await?
        .iter()
        .map(|work| work.covers + work.illustrations)
        .sum();
    let phase = if canceled {
        "canceled"
    } else if stopped_reason.is_some() {
        "paused"
    } else {
        "complete"
    };
    emit_progress(
        &app,
        progress(
            phase,
            completed,
            upgraded,
            unavailable,
            failed,
            None,
            stopped_reason.clone(),
        ),
    );
    log::info!(
        "新しい棚の pixiv の画像の取り直し: 原寸{upgraded}件、公開元に無し{unavailable}件、失敗{failed}件、残り{remaining}件"
    );
    Ok(OriginalImageResult {
        attempted: completed,
        upgraded,
        unavailable,
        failed,
        canceled,
        stopped_reason,
        remaining,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(asset_type: &str, local_path: &str) -> crate::database::queries::ReducedPixivImage {
        crate::database::queries::ReducedPixivImage {
            asset_id: 1,
            download_id: 1,
            title: "作品".into(),
            asset_type: asset_type.into(),
            local_path: local_path.into(),
            original_url: String::new(),
        }
    }

    #[test]
    fn covers_take_the_original_name_and_illustrations_keep_their_stem() {
        let cover = destination_for(
            &image(
                "cover",
                "C:/lib/pixiv/1/v1/data_assets/cover/ci1_abc_master1200.jpg",
            ),
            "https://i.pximg.net/novel-cover-original/img/2024/01/01/00/00/00/ci1_abc.png",
        )
        .unwrap();
        assert_eq!(
            cover,
            Path::new("C:/lib/pixiv/1/v1/data_assets/cover/ci1_abc.png")
        );
        let illust = destination_for(
            &image(
                "illustration",
                "C:/lib/pixiv/1/v1/data_assets/illustrations/70731269-1_p0.jpg",
            ),
            "https://i.pximg.net/img-original/img/2018/09/17/00/00/11/70731269_p0.png",
        )
        .unwrap();
        assert_eq!(
            illust,
            Path::new("C:/lib/pixiv/1/v1/data_assets/illustrations/70731269-1_p0.png")
        );
        assert_eq!(mime_for(&illust), Some("image/png"));
    }
}
