use crate::database::queries::EntityProfileFreshness;
use crate::database::{
    AcceptCollectionSuggestionInput, AssetEntry, BulkMutationResult, CollectionAdditionResult,
    CollectionNameCandidate, CollectionSuggestion, CollectionSuggestionRequest,
    CollectionSweepResult, DashboardSummary, DbStats, DownloadEntry, EditorDocument, EntityFacet,
    EntityFacetScope, EntityProfileRepairStatus, EntitySeriesPage, EntityVersion, FacetCount,
    FilterFacets, LibraryDiagnostics, LibraryMaintenanceResult, LibraryShelfCounts, NewAsset,
    PersonEntry, ReaderContentPage, ReaderMetadata, ReaderSearchHit, SavedSearch, SavedSearchInput,
    SearchIndexOptimizationResult, SearchIndexStatus, SearchSuggestParams, SearchSuggestResult,
    SearchV2Params, SearchV2Result, SeriesEntry, UpdateCredentials, UpdateTarget,
    UpdateTargetInput, WorkBlockInput, WorkCollection, WorkCollectionInput,
    WorkCollectionMemberInput, WorkCollectionSummary, WorkEditRevision, WorkKey,
};
use crate::AppState;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::Instant;
use tauri::{Emitter, Manager};
use tauri_plugin_opener::OpenerExt;
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::{sleep, Duration};

static ENTITY_REFRESH_LOCK: LazyLock<AsyncMutex<Option<Instant>>> =
    LazyLock::new(|| AsyncMutex::new(None));
static ENTITY_PROFILE_REPAIR_RUNNING: AtomicBool = AtomicBool::new(false);
static ENTITY_PROFILE_REPAIR_CANCEL: AtomicBool = AtomicBool::new(false);
static SEARCH_REBUILD_JOBS: LazyLock<std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>> =
    LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));
static SEARCH_REBUILD_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const MAX_COLLECTION_COVER_BYTES: u64 = 20 * 1024 * 1024;
const MAX_COLLECTION_COVER_PIXELS: u64 = 100_000_000;

