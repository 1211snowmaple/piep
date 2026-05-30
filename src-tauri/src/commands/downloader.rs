use crate::database::queries::EntityProfileFreshness;
use crate::database::{DownloadEntry, NewAsset, NewDownload, NewVersion};
use crate::downloader::fanbox::{get_post_detail, FanboxPost};
use crate::downloader::pixiv::{get_novel_detail, PixivNovelContent};
use crate::fanbox_api::client::FanboxAPI;
use crate::pixiv_api;
use crate::AppState;
use std::path::Path;
use std::sync::Arc;
use tauri::Manager;

#[tauri::command]
pub async fn fetch_pixiv_novel(
    novel_id: String,
    refresh_token: String,
) -> Result<PixivNovelContent, String> {
    let detail = get_novel_detail(&novel_id, &refresh_token)
        .await
        .map_err(|e| e.to_string())?;
    let (text, cover_url, illusts, images) =
        crate::downloader::pixiv::get_novel_text_and_assets(&novel_id, &refresh_token)
            .await
            .map_err(|e| e.to_string())?;
    Ok(PixivNovelContent {
        detail,
        text,
        cover_url: Some(cover_url),
        illusts: Some(illusts),
        images: Some(images),
    })
}

#[tauri::command]
pub async fn fetch_pixiv_novel_metadata(
    novel_id: String,
    refresh_token: String,
) -> Result<crate::downloader::pixiv::PixivNovelDetail, String> {
    get_novel_detail(&novel_id, &refresh_token)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn fetch_fanbox_post(
    post_id: String,
    cookie: String,
    user_agent: String,
) -> Result<FanboxPost, String> {
    get_post_detail(&post_id, &cookie, &user_agent)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn fetch_fanbox_creator_posts(
    creator_id: String,
    cookie: String,
    user_agent: String,
) -> Result<Vec<FanboxPost>, String> {
    let api = FanboxAPI::new(cookie, user_agent);
    api.get_all_creator_posts(&creator_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn fetch_pixiv_series_novels(
    series_id: String,
    refresh_token: String,
) -> Result<Vec<pixiv_api::models::NovelInfo>, String> {
    let api = pixiv_api::aapi::AppPixivAPI::new_from_refresh_token(refresh_token);
    let id_u64: u64 = series_id.parse().map_err(|_| "Invalid series ID")?;

    let mut last_order: Option<String> = None;
    let mut all_novels = Vec::new();
    let mut page_count = 0;

    loop {
        page_count += 1;
        if page_count > 100 {
            log::warn!("fetch_pixiv_series_novels: Reached safety page limit of 100 pages. Terminating loop to prevent infinite request recursion.");
            break;
        }

        let res = api
            .novel_series(id_u64, None, last_order.as_deref(), true)
            .await
            .map_err(|e| e.to_string())?;

        #[derive(serde::Deserialize)]
        struct NovelSeriesResponse {
            novels: Vec<pixiv_api::models::NovelInfo>,
            next_url: Option<String>,
        }

        let parsed: NovelSeriesResponse = serde_json::from_value(res).map_err(|e| e.to_string())?;
        all_novels.extend(parsed.novels);

        if let Some(url) = parsed.next_url {
            if let Some(pos) = url.find("last_order=") {
                let last_order_str = &url[pos + 11..];
                let last_order_val: String = last_order_str
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                last_order = Some(last_order_val);
            } else {
                break;
            }
        } else {
            break;
        }
    }

    Ok(all_novels)
}

#[tauri::command]
pub async fn fetch_pixiv_user_novels(
    user_id: String,
    refresh_token: String,
) -> Result<Vec<pixiv_api::models::NovelInfo>, String> {
    let api = pixiv_api::aapi::AppPixivAPI::new_from_refresh_token(refresh_token);
    let id_u64: u64 = user_id.parse().map_err(|_| "Invalid user ID")?;

    let mut offset: Option<String> = None;
    let mut all_novels = Vec::new();
    let mut page_count = 0;

    loop {
        page_count += 1;
        if page_count > 100 {
            log::warn!("fetch_pixiv_user_novels: Reached safety page limit of 100 pages. Terminating loop to prevent infinite request recursion.");
            break;
        }

        let res = api
            .user_novels(id_u64, None, offset.as_deref(), true)
            .await
            .map_err(|e| e.to_string())?;

        all_novels.extend(res.novels);

        if let Some(url) = res.next_url {
            if let Some(pos) = url.find("offset=") {
                let offset_str = &url[pos + 7..];
                let offset_val: String = offset_str
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                offset = Some(offset_val);
            } else {
                break;
            }
        } else {
            break;
        }
    }

    Ok(all_novels)
}

#[tauri::command]
pub async fn db_check_exists(
    app: tauri::AppHandle,
    source: String,
    source_id: String,
) -> Result<bool, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.check_exists(&source, &source_id)
}

#[tauri::command]
pub async fn fetch_pixiv_novel_by_url(
    url: String,
    refresh_token: String,
) -> Result<PixivNovelContent, String> {
    let novel_id =
        crate::downloader::pixiv::extract_novel_id(&url).ok_or("Invalid Pixiv Novel URL")?;
    let detail = get_novel_detail(&novel_id, &refresh_token)
        .await
        .map_err(|e| e.to_string())?;
    let (text, cover_url, illusts, images) =
        crate::downloader::pixiv::get_novel_text_and_assets(&novel_id, &refresh_token)
            .await
            .map_err(|e| e.to_string())?;
    Ok(PixivNovelContent {
        detail,
        text,
        cover_url: Some(cover_url),
        illusts: Some(illusts),
        images: Some(images),
    })
}

/// 本文テキストのハッシュ値、文字数、更新時間を算出する（Pixivはメタデータを含む統合ハッシュ）
fn compute_content_details(
    data: &serde_json::Value,
    source: &str,
) -> (String, i64, Option<String>) {
    use sha2::{Digest, Sha256};
    let mut text = String::new();
    let mut source_updated_at = None;
    let mut meta_merged = String::new();

    if source == "pixiv" {
        // 1. 本文テキスト
        text = data
            .get("text")
            .or_else(|| data.get("detail").and_then(|d| d.get("text")))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // 2. 作者が編集できる主要メタデータを抽出
        let title = data
            .get("title")
            .or_else(|| data.get("detail").and_then(|d| d.get("title")))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let caption = data
            .get("caption")
            .or_else(|| data.get("detail").and_then(|d| d.get("caption")))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let cover_url = data
            .get("coverUrl")
            .or_else(|| data.get("detail").and_then(|d| d.get("coverUrl")))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let tags_str = data
            .get("tags")
            .or_else(|| data.get("detail").and_then(|d| d.get("tags")))
            .map(|t| {
                if let Some(arr) = t.as_array() {
                    arr.iter()
                        .map(|v| v.as_str().unwrap_or(""))
                        .collect::<Vec<&str>>()
                        .join(",")
                } else {
                    t.as_str().unwrap_or("").to_string()
                }
            })
            .unwrap_or_default();

        let series_id = data
            .get("seriesId")
            .or_else(|| data.get("detail").and_then(|d| d.get("seriesId")))
            .map(|v| {
                if v.is_number() {
                    v.as_i64().map(|n| n.to_string()).unwrap_or_default()
                } else {
                    v.as_str().unwrap_or("").to_string()
                }
            })
            .unwrap_or_default();

        // 3. ハッシュ用にすべてを統合
        meta_merged = format!(
            "TITLE:{}\nCAPTION:{}\nCOVER:{}\nTAGS:{}\nSERIES:{}\nBODY:{}",
            title, caption, cover_url, tags_str, series_id, text
        );
    } else if source == "fanbox" {
        if let Some(body) = data.get("body") {
            if let Some(blocks) = body.get("blocks").and_then(|b| b.as_array()) {
                let parts: Vec<String> = blocks
                    .iter()
                    .map(|block| {
                        let block_type = block.get("type").and_then(|t| t.as_str());
                        if block_type == Some("p") || block_type == Some("header") {
                            block
                                .get("text")
                                .and_then(|t| t.as_str())
                                .unwrap_or("")
                                .to_string()
                        } else {
                            "".to_string()
                        }
                    })
                    .filter(|t| !t.trim().is_empty())
                    .collect();
                text = parts.join("\n\n");
            } else if let Some(txt) = body.get("text").and_then(|t| t.as_str()) {
                text = txt.to_string();
            }
        }
        source_updated_at = data
            .get("updatedDatetime")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }

    let text_len = text.chars().count() as i64;
    let mut hasher = Sha256::new();
    if source == "pixiv" {
        hasher.update(meta_merged.as_bytes());
    } else {
        if !text.is_empty() {
            hasher.update(text.as_bytes());
        } else {
            hasher.update(serde_json::to_string(data).unwrap_or_default().as_bytes());
        }
    }
    let hash_bytes = hasher.finalize();
    let hash = hash_bytes
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();

    (hash, text_len, source_updated_at)
}

fn fanbox_has_material_content(data: &serde_json::Value) -> bool {
    let Some(body) = data.get("body").filter(|v| !v.is_null()) else {
        return false;
    };

    if body
        .get("text")
        .and_then(|v| v.as_str())
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
    {
        return true;
    }

    if body
        .get("blocks")
        .and_then(|v| v.as_array())
        .map(|blocks| {
            blocks.iter().any(|block| {
                block
                    .get("text")
                    .and_then(|v| v.as_str())
                    .map(|s| !s.trim().is_empty())
                    .unwrap_or(false)
                    || block
                        .get("type")
                        .and_then(|v| v.as_str())
                        .map(|t| matches!(t, "image" | "file" | "embed"))
                        .unwrap_or(false)
            })
        })
        .unwrap_or(false)
    {
        return true;
    }

    ["imageMap", "fileMap", "images", "files"]
        .iter()
        .any(|key| {
            body.get(key)
                .map(|v| {
                    v.as_array().map(|a| !a.is_empty()).unwrap_or(false)
                        || v.as_object().map(|o| !o.is_empty()).unwrap_or(false)
                })
                .unwrap_or(false)
        })
}

fn json_string_at<'a>(value: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        if let Some(s) = value.get(*key).and_then(|v| v.as_str()) {
            if !s.trim().is_empty() {
                return Some(s);
            }
        }
        if let Some(s) = value
            .get("detail")
            .and_then(|d| d.get(*key))
            .and_then(|v| v.as_str())
        {
            if !s.trim().is_empty() {
                return Some(s);
            }
        }
    }
    None
}

