//! 縮小版で保存された pixiv の表紙・イラストと FANBOX の表紙を、原寸に取り直す。
//!
//! 取りに行くのは画像サーバー（`i.pximg.net`・`pixiv.pximg.net`）だけで、pixiv・FANBOX の
//! API もアカウントも使わない。原寸の在り処は保存済みの縮小版の URL から決まる。
//! 決め方は `downloader::pixiv_originals` にある。
//!
//! 版は増やさず、今の版の画像をその場で差し替える。作者が何かを変えたわけ
//! ではないので、版の履歴に「更新」を残すと読み手を誤らせる。
//!
//! 再開位置は持たない。差し替えた画像は縮小版の URL ではなくなるので、
//! 次に走らせたときは残りだけが対象になる。

use crate::downloader::pixiv_originals::{
    cdn_client, find_fanbox_original_cover, find_original, OriginalLookup,
};
use crate::AppState;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
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

/// 走っている取り直しに中止を頼む。今の一枚が終わったところで止まる。
#[tauri::command]
pub fn cancel_original_image_upgrade() -> bool {
    if !RUNNING.load(Ordering::Acquire) {
        return false;
    }
    CANCEL.store(true, Ordering::Release);
    true
}

// ---- 棚 ----
//
// 棚では、作品ごとに保存し直す。保存済みの応答に原寸の在り処を足し、同じ応答の保存と
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
    shelf: Arc<crate::commands::shelf::Shelf>,
) -> Result<Vec<crate::library_original_images::ReducedShelfWork>, String> {
    tokio::task::spawn_blocking(move || {
        shelf
            .library()?
            .reduced_works()
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("棚の読み込みが途中で止まりました: {error}"))?
}

/// 棚の、原寸になっていない画像の数と、走っている取り直しの進み具合。
#[tauri::command]
pub async fn shelf_original_image_status(
    app: tauri::AppHandle,
) -> Result<OriginalImageStatus, String> {
    let shelf = app
        .state::<Arc<crate::commands::shelf::Shelf>>()
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

/// 一回の取り直しの間に確かめた在り処。同じシリーズの作品は同じ表紙を使うことが多いので、
/// 同じ縮小版の URL を作品ごとに確かめ直さない。通信の失敗は覚えない（次の作品で確かめ直す）。
type KnownLookups = Mutex<HashMap<String, OriginalLookup>>;

/// 公開元で見つからなかった縮小版の URL と、確かめた時刻（UNIX 秒）の控え。アプリのデータ
/// フォルダーに置く。無くなっても確かめ直すだけなので、棚には入れない。
const MISSES_FILE: &str = "original-image-misses.json";
/// 公開元で見つからなかった画像を、確かめ直すまでの間。消えた画像はまず戻らないが、
/// 一時的な不調で 404 が返ることもあるので、ずっと覚えはしない。
const MISS_RECHECK: Duration = Duration::from_secs(30 * 24 * 60 * 60);

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// 控えを読む。古くなった記録は捨てる。読めなければ空（すべて確かめ直す）。
fn load_misses(path: &Path, now: u64) -> HashMap<String, u64> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    let mut misses: HashMap<String, u64> = serde_json::from_str(&text).unwrap_or_else(|error| {
        log::warn!("公開元に無かった画像の控えを読めません（確かめ直します）: {error}");
        HashMap::new()
    });
    misses.retain(|_, checked| now.saturating_sub(*checked) < MISS_RECHECK.as_secs());
    misses
}

/// 一回の実行で確かめた結果を控えに反映する。見つかった URL は消し、無かった URL は
/// 確かめた時刻で残す（前から控えにあったものは、その時刻のまま）。
fn merge_misses(
    misses: &mut HashMap<String, u64>,
    known: &HashMap<String, OriginalLookup>,
    now: u64,
) {
    for (url, lookup) in known {
        match lookup {
            OriginalLookup::Unavailable => {
                misses.entry(url.clone()).or_insert(now);
            }
            OriginalLookup::Found(_) => {
                misses.remove(url);
            }
        }
    }
}

async fn remembered<F, Fut>(
    known: &KnownLookups,
    url: String,
    find: F,
) -> Result<OriginalLookup, String>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<OriginalLookup, String>>,
{
    let hit = known
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&url)
        .cloned();
    if let Some(hit) = hit {
        return Ok(hit);
    }
    let result = find(url.clone()).await;
    if let Ok(found) = &result {
        known
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(url, found.clone());
    }
    result
}