async fn run_db_blocking<T, F>(app: tauri::AppHandle, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(Arc<AppState>) -> Result<T, String> + Send + 'static,
{
    let state = app.state::<Arc<AppState>>().inner().clone();
    tokio::task::spawn_blocking(move || f(state))
        .await
        .map_err(|e| format!("Database task failed: {}", e))?
}

async fn run_library_write_blocking<T, F>(app: tauri::AppHandle, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(Arc<AppState>) -> Result<T, String> + Send + 'static,
{
    let state = app.state::<Arc<AppState>>().inner().clone();
    let guard = state.library_gate.clone().write_owned().await;
    tokio::task::spawn_blocking(move || {
        let _guard = guard;
        f(state)
    })
    .await
    .map_err(|e| format!("Database task failed: {e}"))?
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshEntityProfileParams {
    pub entity_type: String,
    pub source: String,
    pub source_key: String,
    pub force: Option<bool>,
    pub refresh_token: Option<String>,
    pub cookie: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityProfileRepairProgress {
    phase: String,
    completed: usize,
    total: usize,
    repaired: usize,
    failed: usize,
    active_label: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityProfileRepairResult {
    attempted: usize,
    repaired: usize,
    failed: usize,
    canceled: bool,
    remaining: i64,
}

struct EntityProfileRepairRunGuard;

impl Drop for EntityProfileRepairRunGuard {
    fn drop(&mut self) {
        ENTITY_PROFILE_REPAIR_RUNNING.store(false, Ordering::Release);
        ENTITY_PROFILE_REPAIR_CANCEL.store(false, Ordering::Release);
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRebuildJobOptions {
    batch_size: Option<i64>,
    include_semantic: Option<bool>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRebuildProgress {
    job_id: String,
    /// "manual" when someone pressed the button, "automatic" when the app
    /// noticed the index was behind and caught it up on its own. The wording
    /// the UI uses differs, and an automatic run must not read as an error.
    origin: String,
    status: String,
    total_downloads: i64,
    indexed_downloads: i64,
    pending_downloads: i64,
    is_complete: bool,
    phase: String,
    /// Documents handled by this run, which is what the progress bar tracks.
    /// `indexed_downloads` counts the whole library and moves in commit-sized
    /// steps, so on its own it looks stalled for minutes at a time.
    processed: i64,
    processed_total: i64,
    failed: i64,
    embedding_provider: String,
    gpu_enabled: bool,
    throughput_per_sec: Option<f64>,
    eta_seconds: Option<f64>,
    error: Option<String>,
}

#[tauri::command]
pub async fn search_downloads_v2(
    app: tauri::AppHandle,
    params: SearchV2Params,
) -> Result<SearchV2Result, String> {
    run_db_blocking(app, move |state| state.db.search_downloads_v2(&params)).await
}

#[tauri::command]
pub async fn search_suggest(
    app: tauri::AppHandle,
    params: SearchSuggestParams,
) -> Result<SearchSuggestResult, String> {
    run_db_blocking(app, move |state| state.db.search_suggest(&params)).await
}

#[tauri::command]
pub async fn search_rebuild_index(
    app: tauri::AppHandle,
    job_options: Option<SearchRebuildJobOptions>,
) -> Result<String, String> {
    let options = job_options.unwrap_or(SearchRebuildJobOptions {
        batch_size: None,
        include_semantic: None,
    });
    let semantic_enabled = match options.include_semantic {
        Some(enabled) => {
            run_db_blocking(app.clone(), move |state| {
                state
                    .db
                    .set_semantic_index_enabled(enabled)
                    .map(|_| enabled)
            })
            .await?
        }
        None => run_db_blocking(app.clone(), |state| state.db.semantic_index_enabled()).await?,
    };
    let rebuild_options = crate::database::queries::SearchIndexRebuildOptions {
        chunk_size: options.batch_size.unwrap_or(64).clamp(8, 512) as usize,
        include_semantic: semantic_enabled,
        ..Default::default()
    };
    spawn_rebuild_job(app, rebuild_options, "manual")
}

/// Persistent capability switch shared by manual rebuilds, automatic
/// maintenance, and every incremental work update.
#[tauri::command]
pub async fn search_set_semantic_enabled(
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<SearchIndexStatus, String> {
    run_db_blocking(app, move |state| {
        state.db.set_semantic_index_enabled(enabled)
    })
    .await
}

/// Brings the search index up to date in the background, without being asked.
///
/// The index is derived data: a format change, an interrupted run or a restore
/// can leave it behind, and there is nothing useful a person can decide about
/// that. Making them find a button and wait for it is the app failing to do its
/// own housekeeping, so this runs at launch and only announces itself while it
/// has work to do.
pub fn start_automatic_index_maintenance(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Long enough for the first screen to settle; the work then competes
        // with nothing the user is waiting on.
        sleep(Duration::from_millis(1_200)).await;

        // 記録より先に実物を見る。索引が消えているのに「索引済み」の記録だけが
        // 残っていると、待っている件数は 0 と出て、この仕事は何もせずに帰る。
        let resync_started = Instant::now();
        match run_db_blocking(app.clone(), |state| state.db.resync_search_index_state()).await {
            Ok(cleared) if cleared > 0 => {
                log::info!("全文索引が失われていたため、{cleared}件を作り直しの対象へ戻しました");
            }
            Ok(_) => {}
            Err(error) => log::warn!("索引の記録を実物と突き合わせられません: {error}"),
        }
        let resync_elapsed = resync_started.elapsed();

        let status_started = Instant::now();
        let status =
            match run_db_blocking(app.clone(), |state| state.db.get_search_index_status()).await {
                Ok(status) => status,
                Err(error) => {
                    log::warn!("Automatic index maintenance skipped: {error}");
                    return;
                }
            };
        let status_elapsed = status_started.elapsed();
        let pending = if status.semantic_enabled {
            status
                .pending_downloads
                .max(status.semantic_pending_downloads)
        } else {
            status.pending_downloads
        };
        log::info!(
            "Automatic index preflight: resync {:.1} ms, status {:.1} ms, pending {pending}",
            resync_elapsed.as_secs_f64() * 1_000.0,
            status_elapsed.as_secs_f64() * 1_000.0,
        );
        if pending <= 0 {
            return;
        }

        log::info!("Automatic index maintenance starting for {pending} works");
        let options = crate::database::queries::SearchIndexRebuildOptions {
            include_semantic: status.semantic_enabled,
            ..Default::default()
        };
        if let Err(error) = spawn_rebuild_job(app, options, "automatic") {
            log::warn!("Automatic index maintenance could not start: {error}");
        }
    });
}

fn spawn_rebuild_job(
    app: tauri::AppHandle,
    rebuild_options: crate::database::queries::SearchIndexRebuildOptions,
    origin: &'static str,
) -> Result<String, String> {
    let job_id = format!(
        "search-index-{}-{}",
        chrono::Utc::now().timestamp_millis(),
        SEARCH_REBUILD_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut jobs = SEARCH_REBUILD_JOBS.lock().map_err(|e| e.to_string())?;
        // Two writers on one index would fight over the same lock, and the
        // second would simply wait. One run at a time is also what the user
        // sees, so a manual start during automatic maintenance joins the one
        // already running instead of queueing behind it.
        if let Some((existing, _)) = jobs.iter().next() {
            return Ok(existing.clone());
        }
        jobs.insert(job_id.clone(), cancel.clone());
    }

    let app_for_task = app.clone();
    let job_id_for_task = job_id.clone();

    tauri::async_runtime::spawn(async move {
        let started_at = Instant::now();
        let state = app_for_task.state::<Arc<AppState>>().inner().clone();
        let gate_wait_started = Instant::now();
        let library_gate = state.library_gate.clone().write_owned().await;
        let gate_wait_elapsed = gate_wait_started.elapsed();
        let gate_hold_started = Instant::now();
        let progress_app = app_for_task.clone();
        let progress_job = job_id_for_task.clone();

        // The whole rebuild is one blocking task. Driving it as a sequence of
        // short async batches meant re-counting the library between every one
        // of them, and rebuilding the index writer with it.
        let outcome = tokio::task::spawn_blocking(move || {
            let result = {
                let _library_gate = library_gate;
                let cancel_flag = cancel.clone();
                let mut last_emit = Instant::now() - Duration::from_secs(1);
                state.db.rebuild_search_index(
                    rebuild_options,
                    &move || cancel_flag.load(Ordering::Relaxed),
                    |progress| {
                        // Roughly ten updates a second: enough for a bar that
                        // visibly moves, far short of flooding the event channel.
                        let finished = progress.phase != "indexing";
                        if !finished && last_emit.elapsed() < Duration::from_millis(100) {
                            return;
                        }
                        last_emit = Instant::now();
                        let throughput = (progress.elapsed_secs > 0.5)
                            .then(|| progress.processed as f64 / progress.elapsed_secs);
                        let eta = throughput.filter(|rate| *rate > 0.0).map(|rate| {
                            ((progress.total - progress.processed).max(0) as f64 / rate).max(0.0)
                        });
                        let _ = progress_app.emit(
                            "search-index-progress",
                            SearchRebuildProgress {
                                job_id: progress_job.clone(),
                                origin: origin.to_string(),
                                status: "running".to_string(),
                                total_downloads: 0,
                                indexed_downloads: 0,
                                pending_downloads: (progress.total - progress.processed).max(0),
                                is_complete: false,
                                phase: progress.phase.to_string(),
                                processed: progress.processed,
                                processed_total: progress.total,
                                failed: progress.failed,
                                embedding_provider: String::new(),
                                gpu_enabled: false,
                                throughput_per_sec: throughput,
                                eta_seconds: eta,
                                error: None,
                            },
                        );
                    },
                )
            };
            (result, gate_hold_started.elapsed())
        })
        .await;

        let (result, gate_hold_elapsed) = match outcome {
            Ok((result, elapsed)) => (result, Some(elapsed)),
            Err(error) => (Err(format!("Search rebuild task failed: {error}")), None),
        };
        match gate_hold_elapsed {
            Some(elapsed) => log::info!(
                "Search index {origin} job {job_id_for_task}: gate wait {:.1} ms, gate held {:.1} ms, rebuild task elapsed {:.1} ms",
                gate_wait_elapsed.as_secs_f64() * 1_000.0,
                elapsed.as_secs_f64() * 1_000.0,
                started_at.elapsed().as_secs_f64() * 1_000.0,
            ),
            None => log::warn!(
                "Search index {origin} job {job_id_for_task}: gate wait {:.1} ms, gate held unknown (blocking task failed), rebuild task elapsed {:.1} ms",
                gate_wait_elapsed.as_secs_f64() * 1_000.0,
                started_at.elapsed().as_secs_f64() * 1_000.0,
            ),
        }

        // One final status read, rather than one per batch: the three library
        // wide COUNT(*) queries behind it are what made progress reporting
        // itself a measurable share of the rebuild.
        let status = run_db_blocking(app_for_task.clone(), |state| {
            state.db.get_search_index_status()
        })
        .await;

        match (result, status) {
            (Ok(outcome), Ok(status)) => emit_search_index_progress(
                &app_for_task,
                &job_id_for_task,
                origin,
                if outcome.canceled {
                    "canceled"
                } else {
                    "completed"
                },
                status,
                outcome.processed,
                outcome.failed,
                started_at.elapsed().as_secs_f64(),
            ),
            (Err(error), _) | (_, Err(error)) => {
                let _ = app_for_task.emit(
                    "search-index-progress",
                    serde_json::json!({
                        "jobId": job_id_for_task,
                        "origin": origin,
                        "status": "failed",
                        "phase": "failed",
                        "error": error,
                    }),
                );
            }
        }

        if let Ok(mut jobs) = SEARCH_REBUILD_JOBS.lock() {
            jobs.remove(&job_id_for_task);
        }
    });

    Ok(job_id)
}

#[tauri::command]
pub async fn search_cancel_rebuild_index(job_id: String) -> Result<(), String> {
    let jobs = SEARCH_REBUILD_JOBS.lock().map_err(|e| e.to_string())?;
    let Some(cancel) = jobs.get(&job_id) else {
        return Err("Search rebuild job not found".to_string());
    };
    cancel.store(true, Ordering::Relaxed);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_search_index_progress(
    app: &tauri::AppHandle,
    job_id: &str,
    origin: &str,
    status: &str,
    index: SearchIndexStatus,
    processed: i64,
    failed: i64,
    elapsed_secs: f64,
) {
    let _ = app.emit(
        "search-index-progress",
        SearchRebuildProgress {
            job_id: job_id.to_string(),
            origin: origin.to_string(),
            status: status.to_string(),
            total_downloads: index.total_downloads,
            indexed_downloads: index.indexed_downloads,
            pending_downloads: index.pending_downloads,
            is_complete: index.is_complete,
            phase: if status == "completed" {
                "ready".to_string()
            } else {
                index.phase
            },
            processed,
            processed_total: processed,
            failed,
            embedding_provider: index.embedding_provider,
            gpu_enabled: index.gpu_enabled,
            throughput_per_sec: (elapsed_secs > 0.0).then(|| processed as f64 / elapsed_secs),
            eta_seconds: Some(0.0),
            error: None,
        },
    );
}

#[tauri::command]
pub async fn db_get_downloads(
    app: tauri::AppHandle,
    ids: Vec<i64>,
) -> Result<Vec<DownloadEntry>, String> {
    run_db_blocking(app, move |state| state.db.get_downloads(&ids)).await
}

#[tauri::command]
pub async fn db_get_assets(
    app: tauri::AppHandle,
    download_id: i64,
) -> Result<Vec<AssetEntry>, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.get_assets(download_id)
}

#[tauri::command]
pub async fn db_delete_download(app: tauri::AppHandle, id: i64) -> Result<(), String> {
    run_library_write_blocking(app, move |state| state.db.delete_download(id)).await
}

#[tauri::command]
pub async fn db_delete_downloads(
    app: tauri::AppHandle,
    ids: Vec<i64>,
) -> Result<BulkMutationResult, String> {
    run_library_write_blocking(app, move |state| state.db.delete_downloads(&ids)).await
}

#[tauri::command]
pub async fn db_delete_downloads_for_search(
    app: tauri::AppHandle,
    params: SearchV2Params,
) -> Result<BulkMutationResult, String> {
    run_library_write_blocking(app, move |state| {
        state.db.delete_downloads_for_search(&params)
    })
    .await
}

#[tauri::command]
pub async fn db_get_stats(app: tauri::AppHandle) -> Result<DbStats, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.get_stats()
}

#[tauri::command]
pub async fn db_get_library_diagnostics(
    app: tauri::AppHandle,
) -> Result<LibraryDiagnostics, String> {
    let emitter = app.clone();
    run_db_blocking(app, move |state| {
        state.db.get_library_diagnostics_with_progress(&|progress| {
            let _ = emitter.emit(
                "library-diagnostics-progress",
                serde_json::json!({
                    "phase": progress.phase,
                    "step": progress.step,
                    "total": progress.total,
                }),
            );
        })
    })
    .await
}

#[tauri::command]
pub async fn db_maintain_library(
    app: tauri::AppHandle,
    compact: bool,
) -> Result<LibraryMaintenanceResult, String> {
    let _ = app.emit(
        "library-maintenance-progress",
        serde_json::json!({ "phase": if compact { "compacting" } else { "optimizing" } }),
    );
    let result = run_library_write_blocking(app.clone(), move |state| {
        state.db.maintain_library_database(compact)
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

#[tauri::command]
pub async fn db_optimize_search_index(
    app: tauri::AppHandle,
) -> Result<SearchIndexOptimizationResult, String> {
    let segments = run_db_blocking(app.clone(), |state| state.db.search_index_segment_count())
        .await
        .unwrap_or(0);
    let _ = app.emit(
        "search-index-optimization-progress",
        serde_json::json!({ "phase": "preflight", "segments": segments }),
    );
    let _ = app.emit(
        "search-index-optimization-progress",
        serde_json::json!({ "phase": "merging", "segments": segments }),
    );
    let result =
        run_library_write_blocking(app.clone(), move |state| state.db.optimize_search_index())
            .await;
    let _ = app.emit(
        "search-index-optimization-progress",
        serde_json::json!({
            "phase": if result.is_ok() { "complete" } else { "failed" },
            "segments": segments,
            "error": result.as_ref().err(),
        }),
    );
    result
}

#[tauri::command]
pub async fn db_get_library_shelf_counts(
    app: tauri::AppHandle,
    reading_ids: Option<Vec<i64>>,
) -> Result<LibraryShelfCounts, String> {
    let reading_ids = reading_ids.unwrap_or_default();
    run_db_blocking(app, move |state| {
        state.db.get_library_shelf_counts(&reading_ids)
    })
    .await
}

#[tauri::command]
pub async fn db_list_entity_series(
    app: tauri::AppHandle,
    source: String,
    source_key: String,
    limit: Option<i64>,
) -> Result<Vec<EntityFacet>, String> {
    run_db_blocking(app, move |state| {
        state
            .db
            .list_entity_series(&source, &source_key, limit.unwrap_or(60))
    })
    .await
}

#[tauri::command]
pub async fn db_list_entity_series_paged(
    app: tauri::AppHandle,
    source: String,
    source_key: String,
    query: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
) -> Result<EntitySeriesPage, String> {
    run_db_blocking(app, move |state| {
        state.db.list_entity_series_paged(
            &source,
            &source_key,
            query.as_deref(),
            limit.unwrap_or(60),
            cursor.as_deref(),
        )
    })
    .await
}

#[tauri::command]
pub async fn db_list_entity_tags(
    app: tauri::AppHandle,
    kind: String,
    source: String,
    source_key: String,
    limit: Option<i64>,
) -> Result<Vec<FacetCount>, String> {
    run_db_blocking(app, move |state| {
        state
            .db
            .list_entity_tags(&kind, &source, &source_key, limit.unwrap_or(40))
    })
    .await
}

#[tauri::command]
pub async fn db_list_saved_searches(app: tauri::AppHandle) -> Result<Vec<SavedSearch>, String> {
    run_db_blocking(app, move |state| state.db.list_saved_searches()).await
}

#[tauri::command]
pub async fn db_upsert_saved_search(
    app: tauri::AppHandle,
    input: SavedSearchInput,
) -> Result<SavedSearch, String> {
    run_library_write_blocking(app, move |state| state.db.upsert_saved_search(&input)).await
}

#[tauri::command]
pub async fn db_delete_saved_search(app: tauri::AppHandle, id: i64) -> Result<bool, String> {
    run_library_write_blocking(app, move |state| state.db.delete_saved_search(id)).await
}

#[tauri::command]
pub async fn db_list_work_collections(
    app: tauri::AppHandle,
) -> Result<Vec<WorkCollectionSummary>, String> {
    run_db_blocking(app, move |state| state.db.list_work_collections()).await
}

#[tauri::command]
pub async fn db_get_work_collection(
    app: tauri::AppHandle,
    collection_id: String,
) -> Result<WorkCollection, String> {
    run_db_blocking(app, move |state| {
        state.db.get_work_collection(&collection_id)
    })
    .await
}

fn import_collection_cover(app_data_dir: &Path, source: &Path) -> Result<PathBuf, String> {
    let metadata = std::fs::metadata(source).map_err(|e| format!("表紙画像を読めません: {e}"))?;
    if !metadata.is_file() {
        return Err("表紙画像にファイルを指定してください".to_string());
    }
    if metadata.len() > MAX_COLLECTION_COVER_BYTES {
        return Err("表紙画像は20 MB以下にしてください".to_string());
    }
    let bytes = std::fs::read(source).map_err(|e| format!("表紙画像を読めません: {e}"))?;
    if bytes.len() as u64 > MAX_COLLECTION_COVER_BYTES {
        return Err("表紙画像は20 MB以下にしてください".to_string());
    }
    let reader = image::ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| format!("表紙画像の形式を確認できません: {e}"))?;
    let format = reader
        .format()
        .ok_or_else(|| "PNG、JPEG、WebPの画像を選んでください".to_string())?;
    let extension = match format {
        image::ImageFormat::Png => "png",
        image::ImageFormat::Jpeg => "jpg",
        image::ImageFormat::WebP => "webp",
        _ => return Err("PNG、JPEG、WebPの画像を選んでください".to_string()),
    };
    let (width, height) = reader
        .into_dimensions()
        .map_err(|e| format!("表紙画像が壊れています: {e}"))?;
    if u64::from(width).saturating_mul(u64::from(height)) > MAX_COLLECTION_COVER_PIXELS {
        return Err("表紙画像の縦横サイズが大きすぎます".to_string());
    }

    let digest = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let directory = app_data_dir.join("collection-covers");
    std::fs::create_dir_all(&directory)
        .map_err(|e| format!("表紙画像の保存先を作れません: {e}"))?;
    let destination = directory.join(format!("{digest}.{extension}"));
    if destination.exists() {
        return Ok(destination);
    }
    let staging = directory.join(format!(".{digest}.{:016x}.part", rand::random::<u64>()));
    let publish = (|| -> Result<(), String> {
        use std::io::Write;
        let mut output = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&staging)
            .map_err(|e| format!("表紙画像の一時ファイルを作れません: {e}"))?;
        output
            .write_all(&bytes)
            .and_then(|_| output.sync_all())
            .map_err(|e| format!("表紙画像を保存できません: {e}"))?;
        drop(output);
        match std::fs::rename(&staging, &destination) {
            Ok(()) => Ok(()),
            Err(_) if destination.exists() => {
                let _ = std::fs::remove_file(&staging);
                Ok(())
            }
            Err(e) => Err(format!("表紙画像を確定できません: {e}")),
        }
    })();
    if publish.is_err() {
        let _ = std::fs::remove_file(&staging);
    }
    publish.map(|_| destination)
}

#[tauri::command]
pub async fn db_upsert_work_collection(
    app: tauri::AppHandle,
    mut input: WorkCollectionInput,
) -> Result<WorkCollection, String> {
    let source_path = input
        .cover_image_path_patch()
        .flatten()
        .filter(|path| !path.trim().is_empty())
        .map(str::to_string);
    if let Some(source_path) = source_path {
        let app_data_dir = app
            .path()
            .app_data_dir()
            .map_err(|e| format!("アプリの保存先を確認できません: {e}"))?;
        let managed_path = tokio::task::spawn_blocking(move || {
            import_collection_cover(&app_data_dir, Path::new(&source_path))
        })
        .await
        .map_err(|e| format!("表紙画像の取り込み処理に失敗しました: {e}"))??;
        input.cover_image_path = Some(managed_path.to_string_lossy().to_string());
    }
    run_library_write_blocking(app, move |state| state.db.upsert_work_collection(&input)).await
}

#[tauri::command]
pub async fn db_delete_work_collection(
    app: tauri::AppHandle,
    collection_id: String,
) -> Result<(), String> {
    run_library_write_blocking(app, move |state| {
        state.db.delete_work_collection(&collection_id)
    })
    .await
}

#[tauri::command]
pub async fn db_add_work_collection_members(
    app: tauri::AppHandle,
    collection_id: String,
    members: Vec<WorkCollectionMemberInput>,
) -> Result<WorkCollection, String> {
    run_library_write_blocking(app, move |state| {
        state
            .db
            .add_work_collection_members(&collection_id, &members)
    })
    .await
}

/// 棚の複数選択から、選んだ作品をそのまま束へ入れる。
#[tauri::command]
pub async fn db_add_downloads_to_collection(
    app: tauri::AppHandle,
    collection_id: String,
    download_ids: Vec<i64>,
) -> Result<WorkCollection, String> {
    run_library_write_blocking(app, move |state| {
        state
            .db
            .add_downloads_to_collection(&collection_id, &download_ids)
    })
    .await
}

/// 選んだ作品から、新しい束をひとつ作る。
#[tauri::command]
pub async fn db_create_collection_from_downloads(
    app: tauri::AppHandle,
    name: String,
    collection_kind: String,
    download_ids: Vec<i64>,
) -> Result<WorkCollection, String> {
    run_library_write_blocking(app, move |state| {
        state
            .db
            .create_collection_from_downloads(&name, &collection_kind, &download_ids)
    })
    .await
}

/// すでにあるコレクションに、名前の案を出し直す。保存はしない。
#[tauri::command]
pub async fn db_propose_collection_names(
    app: tauri::AppHandle,
    collection_id: String,
) -> Result<Vec<CollectionNameCandidate>, String> {
    run_db_blocking(app, move |state| {
        state.db.collection_name_proposals(&collection_id)
    })
    .await
}

/// すでにあるコレクションの名前と説明を、モデルにも考えてもらう。
///
/// 提案のときと同じく、**足すだけで置き換えない**。返ってきた案を採るかどうかは
/// 利用者が決める。送るのは題名・作者・タグ・シリーズ名だけで、本文は送らない。
#[tauri::command]
pub async fn db_name_collection_with_model(
    app: tauri::AppHandle,
    collection_id: String,
    engine: crate::assist::AssistEngine,
) -> Result<crate::assist::GeneratedAssist<crate::assist::NamedBundle>, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let works =
        tokio::task::spawn_blocking(move || state.db.collection_naming_works_for(&collection_id))
            .await
            .map_err(|e| format!("Database task failed: {e}"))??;
    let provenance = crate::assist::generation_provenance(
        &engine,
        crate::assist::FEATURE_COLLECTION_NAMING,
        &works,
    )?;
    let value = crate::assist::name_bundle(&engine, &works).await?;
    Ok(crate::assist::GeneratedAssist { value, provenance })
}

/// 設定したエンジンを、実際の仕事で試す。
///
/// `collection_id` を渡すとその束で、渡さなければ棚から拾った作品で試す。
/// **保存する前に、何が返るのかを見せる**ためのもの。つながることと
/// 使えることは別である。
#[tauri::command]
pub async fn db_try_naming_engine(
    app: tauri::AppHandle,
    engine: crate::assist::AssistEngine,
    collection_id: Option<String>,
) -> Result<crate::assist::TrialResult, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let works = tokio::task::spawn_blocking(move || match collection_id {
        Some(id) => state.db.collection_naming_works_for(&id),
        None => state.db.sample_naming_works(),
    })
    .await
    .map_err(|e| format!("Database task failed: {e}"))??;
    if works.is_empty() {
        return Err("試すための作品がライブラリにありません".to_string());
    }
    crate::assist::try_engine(&engine, &works).await
}

/// 提案の名前を、利用者が選んだモデルにも考えてもらう。
///
/// 返ってきた案は決定せず、モデル候補だけを更新する。題名・タグなどから作った
/// 通常候補は置き換えない。失敗しても提案そのものは壊れない。
#[tauri::command]
pub async fn db_name_collection_suggestion(
    app: tauri::AppHandle,
    suggestion_id: String,
    engine: crate::assist::AssistEngine,
) -> Result<CollectionSuggestion, String> {
    // 材料を読むところと、外へ送るところを分ける。送っているあいだ
    // ライブラリの錠を握ったままにしない。
    let state = app.state::<Arc<AppState>>().inner().clone();
    let ids = {
        let suggestion_id = suggestion_id.clone();
        let state = state.clone();
        tokio::task::spawn_blocking(move || {
            state
                .db
                .list_collection_suggestions(Some("all"))
                .map(|values| {
                    values
                        .into_iter()
                        .find(|value| value.id == suggestion_id)
                        .map(|value| {
                            value
                                .members
                                .into_iter()
                                .filter_map(|member| member.download_id)
                                .collect::<Vec<_>>()
                        })
                })
        })
        .await
        .map_err(|e| format!("Database task failed: {e}"))??
        .ok_or_else(|| "Collection suggestion not found".to_string())?
    };
    let works = {
        let state = state.clone();
        tokio::task::spawn_blocking(move || state.db.collection_naming_works(&ids))
            .await
            .map_err(|e| format!("Database task failed: {e}"))??
    };
    let provenance = crate::assist::generation_provenance(
        &engine,
        crate::assist::FEATURE_COLLECTION_NAMING,
        &works,
    )?;
    let named = crate::assist::name_bundle(&engine, &works).await?;
    run_library_write_blocking(app, move |state| {
        state
            .db
            .attach_llm_name_option(&suggestion_id, &named, &provenance)
    })
    .await
}

/// 走査で出た候補を、まとめて閉じる。`track` を渡すとその系統だけ。
#[tauri::command]
pub async fn db_dismiss_swept_suggestions(
    app: tauri::AppHandle,
    track: Option<String>,
) -> Result<usize, String> {
    run_library_write_blocking(app, move |state| {
        state.db.dismiss_swept_suggestions(track.as_deref())
    })
    .await
}

/// 棚全体を走査して、束の候補を作り直す。
///
/// 意味索引を全部読むので、開いた瞬間に走らせるものではない。利用者が
/// 「探す」と言ったときだけ動かす。
#[tauri::command]
pub async fn db_sweep_collection_candidates(
    app: tauri::AppHandle,
) -> Result<CollectionSweepResult, String> {
    run_library_write_blocking(app, move |state| state.db.sweep_collection_candidates()).await
}

/// すでにある束へ、あとから入れるとよさそうな作品を探す。
///
/// 保存はしない。読むだけの操作なので、棚の書き込み権は取らない。
#[tauri::command]
pub async fn db_suggest_collection_additions(
    app: tauri::AppHandle,
    collection_id: String,
) -> Result<CollectionAdditionResult, String> {
    run_db_blocking(app, move |state| {
        state.db.suggest_collection_additions(&collection_id)
    })
    .await
}

/// 束の並びを、投稿日順か題名の連番順に一度で整える。
#[tauri::command]
pub async fn db_sort_work_collection_members(
    app: tauri::AppHandle,
    collection_id: String,
    mode: String,
) -> Result<WorkCollection, String> {
    run_library_write_blocking(app, move |state| {
        state.db.sort_work_collection_members(&collection_id, &mode)
    })
    .await
}

#[tauri::command]
pub async fn db_remove_work_collection_members(
    app: tauri::AppHandle,
    collection_id: String,
    members: Vec<WorkKey>,
) -> Result<WorkCollection, String> {
    run_library_write_blocking(app, move |state| {
        state
            .db
            .remove_work_collection_members(&collection_id, &members)
    })
    .await
}

#[tauri::command]
pub async fn db_reorder_work_collection_members(
    app: tauri::AppHandle,
    collection_id: String,
    members: Vec<WorkKey>,
) -> Result<WorkCollection, String> {
    run_library_write_blocking(app, move |state| {
        state
            .db
            .reorder_work_collection_members(&collection_id, &members)
    })
    .await
}

#[tauri::command]
pub async fn db_list_collections_for_work(
    app: tauri::AppHandle,
    source: String,
    source_id: String,
) -> Result<Vec<WorkCollectionSummary>, String> {
    run_db_blocking(app, move |state| {
        state.db.list_collections_for_work(&source, &source_id)
    })
    .await
}

#[tauri::command]
pub async fn db_list_collections_for_person(
    app: tauri::AppHandle,
    source: String,
    person_key: String,
) -> Result<Vec<WorkCollectionSummary>, String> {
    run_db_blocking(app, move |state| {
        state.db.list_collections_for_person(&source, &person_key)
    })
    .await
}

#[tauri::command]
pub async fn db_list_collections_for_series(
    app: tauri::AppHandle,
    source: String,
    series_key: String,
) -> Result<Vec<WorkCollectionSummary>, String> {
    run_db_blocking(app, move |state| {
        state.db.list_collections_for_series(&source, &series_key)
    })
    .await
}

#[tauri::command]
pub async fn db_generate_collection_suggestion(
    app: tauri::AppHandle,
    request: CollectionSuggestionRequest,
) -> Result<CollectionSuggestion, String> {
    run_library_write_blocking(app, move |state| {
        state.db.generate_collection_suggestion(&request)
    })
    .await
}

#[tauri::command]
pub async fn db_list_collection_suggestions(
    app: tauri::AppHandle,
    state_filter: Option<String>,
) -> Result<Vec<CollectionSuggestion>, String> {
    run_db_blocking(app, move |state| {
        state
            .db
            .list_collection_suggestions(state_filter.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn db_dismiss_collection_suggestion(
    app: tauri::AppHandle,
    suggestion_id: String,
) -> Result<bool, String> {
    run_library_write_blocking(app, move |state| {
        state.db.dismiss_collection_suggestion(&suggestion_id)
    })
    .await
}

#[tauri::command]
pub async fn db_accept_collection_suggestion(
    app: tauri::AppHandle,
    input: AcceptCollectionSuggestionInput,
) -> Result<WorkCollection, String> {
    run_library_write_blocking(app, move |state| {
        state.db.accept_collection_suggestion(&input)
    })
    .await
}

#[tauri::command]
pub async fn db_reject_collection_suggestion(
    app: tauri::AppHandle,
    suggestion_id: String,
    member_keys: Option<Vec<WorkKey>>,
) -> Result<bool, String> {
    run_library_write_blocking(app, move |state| {
        state
            .db
            .reject_collection_suggestion(&suggestion_id, member_keys.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn db_get_dashboard_summary(app: tauri::AppHandle) -> Result<DashboardSummary, String> {
    run_db_blocking(app, move |state| state.db.get_dashboard_summary()).await
}

#[tauri::command]
pub async fn db_get_filter_facets(
    app: tauri::AppHandle,
    include_entities: Option<bool>,
) -> Result<FilterFacets, String> {
    let include_entities = include_entities.unwrap_or(true);
    run_db_blocking(app, move |state| {
        state.db.get_filter_facets_with(include_entities)
    })
    .await
}

#[tauri::command]
pub async fn db_get_search_index_status(
    app: tauri::AppHandle,
) -> Result<SearchIndexStatus, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.get_search_index_status()
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn db_search_entity_facets(
    app: tauri::AppHandle,
    kind: String,
    query: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    filters: Option<SearchV2Params>,
    sort_by: Option<String>,
    sort_order: Option<String>,
    scope: Option<EntityFacetScope>,
) -> Result<Vec<EntityFacet>, String> {
    run_db_blocking(app, move |state| {
        state.db.search_entity_facets(
            &kind,
            query.as_deref(),
            limit.unwrap_or(60),
            offset.unwrap_or(0),
            filters.as_ref(),
            sort_by.as_deref(),
            sort_order.as_deref(),
            scope.as_ref(),
        )
    })
    .await
}

#[tauri::command]
pub async fn db_count_entity_facets(
    app: tauri::AppHandle,
    kind: String,
    query: Option<String>,
    filters: Option<SearchV2Params>,
    scope: Option<EntityFacetScope>,
) -> Result<i64, String> {
    run_db_blocking(app, move |state| {
        state
            .db
            .count_entity_facets(&kind, query.as_deref(), filters.as_ref(), scope.as_ref())
    })
    .await
}

#[tauri::command]
pub async fn db_search_filter_facets(
    app: tauri::AppHandle,
    kind: String,
    query: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<FacetCount>, String> {
    run_db_blocking(app, move |state| {
        state
            .db
            .search_filter_facets(&kind, query.as_deref(), limit.unwrap_or(30))
    })
    .await
}

fn validate_path_in_storage(path: &str, storage_dir: &std::path::Path) -> Result<(), String> {
    let p = std::path::Path::new(path);
    // 物理絶対パスに解決（..の除去と正規化）
    let canon_p = p
        .canonicalize()
        .map_err(|e| format!("Invalid path or file not found: {}", e))?;
    let canon_storage = storage_dir
        .canonicalize()
        .map_err(|e| format!("Storage path resolution failed: {}", e))?;
    if !canon_p.starts_with(&canon_storage) {
        return Err("Access Denied: Path is outside of storage directory".to_string());
    }
    Ok(())
}

#[tauri::command]
pub async fn read_file_content(app: tauri::AppHandle, path: String) -> Result<String, String> {
    let state = app.state::<Arc<AppState>>();
    validate_path_in_storage(&path, state.db.storage_dir())?;

    tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| format!("Failed to read file: {}", e))
}

#[tauri::command]
pub async fn open_local_asset(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>();
    validate_path_in_storage(&path, state.db.storage_dir())?;

    let target = std::path::Path::new(&path);
    if !target.is_file() {
        return Err("File not found or path is not a file".to_string());
    }
    // 保存先の中身は、取得元と書庫から来る。**置いてある場所ではなく種類で断る。**
    if !super::shell::file_extension_is_openable(target) {
        return Err("この種類のファイルはアプリから開けません".to_string());
    }

    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| format!("Failed to open file: {}", e))
}

#[tauri::command]
pub async fn db_set_watch_updates(
    app: tauri::AppHandle,
    download_id: i64,
    watch: bool,
) -> Result<(), String> {
    run_library_write_blocking(app, move |state| {
        state.db.set_watch_updates(download_id, watch)
    })
    .await
}

#[tauri::command]
pub async fn db_set_favorite(
    app: tauri::AppHandle,
    download_id: i64,
    favorite: bool,
) -> Result<(), String> {
    run_library_write_blocking(app, move |state| {
        state.db.set_favorite(download_id, favorite)
    })
    .await
}

#[tauri::command]
pub async fn db_set_flags_for_ids(
    app: tauri::AppHandle,
    ids: Vec<i64>,
    favorite: Option<bool>,
    watch: Option<bool>,
) -> Result<BulkMutationResult, String> {
    run_library_write_blocking(app, move |state| {
        state.db.set_flags_for_ids(&ids, favorite, watch)
    })
    .await
}

#[tauri::command]
pub async fn db_upsert_update_target(
    app: tauri::AppHandle,
    target: UpdateTargetInput,
) -> Result<UpdateTarget, String> {
    run_library_write_blocking(app, move |state| state.db.upsert_update_target(&target)).await
}

#[tauri::command]
pub async fn db_get_update_target(
    app: tauri::AppHandle,
    target_type: String,
    source: String,
    source_key: String,
) -> Result<Option<UpdateTarget>, String> {
    run_db_blocking(app, move |state| {
        state
            .db
            .find_update_target(&target_type, &source, &source_key)
    })
    .await
}

#[tauri::command]
pub async fn db_list_update_targets(
    app: tauri::AppHandle,
    target_type: Option<String>,
    enabled_only: Option<bool>,
) -> Result<Vec<UpdateTarget>, String> {
    let state = app.state::<Arc<AppState>>();
    state
        .db
        .list_update_targets(target_type.as_deref(), enabled_only.unwrap_or(false))
}

#[tauri::command]
pub async fn db_set_update_target_enabled(
    app: tauri::AppHandle,
    target_type: String,
    source: String,
    source_key: String,
    enabled: bool,
) -> Result<(), String> {
    run_library_write_blocking(app, move |state| {
        state
            .db
            .set_update_target_enabled(&target_type, &source, &source_key, enabled)
    })
    .await
}

#[tauri::command]
pub async fn db_delete_update_target(
    app: tauri::AppHandle,
    target_type: String,
    source: String,
    source_key: String,
) -> Result<(), String> {
    run_library_write_blocking(app, move |state| {
        state
            .db
            .delete_update_target(&target_type, &source, &source_key)
    })
    .await
}

#[tauri::command]
pub async fn db_get_person(
    app: tauri::AppHandle,
    source: String,
    source_key: String,
) -> Result<PersonEntry, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.get_person(&source, &source_key)
}

#[tauri::command]
pub async fn db_get_series(
    app: tauri::AppHandle,
    source: String,
    source_key: String,
) -> Result<SeriesEntry, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.get_series(&source, &source_key)
}

#[tauri::command]
pub async fn db_list_entity_versions(
    app: tauri::AppHandle,
    entity_type: String,
    source: String,
    source_key: String,
) -> Result<Vec<EntityVersion>, String> {
    let state = app.state::<Arc<AppState>>();
    state
        .db
        .list_entity_versions(&entity_type, &source, &source_key)
}

#[tauri::command]
pub async fn db_get_latest_entity_profile_json(
    app: tauri::AppHandle,
    entity_type: String,
    source: String,
    source_key: String,
) -> Result<Option<serde_json::Value>, String> {
    let state = app.state::<Arc<AppState>>();
    let versions = state
        .db
        .list_entity_versions(&entity_type, &source, &source_key)?;
    let Some(version) = versions.first() else {
        return Ok(None);
    };
    if version.json_path.trim().is_empty() {
        return Ok(None);
    }

    let app_data = state
        .db
        .storage_dir()
        .parent()
        .unwrap_or_else(|| state.db.storage_dir())
        .to_path_buf();
    let json_path = std::path::Path::new(&version.json_path);
    let canon_json = json_path
        .canonicalize()
        .map_err(|e| format!("Entity profile path resolution failed: {}", e))?;
    let canon_app_data = app_data
        .canonicalize()
        .map_err(|e| format!("App data path resolution failed: {}", e))?;
    if !canon_json.starts_with(&canon_app_data) {
        return Err(
            "Access Denied: Entity profile path is outside of app data directory".to_string(),
        );
    }

    let raw = tokio::fs::read_to_string(&canon_json)
        .await
        .map_err(|e| format!("Failed to read entity profile JSON: {}", e))?;
    let value = serde_json::from_str(&raw)
        .map_err(|e| format!("Failed to parse entity profile JSON: {}", e))?;
    Ok(Some(value))
}

fn safe_entity_segment(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string()
}

fn hash_json(value: &serde_json::Value) -> Result<String, String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect())
}

fn json_string_at<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a str> {
    let mut cursor = value;
    for key in path {
        cursor = cursor.get(*key)?;
    }
    cursor.as_str().filter(|s| !s.trim().is_empty())
}

fn first_json_string_at<'a>(value: &'a serde_json::Value, paths: &[&[&str]]) -> Option<&'a str> {
    paths.iter().find_map(|path| json_string_at(value, path))
}

fn collect_profile_links(
    candidates: impl IntoIterator<Item = String>,
    text: Option<&str>,
) -> Vec<String> {
    static URL_PATTERN: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r#"https?://[^\s<>\"']+"#).expect("profile URL regex must compile")
    });
    let mut links = Vec::new();
    let mut push = |candidate: &str| {
        let trimmed = candidate
            .trim()
            .trim_end_matches(['.', ',', ')', ']', '}', '。', '、']);
        if url::Url::parse(trimmed).is_ok() && !links.iter().any(|link| link == trimmed) {
            links.push(trimmed.to_string());
        }
    };
    for candidate in candidates {
        push(&candidate);
    }
    if let Some(text) = text {
        for matched in URL_PATTERN.find_iter(text) {
            push(matched.as_str());
        }
    }
    links
}

/// シリーズの横顔を組み立てる。
///
/// 相手はひとつでも、口はふたつある。
///
/// - アプリAPI（`/v2/novel/series`）は各話の一覧を持っている。題はここの
///   各話が名乗るものがいちばん確かで、話数もここで数えられる。ただし
///   トークンが要る。
/// - web（`/ajax/novel/series/{id}`）はシリーズ自身の表紙・公開話数・完結の
///   有無を持っている。ログインは要らず、R-18 でも読める。
///
/// 片方が黙っても、もう片方まで諦めない。トークンが無いときでも、web だけで
/// 題・説明・表紙は埋まる。
async fn fetch_pixiv_series_profile_json(
    source_key: &str,
    refresh_token: Option<&str>,
    existing_title: &str,
    existing_description: Option<&str>,
) -> Result<Option<serde_json::Value>, String> {
    if source_key.trim().is_empty() {
        return Err("Invalid Pixiv series ID".to_string());
    }
    // web は認証が要らないので、先に聞く。失敗しても続ける。
    let web_series = match crate::pixiv_api::web::WebPixivAPI::new() {
        Ok(api) => match api.novel_series(source_key).await {
            Ok(series) => Some(series),
            Err(error) => {
                log::warn!("pixiv web series {source_key} unavailable: {error}");
                None
            }
        },
        Err(error) => {
            log::warn!("pixiv web client unavailable: {error}");
            None
        }
    };

    let value = match refresh_token
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        Some(token) => {
            let series_id: u64 = source_key.parse().map_err(|_| "Invalid Pixiv series ID")?;
            let api =
                crate::pixiv_api::aapi::AppPixivAPI::new_from_refresh_token(token.to_string())
                    .map_err(|error| error.to_string())?;
            match api.novel_series(series_id, None, None, true).await {
                Ok(value) => Some(value),
                // web から取れているなら、アプリAPIの失敗で全部を捨てない。
                Err(error) if web_series.is_some() => {
                    log::warn!("pixiv app-api series {source_key} unavailable: {error}");
                    None
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        None if web_series.is_some() => None,
        None => return Ok(None),
    };
    let value = value.unwrap_or_else(|| serde_json::json!({}));

    let novels = value
        .get("novels")
        .and_then(|novels| novels.as_array())
        .cloned()
        .unwrap_or_default();
    let title = novels
        .iter()
        .find_map(|novel| {
            first_json_string_at(
                novel,
                &[&["series", "title"], &["seriesTitle"], &["series_title"]],
            )
        })
        .map(str::to_string)
        .or_else(|| web_series.as_ref().and_then(|series| series.title.clone()))
        .unwrap_or_else(|| existing_title.to_string());
    // シリーズ自身の表紙を最優先する。1話目の表紙は、それが取れなかった
    // ときの間に合わせでしかない。
    let cover_url = web_series
        .as_ref()
        .and_then(|series| series.cover_url.as_deref())
        .or_else(|| {
            first_json_string_at(
                &value,
                &[
                    &["novel_series_detail", "cover_image_urls", "large"],
                    &["novelSeriesDetail", "coverImageUrls", "large"],
                    &["novel_series_detail", "cover_image_urls", "medium"],
                    &["novelSeriesDetail", "coverImageUrls", "medium"],
                    &["novel_series_detail", "cover_url"],
                    &["novelSeriesDetail", "coverUrl"],
                ],
            )
        })
        .or_else(|| {
            novels.iter().find_map(|novel| {
                first_json_string_at(
                    novel,
                    &[
                        &["image_urls", "large"],
                        &["imageUrls", "large"],
                        &["image_urls", "medium"],
                        &["imageUrls", "medium"],
                        &["coverUrl"],
                        &["cover_url"],
                    ],
                )
            })
        });
    let description = first_json_string_at(
        &value,
        &[
            &["novel_series_detail", "caption"],
            &["novelSeriesDetail", "caption"],
            &["series", "caption"],
            &["series", "description"],
        ],
    )
    .map(str::to_string)
    .or_else(|| {
        web_series
            .as_ref()
            .and_then(|series| series.caption.clone())
    })
    .or_else(|| existing_description.map(str::to_string));

    Ok(Some(serde_json::json!({
        "source": "pixiv",
        "sourceKey": source_key,
        "title": title,
        "description": description,
        "coverUrl": cover_url,
        "sampleNovelCount": novels.len(),
        // web だけが知っていること。聞けなかったときは null のままにして、
        // 手元の値を上書きしない。
        "isConcluded": web_series.as_ref().and_then(|series| series.is_concluded),
        "publishedContentCount": web_series
            .as_ref()
            .and_then(|series| series.published_content_count),
    })))
}

async fn save_entity_json(
    app_data: &std::path::Path,
    entity_type: &str,
    source: &str,
    source_key: &str,
    version: i64,
    value: &serde_json::Value,
) -> Result<(String, i64), String> {
    let root = if entity_type == "series" {
        "series"
    } else {
        "profiles"
    };
    let dir = app_data
        .join(root)
        .join(safe_entity_segment(source))
        .join(safe_entity_segment(source_key))
        .join(format!("v{}", version));
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    let path = dir.join("original.json");
    let content = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let len = content.len() as i64;
    tokio::fs::write(&path, content)
        .await
        .map_err(|e| e.to_string())?;
    Ok((path.to_string_lossy().to_string(), len))
}

async fn download_entity_image(
    app_data: &std::path::Path,
    entity_type: &str,
    source: &str,
    source_key: &str,
    version: i64,
    kind: &str,
    url: Option<&str>,
) -> Result<Option<(String, i64)>, String> {
    let Some(url) = url else { return Ok(None) };
    let url = url.trim();
    if url.is_empty() {
        return Ok(None);
    }
    // 取得先を https に限る。
    //
    // この一本だけ、取得元の応答に入っていた URL をそのまま `reqwest` へ
    // 渡していた。ほかの外向きの要求は例外なく宛先を絞ってある
    // （`allowed_next_url` / `allowed_api_url`）。ここは資格情報を付けないので
    // 漏れる情報は無いが、`file:` や `http:` を踏みに行く筋を残す理由も無い。
    //
    // ホスト名までは固定しない。表紙とアイコンは取得元の CDN が配るもので、
    // その名前は向こうの都合で変わる。固定すると、変わった日に**画像が黙って
    // 出なくなる**。
    //
    // 綴りの大小は問わない。scheme は RFC 3986 で大小を区別しないので、
    // 厳密に小文字だけを通すと、`HTTPS://` を返してきた日に**画像が黙って
    // 出なくなる**。守りたいのは scheme の種類であって、綴りではない。
    if !url
        .get(..8)
        .is_some_and(|head| head.eq_ignore_ascii_case("https://"))
    {
        log::warn!("Entity image skipped, not an https URL: {kind}");
        return Ok(None);
    }
    let root = if entity_type == "series" {
        "series"
    } else {
        "profiles"
    };
    let dir = app_data
        .join(root)
        .join(safe_entity_segment(source))
        .join(safe_entity_segment(source_key))
        .join(format!("v{}", version))
        .join("assets");
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| e.to_string())?;
    let filename = url
        .split('/')
        .next_back()
        .and_then(|s| s.split('?').next())
        .filter(|s| !s.is_empty())
        .unwrap_or(kind);
    let path = dir.join(format!("{}_{}", kind, safe_entity_segment(filename)));
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|e| format!("Profile image client creation failed: {e}"))?;
    let response = client
        .get(url)
        .header(
            "referer",
            if source == "fanbox" {
                "https://www.fanbox.cc/"
            } else {
                "https://www.pixiv.net/"
            },
        )
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    let len = crate::downloader::asset_downloader::save_response_atomically(
        response,
        &path,
        crate::downloader::asset_downloader::MAX_PROFILE_IMAGE_BYTES,
        true,
    )
    .await? as i64;
    Ok(Some((path.to_string_lossy().to_string(), len)))
}

async fn copy_file_atomically_bounded(
    source: &std::path::Path,
    destination: &std::path::Path,
    max_bytes: u64,
) -> Result<u64, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let parent = destination
        .parent()
        .ok_or_else(|| "Import destination has no parent".to_string())?;
    let filename = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("asset");
    let (part_path, mut output) = {
        let mut opened = None;
        for _ in 0..16 {
            let path = parent.join(format!(".{filename}.{:016x}.part", rand::random::<u64>()));
            match tokio::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
                .await
            {
                Ok(file) => {
                    opened = Some((path, file));
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("Import staging creation failed: {error}")),
            }
        }
        opened.ok_or_else(|| "Could not allocate an import staging file".to_string())?
    };

    let result = async {
        let input = tokio::fs::File::open(source)
            .await
            .map_err(|error| format!("Import source open failed: {error}"))?;
        let mut limited = input.take(max_bytes.saturating_add(1));
        let copied = tokio::io::copy(&mut limited, &mut output)
            .await
            .map_err(|error| format!("Import copy failed: {error}"))?;
        if copied > max_bytes {
            return Err(format!(
                "Import source exceeded the {max_bytes} byte safety limit"
            ));
        }
        output
            .flush()
            .await
            .map_err(|error| format!("Import flush failed: {error}"))?;
        output
            .sync_all()
            .await
            .map_err(|error| format!("Import sync failed: {error}"))?;
        drop(output);
        tokio::fs::rename(&part_path, destination)
            .await
            .map_err(|error| format!("Import publish failed: {error}"))?;
        Ok(copied)
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&part_path).await;
    }
    result
}

#[tauri::command]
pub async fn refresh_entity_profile(
    app: tauri::AppHandle,
    params: RefreshEntityProfileParams,
) -> Result<serde_json::Value, String> {
    let RefreshEntityProfileParams {
        entity_type,
        source,
        source_key,
        force,
        refresh_token,
        cookie,
        user_agent,
    } = params;

    let mut last_refresh = ENTITY_REFRESH_LOCK.lock().await;
    if let Some(last) = *last_refresh {
        let elapsed = last.elapsed();
        if elapsed < Duration::from_secs(3) {
            sleep(Duration::from_secs(3) - elapsed).await;
        }
    }
    *last_refresh = Some(Instant::now());

    let state = app.state::<Arc<AppState>>().inner().clone();
    let force = force.unwrap_or(false);
    let app_data = state
        .db
        .storage_dir()
        .parent()
        .unwrap_or_else(|| state.db.storage_dir())
        .to_path_buf();

    if entity_type == "person" {
        if !force {
            if let Ok(person) = state.db.get_person(&source, &source_key) {
                if recently_checked(person.last_checked_at.as_deref()) {
                    return serde_json::to_value(person).map_err(|e| e.to_string());
                }
            }
        }

        let normalized = if source == "pixiv" {
            let token = refresh_token.ok_or("Pixivプロフィール更新にはrefreshTokenが必要です")?;
            let user_id: u64 = source_key.parse().map_err(|_| "Invalid Pixiv user ID")?;
            let api = crate::pixiv_api::aapi::AppPixivAPI::new_from_refresh_token(token)
                .map_err(|error| error.to_string())?;
            let detail = api
                .user_detail(user_id, None, true)
                .await
                .map_err(|e| e.to_string())?;
            let links = collect_profile_links(
                [
                    format!("https://www.pixiv.net/users/{}", detail.user.id),
                    detail.profile.webpage.clone().unwrap_or_default(),
                    detail.profile.twitter_url.clone().unwrap_or_default(),
                    detail
                        .profile
                        .twitter_account
                        .as_ref()
                        .map(|account| format!("https://x.com/{}", account.trim_start_matches('@')))
                        .unwrap_or_default(),
                    detail.profile.pawoo_url.clone().unwrap_or_default(),
                ],
                detail.user.comment.as_deref(),
            );
            serde_json::json!({
                "source": source,
                "sourceKey": source_key,
                "displayName": detail.user.name,
                "account": detail.user.account,
                "iconUrl": detail.user.profile_image_urls.medium,
                "coverUrl": detail.profile.background_image_url,
                "description": detail.user.comment,
                "links": links,
                "stats": {
                    "totalIllusts": detail.profile.total_illusts.unwrap_or(0),
                    "totalManga": detail.profile.total_manga.unwrap_or(0),
                    "totalNovels": detail.profile.total_novels.unwrap_or(0),
                    "totalNovelSeries": detail.profile.total_novel_series.unwrap_or(0)
                },
            })
        } else if source == "fanbox" {
            let cookie = cookie.ok_or("FANBOXプロフィール更新にはcookieが必要です")?;
            let user_agent = user_agent.unwrap_or_else(|| "Mozilla/5.0".to_string());
            let api = crate::fanbox_api::client::FanboxAPI::new(cookie, user_agent)
                .map_err(|error| error.to_string())?;
            let detail = api
                .get_creator(&source_key)
                .await
                .map_err(|e| e.to_string())?;
            let links = collect_profile_links(
                std::iter::once(format!("https://www.fanbox.cc/@{}", source_key))
                    .chain(detail.profile_links.iter().cloned()),
                Some(&detail.description),
            );
            serde_json::json!({
                "source": source,
                "sourceKey": source_key,
                "displayName": detail.user.name,
                "iconUrl": detail.user.icon_url,
                "coverUrl": detail.cover_image_url,
                "description": detail.description,
                "links": links,
                "profileItems": detail.profile_items,
            })
        } else {
            return Err("Unsupported person source".to_string());
        };

        let _library_write_guard = state.library_gate.clone().write_owned().await;
        let hash = hash_json(&normalized)?;
        let existing = state.db.get_person(&source, &source_key).ok();
        let changed =
            existing.as_ref().and_then(|p| p.content_hash.as_deref()) != Some(hash.as_str());
        let next_version = existing
            .as_ref()
            .map(|p| p.current_version + 1)
            .unwrap_or(1);
        let (json_path, json_size, icon_path, cover_path, asset_size, asset_count) = if changed {
            let (json_path, json_size) = save_entity_json(
                &app_data,
                "person",
                &source,
                &source_key,
                next_version,
                &normalized,
            )
            .await?;
            let icon = download_entity_image(
                &app_data,
                "person",
                &source,
                &source_key,
                next_version,
                "icon",
                normalized.get("iconUrl").and_then(|v| v.as_str()),
            )
            .await?;
            let cover = download_entity_image(
                &app_data,
                "person",
                &source,
                &source_key,
                next_version,
                "cover",
                normalized.get("coverUrl").and_then(|v| v.as_str()),
            )
            .await?;
            let asset_size = icon.as_ref().map(|(_, n)| *n).unwrap_or(0)
                + cover.as_ref().map(|(_, n)| *n).unwrap_or(0);
            let asset_count =
                (if icon.is_some() { 1 } else { 0 }) + (if cover.is_some() { 1 } else { 0 });
            (
                json_path,
                json_size,
                icon.map(|(p, _)| p),
                cover.map(|(p, _)| p),
                asset_size,
                asset_count,
            )
        } else {
            (
                String::new(),
                0,
                existing.as_ref().and_then(|p| p.icon_path.clone()),
                existing.as_ref().and_then(|p| p.cover_path.clone()),
                0,
                0,
            )
        };
        let links_json = serde_json::to_string(
            normalized
                .get("links")
                .unwrap_or(&serde_json::Value::Array(vec![])),
        )
        .ok();
        let person = state.db.upsert_person_profile(
            &source,
            &source_key,
            normalized
                .get("displayName")
                .and_then(|v| v.as_str())
                .unwrap_or(&source_key),
            icon_path.as_deref(),
            cover_path.as_deref(),
            normalized.get("description").and_then(|v| v.as_str()),
            links_json.as_deref(),
            &hash,
            &json_path,
            asset_count,
            json_size + asset_size,
            EntityProfileFreshness::RemoteChecked,
        )?;
        return serde_json::to_value(person).map_err(|e| e.to_string());
    }

    if entity_type == "series" {
        let mut existing = state.db.get_series(&source, &source_key).ok();
        if !force {
            if let Some(series) = existing.as_ref() {
                if recently_checked(series.last_checked_at.as_deref()) {
                    return serde_json::to_value(series).map_err(|e| e.to_string());
                }
            }
        }
        let title = existing
            .as_ref()
            .map(|s| s.title.clone())
            .unwrap_or_else(|| source_key.clone());
        let description = existing.as_ref().and_then(|s| s.description.clone());
        let normalized = if source == "pixiv" {
            match fetch_pixiv_series_profile_json(
                &source_key,
                refresh_token.as_deref(),
                &title,
                description.as_deref(),
            )
            .await
            {
                Ok(Some(value)) => value,
                // `None` means neither the web endpoint nor the app API yielded
                // series data. A save-time snapshot is still useful, but it is
                // not a completed remote refresh and must stay retryable.
                Ok(None) => {
                    return Err(format!(
                        "Pixivシリーズ情報を取得できませんでした ({source_key})"
                    ))
                }
                Err(error) => return Err(error),
            }
        } else {
            serde_json::json!({
                "source": source,
                "sourceKey": source_key,
                "title": title,
                "description": description,
                "coverUrl": null,
            })
        };
        let _library_write_guard = state.library_gate.clone().write_owned().await;
        existing = state.db.get_series(&source, &source_key).ok();
        let hash = hash_json(&normalized)?;
        let changed =
            existing.as_ref().and_then(|s| s.content_hash.as_deref()) != Some(hash.as_str());
        let next_version = existing
            .as_ref()
            .map(|s| s.current_version + 1)
            .unwrap_or(1);
        let (json_path, json_size) = if changed {
            save_entity_json(
                &app_data,
                "series",
                &source,
                &source_key,
                next_version,
                &normalized,
            )
            .await?
        } else {
            (String::new(), 0)
        };
        let current_version = existing
            .as_ref()
            .map(|s| s.current_version)
            .unwrap_or(1)
            .max(1);
        let asset_version = if changed {
            next_version
        } else {
            current_version
        };
        let cover_url = normalized.get("coverUrl").and_then(|v| v.as_str());
        let should_download_cover = cover_url.filter(|url| !url.trim().is_empty()).is_some()
            && (changed
                || existing
                    .as_ref()
                    .and_then(|s| s.cover_path.as_ref())
                    .is_none());
        let cover = if should_download_cover {
            match download_entity_image(
                &app_data,
                "series",
                &source,
                &source_key,
                asset_version,
                "cover",
                cover_url,
            )
            .await
            {
                Ok(value) => value,
                Err(error) => {
                    log::warn!("Failed to download series cover {}: {}", source_key, error);
                    None
                }
            }
        } else {
            None
        };
        let cover_path = cover
            .as_ref()
            .map(|(path, _)| path.clone())
            .or_else(|| existing.as_ref().and_then(|s| s.cover_path.clone()));
        let cover_size = cover.as_ref().map(|(_, size)| *size).unwrap_or(0);
        let series = state.db.upsert_series_profile(
            &source,
            &source_key,
            normalized
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or(&source_key),
            normalized.get("description").and_then(|v| v.as_str()),
            cover_path.as_deref(),
            &hash,
            &json_path,
            if cover.is_some() { 1 } else { 0 },
            json_size + cover_size,
            EntityProfileFreshness::RemoteChecked,
            normalized.get("isConcluded").and_then(|v| v.as_bool()),
            normalized
                .get("publishedContentCount")
                .and_then(|v| v.as_i64()),
        )?;
        return serde_json::to_value(series).map_err(|e| e.to_string());
    }

    Err("Unsupported entity type".to_string())
}

#[tauri::command]
pub async fn db_get_entity_profile_repair_status(
    app: tauri::AppHandle,
) -> Result<EntityProfileRepairStatus, String> {
    run_db_blocking(app, |state| state.db.entity_profile_repair_status()).await
}

/// 完全取得できていない作者・シリーズを、成功したものから順に埋め直す。
///
/// 再開位置は一時的なメモリではなく各エンティティの `last_fetched_at`。
/// アプリ終了やキャンセル後も、次回は残件だけが対象になる。
#[tauri::command]
pub async fn repair_incomplete_entity_profiles(
    app: tauri::AppHandle,
    credentials: UpdateCredentials,
) -> Result<EntityProfileRepairResult, String> {
    ENTITY_PROFILE_REPAIR_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "作者・シリーズ情報の修復はすでに実行中です".to_string())?;
    let _run_guard = EntityProfileRepairRunGuard;
    ENTITY_PROFILE_REPAIR_CANCEL.store(false, Ordering::Release);

    let targets =
        run_db_blocking(app.clone(), |state| state.db.incomplete_entity_profiles()).await?;
    let total = targets.len();
    let mut repaired = 0usize;
    let mut failed = 0usize;
    let mut completed = 0usize;

    let _ = app.emit(
        "entity-profile-repair-progress",
        EntityProfileRepairProgress {
            phase: "running".to_string(),
            completed,
            total,
            repaired,
            failed,
            active_label: None,
            error: None,
        },
    );

    for target in targets {
        if ENTITY_PROFILE_REPAIR_CANCEL.load(Ordering::Acquire) {
            break;
        }
        let params = RefreshEntityProfileParams {
            entity_type: target.entity_type.clone(),
            source: target.source.clone(),
            source_key: target.source_key.clone(),
            force: Some(false),
            refresh_token: credentials.pixiv_refresh_token.clone(),
            cookie: credentials.fanbox_cookie.clone(),
            user_agent: credentials.fanbox_user_agent.clone(),
        };
        let result = refresh_entity_profile(app.clone(), params).await;
        completed += 1;
        let error = match result {
            Ok(_) => {
                repaired += 1;
                None
            }
            Err(error) => {
                failed += 1;
                log::warn!(
                    "{}:{}:{} のプロフィール修復に失敗しました: {}",
                    target.entity_type,
                    target.source,
                    target.source_key,
                    error
                );
                Some(error)
            }
        };
        let _ = app.emit(
            "entity-profile-repair-progress",
            EntityProfileRepairProgress {
                phase: "running".to_string(),
                completed,
                total,
                repaired,
                failed,
                active_label: Some(target.display_name),
                error,
            },
        );
    }

    let canceled = ENTITY_PROFILE_REPAIR_CANCEL.load(Ordering::Acquire);
    let remaining = run_db_blocking(app.clone(), |state| state.db.entity_profile_repair_status())
        .await?
        .total_count;
    let _ = app.emit(
        "entity-profile-repair-progress",
        EntityProfileRepairProgress {
            phase: if canceled { "canceled" } else { "complete" }.to_string(),
            completed,
            total,
            repaired,
            failed,
            active_label: None,
            error: None,
        },
    );
    Ok(EntityProfileRepairResult {
        attempted: completed,
        repaired,
        failed,
        canceled,
        remaining,
    })
}

#[tauri::command]
pub fn cancel_entity_profile_repair() -> bool {
    if !ENTITY_PROFILE_REPAIR_RUNNING.load(Ordering::Acquire) {
        return false;
    }
    ENTITY_PROFILE_REPAIR_CANCEL.store(true, Ordering::Release);
    true
}

fn recently_checked(value: Option<&str>) -> bool {
    let Some(value) = value else { return false };
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(value) else {
        return false;
    };
    chrono::Utc::now().signed_duration_since(dt.with_timezone(&chrono::Utc))
        < chrono::Duration::hours(24)
}

#[tauri::command]
pub async fn db_get_download_by_source(
    app: tauri::AppHandle,
    source: String,
    source_id: String,
) -> Result<Option<DownloadEntry>, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.get_download_by_source(&source, &source_id)
}