fn json_id_at(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        let direct = value.get(*key);
        let nested = value.get("detail").and_then(|d| d.get(*key));
        for candidate in [direct, nested].into_iter().flatten() {
            if let Some(s) = candidate.as_str() {
                if !s.trim().is_empty() {
                    return Some(s.to_string());
                }
            }
            if let Some(n) = candidate.as_u64() {
                return Some(n.to_string());
            }
        }
    }
    None
}

fn extract_series_relation(data: &serde_json::Value) -> Option<(String, String)> {
    let series_id = json_id_at(data, &["seriesId", "series_id"])?;
    let series_title = json_string_at(data, &["seriesTitle", "series_title"])
        .or_else(|| {
            data.get("seriesNavigation")
                .and_then(|v| v.get("seriesTitle"))
                .and_then(|v| v.as_str())
        })
        .or_else(|| {
            data.get("detail")
                .and_then(|d| d.get("seriesNavigation"))
                .and_then(|v| v.get("seriesTitle"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("シリーズ");
    Some((series_id, series_title.to_string()))
}

fn extract_series_content_order(data: &serde_json::Value) -> Option<i64> {
    json_id_at(data, &["contentOrder", "content_order", "order"])
        .and_then(|s| s.parse::<i64>().ok())
        .or_else(|| {
            first_string_at(
                data,
                &[
                    &["detail", "contentOrder"],
                    &["detail", "content_order"],
                    &["detail", "order"],
                ],
            )
            .and_then(|s| s.parse::<i64>().ok())
        })
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

fn sha256_json(value: &serde_json::Value) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect())
}

fn first_string_at<'a>(value: &'a serde_json::Value, paths: &[&[&str]]) -> Option<&'a str> {
    for path in paths {
        let mut cursor = value;
        let mut ok = true;
        for key in *path {
            if let Some(next) = cursor.get(*key) {
                cursor = next;
            } else {
                ok = false;
                break;
            }
        }
        if ok {
            if let Some(s) = cursor.as_str() {
                if !s.trim().is_empty() {
                    return Some(s);
                }
            }
        }
    }
    None
}

async fn save_person_snapshot_from_download(
    state: &Arc<AppState>,
    data: &serde_json::Value,
    source: &str,
    author_id: &str,
    author_name: &str,
) -> Result<(), String> {
    if author_id.trim().is_empty() || author_name.trim().is_empty() {
        return Ok(());
    }
    if state
        .db
        .get_person(source, author_id)
        .ok()
        .and_then(|p| p.content_hash)
        .is_some()
    {
        return Ok(());
    }

    let icon_url = first_string_at(
        data,
        &[
            &["detail", "user", "profile_image_urls", "medium"],
            &["user", "profile_image_urls", "medium"],
            &["user", "iconUrl"],
            &["user", "icon_url"],
        ],
    );
    let description = first_string_at(
        data,
        &[&["detail", "user", "comment"], &["user", "comment"]],
    );
    let profile_url = if source == "pixiv" {
        format!("https://www.pixiv.net/users/{}", author_id)
    } else {
        format!("https://{}.fanbox.cc/", author_id)
    };
    let normalized = serde_json::json!({
        "source": source,
        "sourceKey": author_id,
        "displayName": author_name,
        "iconUrl": icon_url,
        "coverUrl": null,
        "description": description,
        "links": [profile_url],
    });
    let hash = sha256_json(&normalized)?;
    let existing = state.db.get_person(source, author_id).ok();
    let json_path =
        if existing.as_ref().and_then(|p| p.content_hash.as_deref()) == Some(hash.as_str()) {
            String::new()
        } else {
            let next_version = existing
                .as_ref()
                .map(|p| p.current_version + 1)
                .unwrap_or(1);
            let dir = state
                .db
                .storage_dir()
                .parent()
                .unwrap_or_else(|| state.db.storage_dir())
                .join("profiles")
                .join(safe_entity_segment(source))
                .join(safe_entity_segment(author_id))
                .join(format!("v{}", next_version));
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(|e| e.to_string())?;
            let path = dir.join("original.json");
            let content = serde_json::to_string_pretty(&normalized).map_err(|e| e.to_string())?;
            tokio::fs::write(&path, content)
                .await
                .map_err(|e| e.to_string())?;
            path.to_string_lossy().to_string()
        };
    let links_json = serde_json::to_string(&normalized["links"]).ok();
    state.db.upsert_person_profile(
        source,
        author_id,
        author_name,
        None,
        None,
        description,
        links_json.as_deref(),
        &hash,
        &json_path,
        0,
        0,
        EntityProfileFreshness::SnapshotOnly,
    )?;
    Ok(())
}

async fn save_series_snapshot_from_download(
    state: &Arc<AppState>,
    data: &serde_json::Value,
    source: &str,
) -> Result<(), String> {
    let Some((series_id, series_title)) = extract_series_relation(data) else {
        return Ok(());
    };
    if state
        .db
        .get_series(source, &series_id)
        .ok()
        .and_then(|s| s.content_hash)
        .is_some()
    {
        return Ok(());
    }
    let normalized = serde_json::json!({
        "source": source,
        "sourceKey": series_id,
        "title": series_title,
        "description": null,
        "coverUrl": first_string_at(data, &[&["coverUrl"], &["cover_url"], &["detail", "coverUrl"], &["detail", "cover_url"]]),
        "seriesNavigation": data.get("seriesNavigation").or_else(|| data.get("detail").and_then(|d| d.get("seriesNavigation"))),
    });
    let hash = sha256_json(&normalized)?;
    let existing = state.db.get_series(source, &series_id).ok();
    let json_path =
        if existing.as_ref().and_then(|s| s.content_hash.as_deref()) == Some(hash.as_str()) {
            String::new()
        } else {
            let next_version = existing
                .as_ref()
                .map(|s| s.current_version + 1)
                .unwrap_or(1);
            let dir = state
                .db
                .storage_dir()
                .parent()
                .unwrap_or_else(|| state.db.storage_dir())
                .join("series")
                .join(safe_entity_segment(source))
                .join(safe_entity_segment(&series_id))
                .join(format!("v{}", next_version));
            tokio::fs::create_dir_all(&dir)
                .await
                .map_err(|e| e.to_string())?;
            let path = dir.join("original.json");
            let content = serde_json::to_string_pretty(&normalized).map_err(|e| e.to_string())?;
            tokio::fs::write(&path, content)
                .await
                .map_err(|e| e.to_string())?;
            path.to_string_lossy().to_string()
        };
    state.db.upsert_series_profile(
        source,
        &series_id,
        &series_title,
        None,
        None,
        &hash,
        &json_path,
        0,
        0,
        EntityProfileFreshness::SnapshotOnly,
    )?;
    Ok(())
}

async fn sync_download_entities(
    state: &Arc<AppState>,
    data: &serde_json::Value,
    download_id: i64,
    source: &str,
    author_id: &str,
    author_name: &str,
) {
    if let Err(e) =
        state
            .db
            .upsert_download_relation(download_id, "author", source, author_id, author_name)
    {
        log::warn!("Failed to register author relation: {}", e);
    }
    if let Err(e) = state.db.upsert_download_person(
        download_id,
        source,
        author_id,
        if source == "fanbox" {
            "creator"
        } else {
            "author"
        },
        author_name,
    ) {
        log::warn!("Failed to register person relation: {}", e);
    }
    if let Err(e) =
        save_person_snapshot_from_download(state, data, source, author_id, author_name).await
    {
        log::warn!("Failed to save person snapshot: {}", e);
    }
    if source == "pixiv" {
        if let Some((series_id, series_title)) = extract_series_relation(data) {
            if let Err(e) = state.db.upsert_download_relation(
                download_id,
                "series",
                source,
                &series_id,
                &series_title,
            ) {
                log::warn!("Failed to register series relation: {}", e);
            }
            if let Err(e) = state.db.upsert_download_series(
                download_id,
                source,
                &series_id,
                &series_title,
                extract_series_content_order(data),
            ) {
                log::warn!("Failed to register normalized series relation: {}", e);
            }
        }
        if let Err(e) = save_series_snapshot_from_download(state, data, source).await {
            log::warn!("Failed to save series snapshot: {}", e);
        }
    }
}

/// ディレクトリを再帰的にコピーする補助関数
fn copy_dir_all_sync(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all_sync(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            std::fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn download_and_save(
    app: tauri::AppHandle,
    data: serde_json::Value,
    source: String,
    source_id: String,
    title: String,
    author_name: String,
    author_id: String,
    content_type: String,
    tags: Option<Vec<String>>,
    excerpt: Option<String>,
    source_created_at: Option<String>,
    cookie: Option<String>,
    user_agent: Option<String>,
) -> Result<DownloadEntry, String> {
    let state = app.state::<Arc<AppState>>();
    let storage = state.db.storage_dir().to_path_buf();
    let root_item_dir = storage.join(&source).join(&source_id);

    // 1. ハッシュ値と文字数を算出
    let (new_hash, new_text_len, new_source_updated) = compute_content_details(&data, &source);

    // 2. 既存チェックとバージョン番号の特定
    let existing_dl = state.db.get_download_by_source(&source, &source_id)?;
    let mut next_version = 1i64;
    let mut is_update = false;
    let mut _existing_id = None;

    if let Some(ref dl) = existing_dl {
        _existing_id = Some(dl.id);
        if source == "fanbox"
            && (dl.text_length > 0 || dl.asset_count > 0)
            && !fanbox_has_material_content(&data)
        {
            sync_download_entities(
                &state,
                &data,
                dl.id,
                &source,
                &dl.author_id,
                &dl.author_name,
            )
            .await;
            if let Err(e) = state.db.reindex_download(dl.id) {
                log::warn!("Failed to refresh search index for {}: {}", dl.id, e);
            }
            log::info!(
                "Skipping FANBOX version update for {} because fetched post has no material body/assets",
                source_id
            );
            return state.db.get_download(dl.id);
        }

        let hash_changed = dl.content_hash.as_deref() != Some(&new_hash);
        let updated_changed = if source == "fanbox" {
            dl.source_updated_at != new_source_updated
        } else {
            false
        };

        if hash_changed || updated_changed {
            is_update = true;
            next_version = dl.current_version + 1;
        } else {
            sync_download_entities(
                &state,
                &data,
                dl.id,
                &source,
                &dl.author_id,
                &dl.author_name,
            )
            .await;
            if let Err(e) = state.db.reindex_download(dl.id) {
                log::warn!("Failed to refresh search index for {}: {}", dl.id, e);
            }
            return state.db.get_download(dl.id);
        }
    }

    // 新しいバージョン用のアイテムディレクトリ
    let item_dir = root_item_dir.join(format!("v{}", next_version));
    tokio::fs::create_dir_all(&item_dir)
        .await
        .map_err(|e| e.to_string())?;

    // 既存 v1 への自動マイグレーション（フォルダ直下にファイルがあり、かつ v1 フォルダがまだない場合）
    if existing_dl.is_some() && is_update && next_version == 2 {
        let v1_dir = root_item_dir.join("v1");
        if !v1_dir.exists() {
            tokio::fs::create_dir_all(&v1_dir)
                .await
                .map_err(|e| e.to_string())?;

            // プレミアム最適化：スレッドプールのワーカースレッドに同期ディスク操作を委譲
            let src_root = root_item_dir.clone();
            let dest_root = v1_dir.clone();
            tokio::task::spawn_blocking(move || {
                let paths_to_move = vec!["data.json", "original.json", "data_assets"];
                for name in paths_to_move {
                    let src_path = src_root.join(name);
                    let dest_path = dest_root.join(name);
                    if src_path.exists() {
                        if src_path.is_dir() {
                            copy_dir_all_sync(&src_path, &dest_path)?;
                            std::fs::remove_dir_all(&src_path)?;
                        } else {
                            std::fs::copy(&src_path, &dest_path)?;
                            std::fs::remove_file(&src_path)?;
                        }
                    }
                }
                Ok::<(), std::io::Error>(())
            })
            .await
            .map_err(|e| format!("Migration thread panicked: {}", e))?
            .map_err(|e| format!("Migration failed: {}", e))?;

            // DBに v1 の履歴を追加
            if let Some(ref dl) = existing_dl {
                let v1_orig_path = v1_dir.join("original.json");
                let v1_data_path = v1_dir.join("data.json");
                let resolved_v1_path = if v1_orig_path.exists() {
                    v1_orig_path
                } else if v1_data_path.exists() {
                    v1_data_path
                } else {
                    v1_orig_path
                };

                state.db.insert_version(&NewVersion {
                    download_id: dl.id,
                    version: 1,
                    content_hash: dl.content_hash.clone(),
                    text_length: dl.text_length,
                    json_path: resolved_v1_path.to_string_lossy().to_string(),
                    original_json_path: Some(resolved_v1_path.to_string_lossy().to_string()),
                    asset_count: dl.asset_count,
                    file_size_bytes: dl.file_size_bytes,
                    created_at: dl.downloaded_at.clone(),
                    change_summary: Some("初期バージョン (自動移行)".to_string()),
                })?;
            }
        }
    }

    let is_fanbox = source == "fanbox";

    // 1. オリジナルJSON保存
    let original_json_path = item_dir.join("original.json");
    let original_content = serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?;
    tokio::fs::write(&original_json_path, &original_content)
        .await
        .map_err(|e| e.to_string())?;

    // 2. アセットダウンロード + ローカルパスインジェクション (インメモリで実行するため、パスインジェクション結果は disk に保存せず DB登録やアセット情報収集のためにのみ一時的に使用)
    let mut modified_data = data.clone();
    // Assetsフォルダ名は "data_assets" にするため、dummyとして data.json のパスを指定します
    let dummy_json_path = item_dir.join("data.json");

    if let Err(e) = crate::downloader::asset_downloader::download_and_link_assets(
        &app,
        &mut modified_data,
        &dummy_json_path,
        is_fanbox,
        cookie,
        user_agent,
    )
    .await
    {
        log::error!("Asset download had errors: {}", e);
    }

    // 3. アセットの情報を収集
    let assets_dir = item_dir.join("data_assets");

    // プレミアム最適化：激重な再帰ディレクトリ走査とサイズ計算を非同期ワーカースレッドへ完璧に移譲
    let (asset_entries, total_size, cover_path_str) = if assets_dir.exists() {
        let assets_dir_clone = assets_dir.clone();
        let initial_size = original_content.len() as i64;

        tokio::task::spawn_blocking(move || {
            let mut entries = Vec::new();
            let mut size = initial_size;
            let mut cover = None;
            collect_assets_recursive(&assets_dir_clone, &mut entries, &mut size, &mut cover, 0)?;
            Ok::<(Vec<NewAsset>, i64, Option<String>), String>((entries, size, cover))
        })
        .await
        .map_err(|e| format!("Asset collect thread panicked: {}", e))??
    } else {
        (Vec::new(), original_content.len() as i64, None)
    };

    let json_path = original_json_path.clone();

    // 5. DB登録
    let tags_json = tags.map(|t| serde_json::to_string(&t).unwrap_or_default());

    let new_dl = NewDownload {
        source: source.clone(),
        source_id: source_id.clone(),
        title,
        author_name,
        author_id,
        content_type,
        tags: tags_json,
        excerpt,
        cover_path: cover_path_str,
        json_path: json_path.to_string_lossy().to_string(),
        original_json_path: Some(original_json_path.to_string_lossy().to_string()),
        asset_count: asset_entries.len() as i64,
        file_size_bytes: total_size,
        downloaded_at: chrono::Utc::now().to_rfc3339(),
        source_created_at,
        content_hash: Some(new_hash.clone()),
        text_length: new_text_len,
        source_updated_at: new_source_updated.clone(),
        watch_updates: existing_dl
            .as_ref()
            .map(|dl| dl.watch_updates)
            .unwrap_or(false),
        current_version: next_version,
        favorite: existing_dl.as_ref().map(|dl| dl.favorite).unwrap_or(false),
    };

    let dl_id = state.db.upsert_download(&new_dl)?;

    sync_download_entities(
        &state,
        &data,
        dl_id,
        &source,
        &new_dl.author_id,
        &new_dl.author_name,
    )
    .await;

    // アセットをDB登録
    for mut asset in asset_entries {
        asset.download_id = dl_id;
        if let Err(e) = state.db.insert_asset(&asset) {
            log::error!("Failed to insert asset to database: {}", e);
        }
    }

    // 6. 今回のバージョン履歴レコードをDB挿入
    // ただし、既存レコードがあり、かつ今回コンテンツ更新がなかった（is_update = false）場合は、
    // すでにそのバージョン（e.g. v1）の履歴レコードが存在するため、一意制約エラーを避けるために挿入をスキップします。
    if existing_dl.is_none() || is_update {
        let change_summary = if is_update {
            format!("本文・コンテンツの更新 (v{})", next_version)
        } else {
            format!("新規ダウンロード (v{})", next_version)
        };

        state.db.insert_version(&NewVersion {
            download_id: dl_id,
            version: next_version,
            content_hash: Some(new_hash),
            text_length: new_text_len,
            json_path: json_path.to_string_lossy().to_string(),
            original_json_path: Some(original_json_path.to_string_lossy().to_string()),
            asset_count: new_dl.asset_count,
            file_size_bytes: new_dl.file_size_bytes,
            created_at: new_dl.downloaded_at.clone(),
            change_summary: Some(change_summary),
        })?;
    }

    if let Err(e) = state.db.reindex_download(dl_id) {
        log::warn!("Failed to refresh search index for {}: {}", dl_id, e);
    }

    state.db.get_download(dl_id)
}

fn collect_assets_recursive(
    dir: &std::path::Path,
    assets: &mut Vec<NewAsset>,
    total_size: &mut i64,
    cover_path: &mut Option<String>,
    _depth: u32,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            collect_assets_recursive(&path, assets, total_size, cover_path, _depth + 1)?;
        } else {
            let filename = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let size = path.metadata().map(|m| m.len() as i64).unwrap_or(0);
            *total_size += size;

            let parent_name = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("");

            let asset_type = if parent_name == "cover" {
                if cover_path.is_none() {
                    *cover_path = Some(path.to_string_lossy().to_string());
                }
                "cover"
            } else if parent_name == "illustrations" {
                "illustration"
            } else {
                "file"
            };

            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let mime = match ext {
                "jpg" | "jpeg" => Some("image/jpeg".to_string()),
                "png" => Some("image/png".to_string()),
                "gif" => Some("image/gif".to_string()),
                "webp" => Some("image/webp".to_string()),
                "pdf" => Some("application/pdf".to_string()),
                _ => None,
            };

            assets.push(NewAsset {
                download_id: 0,
                asset_type: asset_type.to_string(),
                filename,
                local_path: path.to_string_lossy().to_string(),
                original_url: None,
                mime_type: mime,
                file_size_bytes: size,
            });
        }
    }
    Ok(())
}