/// 一作品の縮小版を原寸に取り直す。原寸が一枚も見つからなければ保存しない。
async fn upgrade_shelf_work<F, Fut>(
    shelf: &Arc<crate::commands::shelf::Shelf>,
    library: &Arc<crate::library::Library>,
    work: crate::library::LibraryId,
    find: F,
) -> Result<(Lookups, String), String>
where
    F: Fn(&str, String) -> Fut,
    Fut: std::future::Future<Output = Result<OriginalLookup, String>>,
{
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
    if !matches!(card.provider.as_str(), "pixiv" | "fanbox") {
        return Ok((Lookups::default(), card.title));
    }
    let raw = raw.ok_or("取得元の応答が残っていないため、原寸を探せません")?;
    let data: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    // 棚がもう原寸を持っている表紙（同じシリーズの表紙）は、画像サーバーに確かめない。
    // 前の実行で取り直した作品の表紙も含むので、実行をまたいでも確かめ直さない。
    let (mut data, cover_from_shelf) = {
        let (library, provider) = (Arc::clone(library), card.provider.clone());
        tokio::task::spawn_blocking(move || {
            let mut data = data;
            let filled = crate::downloader::pixiv_originals::fill_cover_original_from_shelf(
                &library, &mut data, &provider,
            )?;
            Ok::<_, String>((data, filled))
        })
        .await
        .map_err(|error| format!("棚の読み込みが途中で止まりました: {error}"))??
    };
    let lookups = std::sync::Mutex::new(Lookups::default());
    let count = |result: Result<OriginalLookup, String>| {
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
    };
    let lookup = |url| {
        let pending = find(&card.provider, url);
        let count = &count;
        async move { count(pending.await) }
    };
    if card.provider == "pixiv" {
        crate::downloader::pixiv_originals::add_original_urls_with(&mut data, lookup).await;
    } else {
        crate::downloader::pixiv_originals::add_fanbox_original_cover_with(&mut data, lookup).await;
    }
    let mut lookups = lookups
        .into_inner()
        .unwrap_or_else(|error| error.into_inner());
    if cover_from_shelf {
        lookups.found += 1;
    }
    if lookups.found == 0 {
        return Ok((lookups, card.title));
    }
    match crate::commands::downloader::shelf::save_originals_to_shelf(
        Arc::clone(shelf),
        Arc::clone(library),
        data,
        card.provider.clone(),
        card.external_key.clone(),
        card.snapshot_id,
    )
    .await
    {
        Ok(Some(_)) => {}
        // 次回の実行で、現在の保存版に残っている縮小版を選び直す。
        Ok(None) => return Ok((Lookups::default(), card.title)),
        Err(error) => {
            // 見つけた原寸は保存できていない。縮小版のまま残る。
            lookups.failed += lookups.found;
            lookups.found = 0;
            lookups.last_error = Some(error);
        }
    }
    Ok((lookups, card.title))
}