#[tauri::command]
pub async fn db_get_reader_metadata(
    app: tauri::AppHandle,
    download_id: i64,
) -> Result<ReaderMetadata, String> {
    run_db_blocking(app, move |state| state.db.get_reader_metadata(download_id)).await
}

#[tauri::command]
pub async fn db_get_reader_content_page(
    app: tauri::AppHandle,
    download_id: i64,
    version: Option<i64>,
    page: usize,
) -> Result<ReaderContentPage, String> {
    run_db_blocking(app, move |state| {
        state.db.get_reader_content_page(download_id, version, page)
    })
    .await
}

#[tauri::command]
pub async fn db_search_reader_content(
    app: tauri::AppHandle,
    download_id: i64,
    version: Option<i64>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<ReaderSearchHit>, String> {
    run_db_blocking(app, move |state| {
        state
            .db
            .search_reader_content(download_id, version, &query, limit.unwrap_or(50))
    })
    .await
}

#[tauri::command]
pub async fn db_get_editor_document(
    app: tauri::AppHandle,
    download_id: i64,
) -> Result<EditorDocument, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.get_editor_document(download_id)
}

#[tauri::command]
pub async fn db_save_work_draft(
    app: tauri::AppHandle,
    download_id: i64,
    base_version: i64,
    blocks: Vec<WorkBlockInput>,
) -> Result<WorkEditRevision, String> {
    run_library_write_blocking(app, move |state| {
        state
            .db
            .save_work_draft(download_id, base_version, blocks.as_slice())
    })
    .await
}

