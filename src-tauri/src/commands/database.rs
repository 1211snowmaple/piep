use crate::database::queries::EntityProfileFreshness;
use crate::database::{
    AssetEntry, BulkMutationResult, DashboardSummary, DbStats, DownloadEntry, DownloadRelation,
    DownloadVersion, EditorDocument, EntityVersion, FacetCount, FilterFacets, NewAsset,
    NewDownload, PersonEntry, ReaderDocument, SearchIndexStatus, SearchSuggestParams,
    SearchSuggestResult, SearchV2Params, SearchV2Result, SeriesEntry, UpdateTarget,
    UpdateTargetInput, WorkBlockInput, WorkEditRevision,
};
use crate::AppState;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
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
static SEARCH_REBUILD_JOBS: LazyLock<std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>> =
    LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));
static SEARCH_REBUILD_SEQUENCE: AtomicU64 = AtomicU64::new(1);

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

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshEntityProfileParams {
    entity_type: String,
    source: String,
    source_key: String,
    force: Option<bool>,
    refresh_token: Option<String>,
    cookie: Option<String>,
    user_agent: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRebuildJobOptions {
    batch_size: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRebuildProgress {
    job_id: String,
    status: String,
    total_downloads: i64,
    indexed_downloads: i64,
    pending_downloads: i64,
    is_complete: bool,
    phase: String,
    indexed_chunks: i64,
    embedding_provider: String,
    gpu_enabled: bool,
    throughput_per_sec: Option<f64>,
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
    let job_id = format!(
        "search-index-{}-{}",
        chrono::Utc::now().timestamp_millis(),
        SEARCH_REBUILD_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let cancel = Arc::new(AtomicBool::new(false));
    SEARCH_REBUILD_JOBS
        .lock()
        .map_err(|e| e.to_string())?
        .insert(job_id.clone(), cancel.clone());

    let batch_size = job_options
        .and_then(|options| options.batch_size)
        .unwrap_or(64)
        .clamp(1, 200);
    let app_for_task = app.clone();
    let job_id_for_task = job_id.clone();

    tauri::async_runtime::spawn(async move {
        let started_at = Instant::now();
        let mut initial_indexed: Option<i64> = None;
        loop {
            if cancel.load(Ordering::Relaxed) {
                if let Ok(state) = run_db_blocking(app_for_task.clone(), |state| {
                    state.db.get_search_index_status()
                })
                .await
                {
                    emit_search_index_progress(&app_for_task, &job_id_for_task, "canceled", state);
                }
                break;
            }

            let status = run_db_blocking(app_for_task.clone(), move |state| {
                state.db.rebuild_search_index_batch(batch_size)
            })
            .await;
            match status {
                Ok(mut status) => {
                    let baseline = *initial_indexed.get_or_insert(status.indexed_downloads);
                    let elapsed = started_at.elapsed().as_secs_f64();
                    if elapsed > 0.0 {
                        status.throughput_per_sec =
                            Some(((status.indexed_downloads - baseline).max(0) as f64) / elapsed);
                    }
                    let state_label = if status.pending_downloads == 0 {
                        "completed"
                    } else {
                        "running"
                    };
                    emit_search_index_progress(
                        &app_for_task,
                        &job_id_for_task,
                        state_label,
                        status.clone(),
                    );
                    if status.pending_downloads == 0 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => {
                    let _ = app_for_task.emit(
                        "search-index-progress",
                        serde_json::json!({
                            "jobId": job_id_for_task,
                            "status": "failed",
                            "error": error,
                        }),
                    );
                    break;
                }
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

fn emit_search_index_progress(
    app: &tauri::AppHandle,
    job_id: &str,
    status: &str,
    index: SearchIndexStatus,
) {
    let _ = app.emit(
        "search-index-progress",
        SearchRebuildProgress {
            job_id: job_id.to_string(),
            status: status.to_string(),
            total_downloads: index.total_downloads,
            indexed_downloads: index.indexed_downloads,
            pending_downloads: index.pending_downloads,
            is_complete: index.is_complete,
            phase: index.phase,
            indexed_chunks: index.indexed_chunks,
            embedding_provider: index.embedding_provider,
            gpu_enabled: index.gpu_enabled,
            throughput_per_sec: index.throughput_per_sec,
        },
    );
}

#[tauri::command]
pub async fn db_get_download(app: tauri::AppHandle, id: i64) -> Result<DownloadEntry, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.get_download(id)
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
    let state = app.state::<Arc<AppState>>();
    state.db.delete_download(id)
}

#[tauri::command]
pub async fn db_delete_downloads(
    app: tauri::AppHandle,
    ids: Vec<i64>,
) -> Result<BulkMutationResult, String> {
    run_db_blocking(app, move |state| state.db.delete_downloads(&ids)).await
}

#[tauri::command]
pub async fn db_delete_downloads_for_search(
    app: tauri::AppHandle,
    params: SearchV2Params,
) -> Result<BulkMutationResult, String> {
    run_db_blocking(app, move |state| {
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
pub async fn db_get_dashboard_summary(app: tauri::AppHandle) -> Result<DashboardSummary, String> {
    run_db_blocking(app, move |state| state.db.get_dashboard_summary()).await
}

#[tauri::command]
pub async fn db_seed_test_data(app: tauri::AppHandle, count: i64) -> Result<i64, String> {
    let count = count.clamp(1, 200_000);
    run_db_blocking(app, move |state| {
        let root = state.db.storage_dir().join("seed").join(format!("{}", chrono::Utc::now().timestamp_millis()));
        std::fs::create_dir_all(&root).map_err(|e| format!("Seed dir creation failed: {}", e))?;
        for index in 0..count {
            let source = if index % 3 == 0 { "fanbox" } else { "pixiv" };
            let source_id = format!("seed-{}", index + 1);
            let dir = root.join(source).join(&source_id).join("v1");
            std::fs::create_dir_all(&dir).map_err(|e| format!("Seed item dir creation failed: {}", e))?;
            let json_path = dir.join("original.json");
            let body = format!(
                "検索性能検証用の本文です。番号 {}、タグ seed{}、作者 {}。日本語とASCII mixed text for search.",
                index + 1,
                index % 25,
                index % 100,
            );
            std::fs::write(&json_path, serde_json::json!({ "text": body }).to_string())
                .map_err(|e| format!("Seed json write failed: {}", e))?;
            let tags = vec![
                format!("seed{}", index % 25),
                format!("group{}", index % 10),
            ];
            let created = format!("2026-{:02}-{:02}T00:00:00Z", (index % 12) + 1, (index % 27) + 1);
            let download = NewDownload {
                source: source.to_string(),
                source_id: source_id.clone(),
                title: format!("Seed Work {:06}", index + 1),
                author_name: format!("Seed Author {:03}", index % 100),
                author_id: format!("seed-author-{}", index % 100),
                content_type: if source == "pixiv" { "novel" } else { "article" }.to_string(),
                tags,
                excerpt: Some(format!("seed excerpt {}", index + 1)),
                cover_path: None,
                json_path: json_path.to_string_lossy().to_string(),
                original_json_path: Some(json_path.to_string_lossy().to_string()),
                asset_count: 0,
                file_size_bytes: 0,
                downloaded_at: chrono::Utc::now().to_rfc3339(),
                source_created_at: Some(created),
                content_hash: Some(format!("seed-hash-{}", index + 1)),
                text_length: body.chars().count() as i64,
                source_updated_at: None,
                watch_updates: index % 9 == 0,
                current_version: 1,
                favorite: index % 17 == 0,
            };
            state.db.upsert_download(&download)?;
        }
        Ok(count)
    })
    .await
}

#[tauri::command]
pub async fn db_get_filter_facets(app: tauri::AppHandle) -> Result<FilterFacets, String> {
    run_db_blocking(app, move |state| state.db.get_filter_facets()).await
}

#[tauri::command]
pub async fn db_get_search_index_status(
    app: tauri::AppHandle,
) -> Result<SearchIndexStatus, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.get_search_index_status()
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
    let canon_app_data = storage_dir
        .parent()
        .unwrap_or(storage_dir)
        .canonicalize()
        .map_err(|e| format!("App data path resolution failed: {}", e))?;

    if !canon_p.starts_with(&canon_storage) && !canon_p.starts_with(&canon_app_data) {
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

    if !std::path::Path::new(&path).is_file() {
        return Err("File not found or path is not a file".to_string());
    }

    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|e| format!("Failed to open file: {}", e))
}

#[tauri::command]
pub async fn db_get_versions(
    app: tauri::AppHandle,
    download_id: i64,
) -> Result<Vec<DownloadVersion>, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.get_versions(download_id)
}

#[tauri::command]
pub async fn db_get_version(
    app: tauri::AppHandle,
    download_id: i64,
    version: i64,
) -> Result<DownloadVersion, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.get_version(download_id, version)
}

#[tauri::command]
pub async fn db_delete_version(
    app: tauri::AppHandle,
    download_id: i64,
    version: i64,
) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>();
    state.db.delete_version(download_id, version)
}

#[tauri::command]
pub async fn db_set_watch_updates(
    app: tauri::AppHandle,
    download_id: i64,
    watch: bool,
) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>();
    state.db.set_watch_updates(download_id, watch)
}

#[tauri::command]
pub async fn db_set_watch_updates_for_search(
    app: tauri::AppHandle,
    params: SearchV2Params,
    watch: bool,
) -> Result<BulkMutationResult, String> {
    run_db_blocking(app, move |state| {
        state.db.set_watch_updates_for_search(&params, watch)
    })
    .await
}

#[tauri::command]
pub async fn db_set_favorite(
    app: tauri::AppHandle,
    download_id: i64,
    favorite: bool,
) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>();
    state.db.set_favorite(download_id, favorite)
}

#[tauri::command]
pub async fn db_get_watched_downloads(app: tauri::AppHandle) -> Result<Vec<DownloadEntry>, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.get_watched_downloads()
}

#[tauri::command]
pub async fn db_upsert_update_target(
    app: tauri::AppHandle,
    target: UpdateTargetInput,
) -> Result<UpdateTarget, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.upsert_update_target(&target)
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
    let state = app.state::<Arc<AppState>>();
    state
        .db
        .set_update_target_enabled(&target_type, &source, &source_key, enabled)
}

#[tauri::command]
pub async fn db_delete_update_target(
    app: tauri::AppHandle,
    target_type: String,
    source: String,
    source_key: String,
) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>();
    state
        .db
        .delete_update_target(&target_type, &source, &source_key)
}

#[tauri::command]
pub async fn db_mark_update_target_checked(
    app: tauri::AppHandle,
    target_type: String,
    source: String,
    source_key: String,
    last_seen_source_id: Option<String>,
    last_seen_source_updated_at: Option<String>,
) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>();
    state.db.mark_update_target_checked(
        &target_type,
        &source,
        &source_key,
        last_seen_source_id.as_deref(),
        last_seen_source_updated_at.as_deref(),
    )
}

#[tauri::command]
pub async fn db_list_download_relations(
    app: tauri::AppHandle,
    relation_type: Option<String>,
) -> Result<Vec<DownloadRelation>, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.list_download_relations(relation_type.as_deref())
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

async fn fetch_pixiv_series_profile_json(
    source_key: &str,
    refresh_token: Option<&str>,
    existing_title: &str,
    existing_description: Option<&str>,
) -> Result<Option<serde_json::Value>, String> {
    let Some(token) = refresh_token
        .map(str::trim)
        .filter(|token| !token.is_empty())
    else {
        return Ok(None);
    };
    let series_id: u64 = source_key.parse().map_err(|_| "Invalid Pixiv series ID")?;
    let api = crate::pixiv_api::aapi::AppPixivAPI::new_from_refresh_token(token.to_string());
    let value = api
        .novel_series(series_id, None, None, true)
        .await
        .map_err(|e| e.to_string())?;

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
        .unwrap_or(existing_title)
        .to_string();
    let cover_url = first_json_string_at(
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
    .or(existing_description);

    Ok(Some(serde_json::json!({
        "source": "pixiv",
        "sourceKey": source_key,
        "title": title,
        "description": description,
        "coverUrl": cover_url,
        "sampleNovelCount": novels.len(),
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
    if url.trim().is_empty() {
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
    let bytes = reqwest::Client::new()
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
        .map_err(|e| e.to_string())?
        .bytes()
        .await
        .map_err(|e| e.to_string())?;
    let len = bytes.len() as i64;
    tokio::fs::write(&path, bytes)
        .await
        .map_err(|e| e.to_string())?;
    Ok(Some((path.to_string_lossy().to_string(), len)))
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

    let state = app.state::<Arc<AppState>>();
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
            let api = crate::pixiv_api::aapi::AppPixivAPI::new_from_refresh_token(token);
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
            let api = crate::fanbox_api::client::FanboxAPI::new(cookie, user_agent);
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
        let existing = state.db.get_series(&source, &source_key).ok();
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
                Ok(None) => serde_json::json!({
                    "source": source,
                    "sourceKey": source_key,
                    "title": title,
                    "description": description,
                    "coverUrl": null,
                }),
                Err(error) => {
                    log::warn!(
                        "Failed to refresh Pixiv series profile {}: {}",
                        source_key,
                        error
                    );
                    serde_json::json!({
                        "source": source,
                        "sourceKey": source_key,
                        "title": title,
                        "description": description,
                        "coverUrl": null,
                    })
                }
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
        )?;
        return serde_json::to_value(series).map_err(|e| e.to_string());
    }

    Err("Unsupported entity type".to_string())
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
pub async fn db_get_download_html(
    app: tauri::AppHandle,
    download_id: i64,
    version: i64,
) -> Result<String, String> {
    let state = app.state::<Arc<AppState>>();

    // 1. ダウンロードエントリを取得
    let dl = state.db.get_download(download_id)?;

    // 2. 指定バージョンのJSONパスを特定
    let mut target_json_path = dl
        .original_json_path
        .clone()
        .filter(|p| std::path::Path::new(p).exists())
        .unwrap_or_else(|| dl.json_path.clone());
    if version != dl.current_version {
        let versions = state.db.get_versions(download_id)?;
        if let Some(v) = versions.iter().find(|x| x.version == version) {
            target_json_path = v
                .original_json_path
                .clone()
                .filter(|p| std::path::Path::new(p).exists())
                .unwrap_or_else(|| v.json_path.clone());
        }
    }

    // 3. JSONファイルを非同期ロード
    validate_path_in_storage(&target_json_path, state.db.storage_dir())?;
    let raw_json = tokio::fs::read_to_string(&target_json_path)
        .await
        .map_err(|e| format!("Failed to read JSON file: {}", e))?;

    // 4. 紐づくアセット一覧を取得
    let assets = state.db.get_assets(download_id)?;

    // 5. ソースに応じて構文解析を実行
    let html = if dl.source == "pixiv" {
        crate::database::parser::parse_pixiv_to_html(&raw_json, &assets)
    } else if dl.source == "fanbox" {
        crate::database::parser::parse_fanbox_to_html(&raw_json, &assets)
    } else {
        return Err(format!("Unsupported source: {}", dl.source));
    };

    Ok(html)
}

#[tauri::command]
pub async fn db_get_reader_document(
    app: tauri::AppHandle,
    download_id: i64,
    version: Option<i64>,
) -> Result<ReaderDocument, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.get_reader_document(download_id, version)
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
    let state = app.state::<Arc<AppState>>();
    state
        .db
        .save_work_draft(download_id, base_version, blocks.as_slice())
}

#[tauri::command]
pub async fn db_activate_work_edit(
    app: tauri::AppHandle,
    edit_revision_id: i64,
) -> Result<WorkEditRevision, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.activate_work_edit(edit_revision_id)
}

#[tauri::command]
pub async fn import_work_asset(
    app: tauri::AppHandle,
    download_id: i64,
    source_path: String,
) -> Result<AssetEntry, String> {
    let state = app.state::<Arc<AppState>>();
    let source = std::path::PathBuf::from(&source_path);
    if !source.is_file() {
        return Err("Import source is not a file".to_string());
    }

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

    tokio::fs::copy(&source, &target_path)
        .await
        .map_err(|e| format!("Failed to copy editor asset: {}", e))?;
    let metadata = tokio::fs::metadata(&target_path)
        .await
        .map_err(|e| format!("Failed to inspect editor asset: {}", e))?;
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
        file_size_bytes: metadata.len() as i64,
    };
    let id = state.db.insert_asset(&asset)?;

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
    use super::collect_profile_links;

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
}