/// 棚で、縮小版で保存された表紙・イラストを原寸に取り直す。
#[tauri::command]
pub async fn shelf_upgrade_images_to_original(
    app: tauri::AppHandle,
) -> Result<OriginalImageResult, String> {
    let shelf = app
        .state::<Arc<crate::commands::shelf::Shelf>>()
        .inner()
        .clone();
    // 書き換えるのは
    let library = shelf.library()?;
    RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "画像の取り直しはすでに実行中です".to_string())?;
    let _run_guard = RunGuard;
    CANCEL.store(false, Ordering::Release);

    let state = app.state::<Arc<AppState>>().inner().clone();
    let client = cdn_client()?;
    // 前の実行で公開元に無かった画像は、しばらく確かめ直さない。
    let misses_path = app
        .path()
        .app_data_dir()
        .map(|dir| dir.join(MISSES_FILE))
        .map_err(|error| log::warn!("公開元に無かった画像の控えの置き場が分かりません: {error}"))
        .ok();
    let started = unix_now();
    let mut misses = misses_path
        .as_deref()
        .map(|path| load_misses(path, started))
        .unwrap_or_default();
    let known: KnownLookups = Mutex::new(
        misses
            .keys()
            .map(|url| (url.clone(), OriginalLookup::Unavailable))
            .collect(),
    );
    let asked = AtomicUsize::new(0);
    let find = |provider: &str, url: String| {
        let pixiv = provider == "pixiv";
        let (known, asked, client) = (&known, &asked, &client);
        async move {
            remembered(known, url, |url| async move {
                asked.fetch_add(1, Ordering::Relaxed);
                if pixiv {
                    find_original(client, &url).await
                } else {
                    find_fanbox_original_cover(client, &url).await
                }
            })
            .await
        }
    };
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
        let asked_before = asked.load(Ordering::Relaxed);
        let (lookups, title) = match upgrade_shelf_work(&shelf, &library, reduced.work, find).await
        {
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
                "画像を原寸に取り直せません（作品 {}）: {}",
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
        // 画像サーバーに一度も問い合わせず、何も保存しなかった作品（無かった画像だけが残る）は
        // 間を空けない。
        if asked.load(Ordering::Relaxed) != asked_before || lookups.found > 0 {
            tokio::time::sleep(PACE).await;
        }
    }
    if let Some(path) = &misses_path {
        let known = known
            .into_inner()
            .unwrap_or_else(|error| error.into_inner());
        merge_misses(&mut misses, &known, unix_now());
        match serde_json::to_vec(&misses) {
            Ok(bytes) => {
                if let Err(error) = crate::server::update::write_atomic(path, &bytes) {
                    log::warn!("公開元に無かった画像の控えを残せません: {error}");
                }
            }
            Err(error) => log::warn!("公開元に無かった画像の控えを作れません: {error}"),
        }
    }
    // 表紙の問い合わせは画面が取り直しの完了で捨てる（棚の作品は ID で引く）。

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
        "棚の画像の取り直し: 原寸{upgraded}件、公開元に無し{unavailable}件、失敗{failed}件、残り{remaining}件"
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
    use crate::commands::shelf::Shelf;
    use crate::downloader::pixiv_originals::COVER_ORIGINAL_FIELD;
    use crate::library::{Library, LibraryId, StagedAsset, WorkSave};
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;

    #[tokio::test]
    async fn original_lookup_does_not_save_over_a_newer_snapshot() {
        const REDUCED: &str =
            "https://i.pximg.net/novel-cover-master/img/2024/01/01/00/00/00/1_master1200.png";
        const ORIGINAL: &str =
            "https://i.pximg.net/novel-cover-original/img/2024/01/01/00/00/00/1.png";
        struct Temp(std::path::PathBuf);
        impl Drop for Temp {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let temp =
            Temp(std::env::temp_dir().join(format!("piep-original-race-{}", LibraryId::now_v7())));
        let library = Arc::new(Library::create(&temp.0.join("shelf")).unwrap());
        let shelf = Arc::new(Shelf::with_test_library(Arc::clone(&library)));
        let save = |text: &str, cover: &str, name: &str| {
            let mut data = json!({"detail": {"title": "題"}, "text": text, "cover_url": REDUCED});
            if cover == ORIGINAL {
                data[COVER_ORIGINAL_FIELD] = json!(ORIGINAL);
            }
            let path = temp.0.join(name);
            image::RgbImage::from_pixel(1, 1, image::Rgb([1, 2, 3]))
                .save(&path)
                .unwrap();
            library
                .save_readable_work(
                    WorkSave {
                        request: LibraryId::now_v7(),
                        provider: "pixiv",
                        key: "1",
                        route: "save",
                        data: &data,
                        assets: vec![StagedAsset {
                            role: "cover".into(),
                            name: name.into(),
                            path,
                            url: Some(cover.into()),
                            mime_type: Some("image/png".into()),
                        }],
                    },
                    None,
                )
                .unwrap()
        };
        let first = save("最初の本文", REDUCED, "1_master1200.png");
        let newer = std::cell::OnceCell::new();
        let (lookups, _) =
            upgrade_shelf_work(&shelf, &library, first.entity_id, |provider, url| {
                assert_eq!(provider, "pixiv");
                assert_eq!(url, REDUCED);
                let newer = &newer;
                async move {
                    // 原寸の在り処の問い合わせ中に、別の保存が新しい本文と原寸の表紙を確定する。
                    // 原寸の実体も棚に置くので、その後の保存経路でも外部通信は起きない。
                    let saved = save("新しい本文", ORIGINAL, "1.png");
                    assert!(saved.created_revision);
                    assert!(newer.set(saved.snapshot_id).is_ok());
                    Ok(OriginalLookup::Found(ORIGINAL.into()))
                }
            })
            .await
            .unwrap();

        let newer = newer.into_inner().unwrap();
        assert_eq!(library.current_snapshot(first.entity_id).unwrap(), newer);
        assert_eq!(library.snapshots(first.entity_id).unwrap().len(), 2);
        assert_eq!(lookups.found, 0);
        assert_eq!(lookups.failed, 0);
        library.verify().unwrap();
    }

    #[tokio::test]
    async fn the_same_reduced_url_is_looked_up_once_but_a_failure_is_tried_again() {
        let known = KnownLookups::default();
        let calls = AtomicUsize::new(0);
        let find = |url: String| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move { Ok(OriginalLookup::Found(format!("{url}-original"))) }
        };
        let first = remembered(&known, "a".into(), find).await;
        let second = remembered(&known, "a".into(), find).await;
        assert_eq!(first, second);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let failing = |_: String| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err::<OriginalLookup, String>("接続できません".into()) }
        };
        assert!(remembered(&known, "b".into(), failing).await.is_err());
        assert!(remembered(&known, "b".into(), failing).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    /// 公開元に無かった画像は控えに残り、30日たつまで確かめ直さない。見つかれば控えから消える。
    #[test]
    fn missing_originals_are_remembered_for_a_while_and_forgotten_when_found() {
        let dir = std::env::temp_dir().join(format!(
            "piep-original-misses-{}",
            crate::library::LibraryId::now_v7()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(MISSES_FILE);
        let day = 24 * 60 * 60;
        let now = 1_000 * day;
        // 控えが無い・壊れているときは、すべて確かめ直す。
        assert!(load_misses(&path, now).is_empty());
        std::fs::write(&path, b"not json").unwrap();
        assert!(load_misses(&path, now).is_empty());

        let mut misses = HashMap::from([
            ("old".to_string(), now - 31 * day),
            ("recent".to_string(), now - 2 * day),
            ("revived".to_string(), now - 2 * day),
        ]);
        let known = HashMap::from([
            ("recent".to_string(), OriginalLookup::Unavailable),
            ("new".to_string(), OriginalLookup::Unavailable),
            ("revived".to_string(), OriginalLookup::Found("x".into())),
        ]);
        merge_misses(&mut misses, &known, now);
        crate::server::update::write_atomic(&path, &serde_json::to_vec(&misses).unwrap()).unwrap();
        let loaded = load_misses(&path, now);
        assert_eq!(loaded.get("recent"), Some(&(now - 2 * day)));
        assert_eq!(loaded.get("new"), Some(&now));
        assert!(!loaded.contains_key("revived"));
        assert!(!loaded.contains_key("old"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