#[tauri::command]
pub async fn db_activate_work_edit(
    app: tauri::AppHandle,
    edit_revision_id: i64,
) -> Result<WorkEditRevision, String> {
    run_library_write_blocking(app, move |state| {
        state.db.activate_work_edit(edit_revision_id)
    })
    .await
}

#[tauri::command]
pub async fn import_work_asset(
    app: tauri::AppHandle,
    download_id: i64,
    source_path: String,
) -> Result<AssetEntry, String> {
    let source = std::path::PathBuf::from(&source_path);
    let source_metadata = tokio::fs::symlink_metadata(&source)
        .await
        .map_err(|error| format!("Failed to inspect import source: {error}"))?;
    if source_metadata.file_type().is_symlink() || !source_metadata.file_type().is_file() {
        return Err("Import source is not a file".to_string());
    }
    if source_metadata.len() > crate::downloader::asset_downloader::MAX_ASSET_DOWNLOAD_BYTES {
        return Err(format!(
            "Import source exceeds the {} byte safety limit",
            crate::downloader::asset_downloader::MAX_ASSET_DOWNLOAD_BYTES
        ));
    }

    let state = app.state::<Arc<AppState>>().inner().clone();
    let _library_write_guard = state.library_gate.clone().write_owned().await;

    state.db.get_download(download_id)?;

    let original_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("asset");
    let filename = crate::downloader::asset_downloader::sanitize_filename(original_name);
    let target_dir = state
        .db
        .storage_dir()
        .join("editor-assets")
        .join(download_id.to_string());
    tokio::fs::create_dir_all(&target_dir)
        .await
        .map_err(|e| format!("Failed to create editor asset directory: {}", e))?;
    let canonical_storage = tokio::fs::canonicalize(state.db.storage_dir())
        .await
        .map_err(|error| format!("Failed to resolve library storage: {error}"))?;
    let canonical_target_dir = tokio::fs::canonicalize(&target_dir)
        .await
        .map_err(|error| format!("Failed to resolve editor asset directory: {error}"))?;
    if !canonical_target_dir.starts_with(&canonical_storage) {
        return Err("Editor asset directory escapes library storage".to_string());
    }

    let mut target_path = target_dir.join(&filename);
    if target_path.exists() {
        let stem = target_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("asset");
        let ext = target_path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| format!(".{}", value))
            .unwrap_or_default();
        target_path = target_dir.join(format!(
            "{}-{}{}",
            stem,
            chrono::Utc::now().timestamp_millis(),
            ext
        ));
    }

    let copied = copy_file_atomically_bounded(
        &source,
        &target_path,
        crate::downloader::asset_downloader::MAX_ASSET_DOWNLOAD_BYTES,
    )
    .await?;
    let mime_type = target_path
        .extension()
        .and_then(|value| value.to_str())
        .map(|ext| match ext.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => "image/jpeg",
            "png" => "image/png",
            "webp" => "image/webp",
            "gif" => "image/gif",
            _ => "application/octet-stream",
        })
        .map(str::to_string);

    let local_path = target_path.to_string_lossy().to_string();
    let asset = NewAsset {
        download_id,
        asset_type: "editor_image".to_string(),
        filename: target_path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(&filename)
            .to_string(),
        local_path: local_path.clone(),
        original_url: None,
        mime_type: mime_type.clone(),
        file_size_bytes: copied as i64,
    };
    let id = match state.db.insert_asset(&asset) {
        Ok(id) => id,
        Err(error) => {
            if let Err(cleanup_error) = tokio::fs::remove_file(&target_path).await {
                log::warn!(
                    "Failed to roll back imported asset {:?}: {}",
                    target_path,
                    cleanup_error
                );
            }
            return Err(error);
        }
    };

    Ok(AssetEntry {
        id,
        download_id,
        asset_type: asset.asset_type,
        filename: asset.filename,
        local_path,
        original_url: None,
        mime_type,
        file_size_bytes: asset.file_size_bytes,
    })
}

