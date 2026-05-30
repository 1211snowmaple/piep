use crate::database::queries::EntityProfileFreshness;
use crate::database::{
    AssetEntry, DbStats, DownloadEntry, DownloadRelation, DownloadVersion, EntityVersion,
    FacetCount, FilterFacets, PersonEntry, SearchIndexStatus, SearchParams, SeriesEntry,
    UpdateTarget, UpdateTargetInput,
};
use crate::AppState;
use base64::Engine as _;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::sync::LazyLock;
use std::time::Instant;
use tauri::Manager;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::Mutex as AsyncMutex;
use tokio::time::{sleep, Duration};

static ENTITY_REFRESH_LOCK: LazyLock<AsyncMutex<Option<Instant>>> =
    LazyLock::new(|| AsyncMutex::new(None));

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

#[tauri::command]
pub async fn db_search_downloads(
    app: tauri::AppHandle,
    params: SearchParams,
) -> Result<Vec<DownloadEntry>, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.search_downloads(&params)
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
pub async fn db_get_stats(app: tauri::AppHandle) -> Result<DbStats, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.get_stats()
}

#[tauri::command]
pub async fn db_get_filter_facets(app: tauri::AppHandle) -> Result<FilterFacets, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.get_filter_facets()
}

#[tauri::command]
pub async fn db_get_search_index_status(
    app: tauri::AppHandle,
) -> Result<SearchIndexStatus, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.get_search_index_status()
}

#[tauri::command]
pub async fn db_rebuild_search_index_batch(
    app: tauri::AppHandle,
    limit: Option<i64>,
) -> Result<SearchIndexStatus, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.rebuild_search_index_batch(limit.unwrap_or(24))
}

#[tauri::command]
pub async fn db_search_filter_facets(
    app: tauri::AppHandle,
    kind: String,
    query: Option<String>,
    limit: Option<i64>,
) -> Result<Vec<FacetCount>, String> {
    let state = app.state::<Arc<AppState>>();
    state
        .db
        .search_filter_facets(&kind, query.as_deref(), limit.unwrap_or(30))
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
pub async fn read_image_base64(app: tauri::AppHandle, path: String) -> Result<String, String> {
    let state = app.state::<Arc<AppState>>();
    validate_path_in_storage(&path, state.db.storage_dir())?;

    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| format!("Failed to read image: {}", e))?;
    let ext = std::path::Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png");
    let mime = match ext {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    };
    Ok(format!(
        "data:{};base64,{}",
        mime,
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
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
            let links = [
                format!("https://www.pixiv.net/users/{}", detail.user.id),
                detail.profile.webpage.clone().unwrap_or_default(),
                detail.profile.twitter_url.clone().unwrap_or_default(),
            ]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<String>>();
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
            serde_json::json!({
                "source": source,
                "sourceKey": source_key,
                "displayName": detail.user.name,
                "iconUrl": detail.user.icon_url,
                "coverUrl": detail.cover_image_url,
                "description": detail.description,
                "links": detail.profile_links,
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
        let normalized = serde_json::json!({
            "source": source,
            "sourceKey": source_key,
            "title": title,
            "description": existing.as_ref().and_then(|s| s.description.clone()),
            "coverUrl": null,
        });
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
        let series = state.db.upsert_series_profile(
            &source,
            &source_key,
            normalized
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or(&source_key),
            normalized.get("description").and_then(|v| v.as_str()),
            None,
            &hash,
            &json_path,
            0,
            json_size,
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