#[cfg(test)]
mod profile_link_tests {
    use super::{
        collect_profile_links, copy_file_atomically_bounded, import_collection_cover,
        validate_path_in_storage, MAX_COLLECTION_COVER_BYTES,
    };
    use std::fs;

    #[test]
    fn profile_links_are_deduplicated_and_extracted_from_text() {
        let links = collect_profile_links(
            [
                "https://www.pixiv.net/users/42".to_string(),
                "https://x.com/example".to_string(),
                "https://x.com/example".to_string(),
            ],
            Some("Skeb: https://skeb.jp/@example。 portfolio https://example.com/work"),
        );
        assert_eq!(
            links,
            vec![
                "https://www.pixiv.net/users/42",
                "https://x.com/example",
                "https://skeb.jp/@example",
                "https://example.com/work",
            ]
        );
    }

    #[test]
    fn collection_cover_is_validated_and_copied_into_app_data() {
        let root = std::env::temp_dir().join(format!(
            "piep_collection_cover_test_{}",
            rand::random::<u64>()
        ));
        let app_data = root.join("app-data");
        fs::create_dir_all(&root).unwrap();
        let source = root.join("chosen.png");
        image::RgbImage::from_pixel(2, 2, image::Rgb([20, 40, 60]))
            .save(&source)
            .unwrap();

        let imported = import_collection_cover(&app_data, &source).unwrap();
        assert!(imported.starts_with(app_data.join("collection-covers")));
        assert_eq!(fs::read(&imported).unwrap(), fs::read(&source).unwrap());
        assert_eq!(
            import_collection_cover(&app_data, &source).unwrap(),
            imported
        );

        let invalid = root.join("not-an-image.png");
        fs::write(&invalid, b"not an image").unwrap();
        assert!(import_collection_cover(&app_data, &invalid).is_err());

        let oversized = root.join("oversized.png");
        let file = fs::File::create(&oversized).unwrap();
        file.set_len(MAX_COLLECTION_COVER_BYTES + 1).unwrap();
        assert!(import_collection_cover(&app_data, &oversized).is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn bounded_atomic_import_never_publishes_oversize_or_partial_files() {
        let root =
            std::env::temp_dir().join(format!("piep_editor_asset_test_{}", rand::random::<u64>()));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source.bin");
        let destination = root.join("destination.bin");
        fs::write(&source, b"12345").unwrap();

        let error = copy_file_atomically_bounded(&source, &destination, 4)
            .await
            .unwrap_err();
        assert!(error.contains("4 byte"));
        assert!(!destination.exists());
        assert!(fs::read_dir(&root).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".part")));

        assert_eq!(
            copy_file_atomically_bounded(&source, &destination, 5)
                .await
                .unwrap(),
            5
        );
        assert_eq!(fs::read(&destination).unwrap(), b"12345");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn file_commands_cannot_read_sibling_app_data_files() {
        let root = std::env::temp_dir().join(format!("piep_path_scope_{}", rand::random::<u64>()));
        let storage = root.join("downloads");
        let allowed = storage.join("pixiv").join("work.json");
        let sibling = root.join("piep.db");
        fs::create_dir_all(allowed.parent().unwrap()).unwrap();
        fs::write(&allowed, "{}").unwrap();
        fs::write(&sibling, "secret").unwrap();

        assert!(validate_path_in_storage(allowed.to_str().unwrap(), &storage).is_ok());
        assert!(validate_path_in_storage(sibling.to_str().unwrap(), &storage).is_err());

        let _ = fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod entity_image_url_tests {
    /// 表紙とアイコンの取得先を https に限る。
    ///
    /// この一本だけ、取得元の応答に入っていた URL をそのまま渡していた。
    /// ほかの外向きの要求は例外なく宛先を絞ってある。
    #[test]
    fn only_https_urls_are_fetched() {
        let allowed = |url: &str| {
            let url = url.trim();
            url.get(..8)
                .is_some_and(|head| head.eq_ignore_ascii_case("https://"))
        };
        assert!(allowed("https://i.pximg.net/user-profile/x.jpg"));
        assert!(allowed("  https://downloads.fanbox.cc/cover.png  "));

        assert!(!allowed("http://i.pximg.net/x.jpg"));
        assert!(!allowed("file:///C:/Windows/win.ini"));
        assert!(!allowed("data:image/png;base64,AAAA"));
        assert!(!allowed("//i.pximg.net/x.jpg"));
        assert!(!allowed(""));
        // scheme の綴りの大小は問わない。RFC 3986 で区別しないものを区別すると、
        // 取得元が大文字で返した日に画像が黙って出なくなる。
        assert!(allowed("HTTPS://i.pximg.net/x.jpg"));
        // 短すぎて scheme が入りきらない文字列で落ちない。
        assert!(!allowed("http"));
    }
}
