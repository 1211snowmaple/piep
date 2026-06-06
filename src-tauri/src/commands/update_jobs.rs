use crate::database::{
    DownloadEntry, StartUpdateJobRequest, UpdateCredentials, UpdateJobItem, UpdateJobItemInput,
    UpdateJobSnapshot, UpdateJobSummary, UpdateTargetInput,
};
use crate::AppState;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{Emitter, Manager};

const UPDATE_JOB_DELAY_MS: u64 = 800;
static UPDATE_JOB_COUNTER: AtomicU64 = AtomicU64::new(1);
static ACTIVE_UPDATE_JOBS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn active_jobs() -> &'static Mutex<HashSet<String>> {
    ACTIVE_UPDATE_JOBS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn make_job_id() -> String {
    let sequence = UPDATE_JOB_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("job-{}-{}", chrono::Utc::now().timestamp_millis(), sequence)
}

fn pixiv_token(credentials: &UpdateCredentials) -> Option<String> {
    credentials
        .pixiv_refresh_token
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
}

fn fanbox_cookie(credentials: &UpdateCredentials) -> Option<String> {
    credentials
        .fanbox_cookie
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
}

fn fanbox_user_agent(credentials: &UpdateCredentials) -> String {
    credentials
        .fanbox_user_agent
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "Mozilla/5.0".to_string())
}

fn value_at<'a>(value: &'a Value, paths: &[&[&str]]) -> Option<&'a Value> {
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
            return Some(cursor);
        }
    }
    None
}

fn string_at(value: &Value, paths: &[&[&str]]) -> Option<String> {
    value_at(value, paths).and_then(|candidate| {
        if let Some(s) = candidate.as_str() {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        candidate.as_i64().map(|n| n.to_string())
    })
}

fn pixiv_tags(data: &Value) -> Vec<String> {
    let Some(tags) = value_at(data, &[&["detail", "tags"], &["tags"]]) else {
        return Vec::new();
    };
    if let Some(items) = tags.as_array() {
        return items
            .iter()
            .filter_map(|tag| {
                tag.as_str()
                    .map(str::to_string)
                    .or_else(|| string_at(tag, &[&["name"]]))
            })
            .collect();
    }
    if let Some(items) = tags.get("tags").and_then(|v| v.as_array()) {
        return items
            .iter()
            .filter_map(|tag| {
                tag.as_str()
                    .map(str::to_string)
                    .or_else(|| string_at(tag, &[&["name"]]))
            })
            .collect();
    }
    Vec::new()
}

fn normalize_pixiv_novel_id(item: &Value) -> Option<String> {
    string_at(item, &[&["id"], &["detail", "id"]])
}

fn normalize_series(item: &Value) -> Option<(String, String)> {
    let id = string_at(
        item,
        &[
            &["seriesId"],
            &["series_id"],
            &["detail", "seriesId"],
            &["detail", "series_id"],
            &["series", "id"],
            &["seriesNavigation", "seriesId"],
            &["series_navigation", "series_id"],
            &["detail", "seriesNavigation", "seriesId"],
        ],
    )?;
    let title = string_at(
        item,
        &[
            &["seriesTitle"],
            &["series_title"],
            &["detail", "seriesTitle"],
            &["detail", "series_title"],
            &["series", "title"],
            &["seriesNavigation", "seriesTitle"],
            &["series_navigation", "series_title"],
            &["detail", "seriesNavigation", "seriesTitle"],
        ],
    )
    .unwrap_or_else(|| "シリーズ".to_string());
    Some((id, title))
}

fn candidate_payload(
    target_type: &str,
    target_label: &str,
    source: &str,
    item: &Value,
) -> Option<Value> {
    let source_id = if source == "pixiv" {
        normalize_pixiv_novel_id(item)?
    } else {
        string_at(item, &[&["id"]])?
    };
    let title = string_at(item, &[&["title"]]).unwrap_or_else(|| "無題".to_string());
    let author = string_at(item, &[&["user", "name"]]).unwrap_or_else(|| target_label.to_string());
    let date = if source == "pixiv" {
        string_at(item, &[&["create_date"], &["createDate"]]).unwrap_or_default()
    } else {
        string_at(
            item,
            &[
                &["publishedDatetime"],
                &["published_datetime"],
                &["updatedDatetime"],
                &["updated_datetime"],
            ],
        )
        .unwrap_or_default()
    };
    Some(serde_json::json!({
        "source": source,
        "sourceId": source_id,
        "title": title,
        "subtitle": format!("{} / {}", author, date),
        "targetLabel": target_label,
        "targetType": target_type,
        "originalData": item,
    }))
}

fn item_from_payload(
    item_type: &str,
    status: &str,
    source: Option<String>,
    source_id: Option<String>,
    target_type: Option<String>,
    title: String,
    payload: Value,
) -> Result<UpdateJobItemInput, String> {
    Ok(UpdateJobItemInput {
        item_type: item_type.to_string(),
        source,
        source_id,
        target_type,
        title,
        payload_json: serde_json::to_string(&payload).map_err(|e| e.to_string())?,
        status: status.to_string(),
    })
}

fn build_initial_items(
    state: &Arc<AppState>,
    request: &StartUpdateJobRequest,
) -> Result<Vec<UpdateJobItemInput>, String> {
    let mut items = Vec::new();
    let scope = request.scope.as_str();
    let include_work = matches!(scope, "all" | "work");
    let include_author = matches!(scope, "all" | "author");
    let include_series = matches!(scope, "all" | "series");
    let work_filter = request
        .work_ids
        .as_ref()
        .map(|ids| ids.iter().copied().collect::<HashSet<_>>());
    let target_filter = request
        .target_ids
        .as_ref()
        .map(|ids| ids.iter().copied().collect::<HashSet<_>>());

    if include_work {
        let works = if let Some(filter) = &work_filter {
            let mut entries = Vec::new();
            for id in filter {
                entries.push(state.db.get_download(*id)?);
            }
            entries
        } else {
            state.db.get_watched_downloads()?
        };
        for dl in works {
            if dl.source != "pixiv" && dl.source != "fanbox" {
                continue;
            }
            let title = dl.title.clone();
            let payload = serde_json::to_value(dl).map_err(|e| e.to_string())?;
            items.push(item_from_payload(
                "work",
                "queued",
                payload
                    .get("source")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                payload
                    .get("sourceId")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                Some("work".to_string()),
                title,
                payload,
            )?);
        }
    }

    if include_author || include_series {
        for target in state.db.list_update_targets(None, true)? {
            if target.target_type == "author" && !include_author {
                continue;
            }
            if target.target_type == "series" && !include_series {
                continue;
            }
            if target.target_type != "author" && target.target_type != "series" {
                continue;
            }
            if let Some(filter) = &target_filter {
                if !filter.contains(&target.id) {
                    continue;
                }
            }
            let title = target.display_name.clone();
            let payload = serde_json::to_value(target).map_err(|e| e.to_string())?;
            items.push(item_from_payload(
                "target",
                "queued",
                payload
                    .get("source")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                payload
                    .get("sourceKey")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                payload
                    .get("targetType")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                title,
                payload,
            )?);
        }
    }

    Ok(items)
}

async fn emit_snapshot(app: &tauri::AppHandle, state: &Arc<AppState>, job_id: &str) {
    if let Ok(snapshot) = state.db.update_job_snapshot(job_id) {
        let _ = app.emit("update-job-progress", snapshot);
    }
}

fn spawn_update_job(app: tauri::AppHandle, job_id: String, credentials: UpdateCredentials) {
    let inserted = active_jobs()
        .lock()
        .map(|mut jobs| jobs.insert(job_id.clone()))
        .unwrap_or(false);
    if !inserted {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let run_result = run_update_job(app.clone(), job_id.clone(), credentials).await;
        if let Err(error) = run_result {
            let state = app.state::<Arc<AppState>>().inner().clone();
            let _ = state.db.append_update_job_log(&job_id, "error", &error);
            let _ = state
                .db
                .set_update_job_status(&job_id, "failed", Some(&error));
            emit_snapshot(&app, &state, &job_id).await;
        }
        if let Ok(mut jobs) = active_jobs().lock() {
            jobs.remove(&job_id);
        }
    });
}

#[tauri::command]
pub async fn start_update_job(
    app: tauri::AppHandle,
    request: StartUpdateJobRequest,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let items = build_initial_items(&state, &request)?;
    if items.is_empty() {
        return Err("更新監視対象がありません".to_string());
    }
    let job_id = make_job_id();
    let snapshot = state.db.create_update_job(&job_id, &request, &items)?;
    spawn_update_job(
        app.clone(),
        job_id.clone(),
        request.credentials.clone().unwrap_or(UpdateCredentials {
            pixiv_refresh_token: None,
            fanbox_cookie: None,
            fanbox_user_agent: None,
        }),
    );
    emit_snapshot(&app, &state, &job_id).await;
    Ok(snapshot)
}

#[tauri::command]
pub async fn pause_update_job(
    app: tauri::AppHandle,
    job_id: String,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    state.db.prepare_update_job_resume(&job_id, false)?;
    state
        .db
        .set_update_job_status(&job_id, "paused", Some("一時停止しました"))?;
    state
        .db
        .append_update_job_log(&job_id, "warn", "更新ジョブを一時停止しました")?;
    emit_snapshot(&app, &state, &job_id).await;
    state.db.update_job_snapshot(&job_id)
}

#[tauri::command]
pub async fn resume_update_job(
    app: tauri::AppHandle,
    job_id: String,
    credentials: UpdateCredentials,
    retry_failed: Option<bool>,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    state
        .db
        .prepare_update_job_resume(&job_id, retry_failed.unwrap_or(false))?;
    state
        .db
        .set_update_job_status(&job_id, "queued", Some("再開待ち"))?;
    state
        .db
        .append_update_job_log(&job_id, "info", "更新ジョブを再開しました")?;
    spawn_update_job(app.clone(), job_id.clone(), credentials);
    emit_snapshot(&app, &state, &job_id).await;
    state.db.update_job_snapshot(&job_id)
}

#[tauri::command]
pub async fn cancel_update_job(
    app: tauri::AppHandle,
    job_id: String,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    state
        .db
        .set_update_job_status(&job_id, "canceling", Some("キャンセル中"))?;
    state
        .db
        .append_update_job_log(&job_id, "warn", "更新ジョブのキャンセルを要求しました")?;
    let credentials = UpdateCredentials {
        pixiv_refresh_token: None,
        fanbox_cookie: None,
        fanbox_user_agent: None,
    };
    spawn_update_job(app.clone(), job_id.clone(), credentials);
    emit_snapshot(&app, &state, &job_id).await;
    state.db.update_job_snapshot(&job_id)
}

#[tauri::command]
pub async fn get_update_job(
    app: tauri::AppHandle,
    job_id: String,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.update_job_snapshot(&job_id)
}

#[tauri::command]
pub async fn list_update_jobs(app: tauri::AppHandle) -> Result<Vec<UpdateJobSummary>, String> {
    let state = app.state::<Arc<AppState>>();
    state.db.list_update_jobs()
}

#[tauri::command]
pub async fn save_update_job_candidates(
    app: tauri::AppHandle,
    job_id: String,
    candidate_ids: Vec<i64>,
    credentials: Option<UpdateCredentials>,
) -> Result<UpdateJobSnapshot, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let changed = state
        .db
        .queue_update_job_candidates(&job_id, &candidate_ids)?;
    if changed > 0 {
        state.db.append_update_job_log(
            &job_id,
            "info",
            &format!("{}件の候補を保存キューに追加しました", changed),
        )?;
        state
            .db
            .set_update_job_status(&job_id, "queued", Some("候補保存待ち"))?;
        spawn_update_job(
            app.clone(),
            job_id.clone(),
            credentials.unwrap_or_else(snapshot_credentials_missing),
        );
    }
    emit_snapshot(&app, &state, &job_id).await;
    state.db.update_job_snapshot(&job_id)
}

fn snapshot_credentials_missing() -> UpdateCredentials {
    UpdateCredentials {
        pixiv_refresh_token: None,
        fanbox_cookie: None,
        fanbox_user_agent: None,
    }
}

#[tauri::command]
pub async fn clear_update_job(app: tauri::AppHandle, job_id: String) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>();
    state.db.clear_update_job(&job_id)
}

async fn run_update_job(
    app: tauri::AppHandle,
    job_id: String,
    credentials: UpdateCredentials,
) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    state
        .db
        .set_update_job_status(&job_id, "running", Some("更新チェックを開始しています"))?;
    emit_snapshot(&app, &state, &job_id).await;

    loop {
        let status = state.db.update_job_status_value(&job_id)?;
        if status == "paused"
            || status == "auth_required"
            || status == "completed"
            || status == "failed"
            || status == "canceled"
        {
            break;
        }
        if status == "canceling" {
            state
                .db
                .set_update_job_status(&job_id, "canceled", Some("キャンセルしました"))?;
            state
                .db
                .append_update_job_log(&job_id, "warn", "更新ジョブをキャンセルしました")?;
            emit_snapshot(&app, &state, &job_id).await;
            break;
        }

        let Some(item) = state.db.next_update_job_item(&job_id)? else {
            let snapshot = state.db.update_job_snapshot(&job_id)?;
            let final_status = if snapshot.error_count > 0 {
                "failed"
            } else {
                "completed"
            };
            let label = if final_status == "failed" {
                "エラーありで終了しました"
            } else {
                "完了しました"
            };
            state
                .db
                .set_update_job_status(&job_id, final_status, Some(label))?;
            state.db.append_update_job_log(
                &job_id,
                if final_status == "failed" {
                    "error"
                } else {
                    "success"
                },
                label,
            )?;
            emit_snapshot(&app, &state, &job_id).await;
            break;
        };

        let outcome = process_update_job_item(&app, &state, &job_id, &item, &credentials).await;
        match outcome {
            Ok(ItemOutcome::Done(message)) => {
                state
                    .db
                    .complete_update_job_item(item.id, "done", None, None)?;
                state
                    .db
                    .append_update_job_log(&job_id, "success", &message)?;
            }
            Ok(ItemOutcome::Saved(download_id, message)) => {
                state
                    .db
                    .complete_update_job_item(item.id, "saved", None, Some(download_id))?;
                state
                    .db
                    .append_update_job_log(&job_id, "success", &message)?;
            }
            Ok(ItemOutcome::Skipped(message)) => {
                state
                    .db
                    .complete_update_job_item(item.id, "skipped", None, None)?;
                state.db.append_update_job_log(&job_id, "info", &message)?;
            }
            Ok(ItemOutcome::AuthRequired(message)) => {
                state
                    .db
                    .complete_update_job_item(item.id, "queued", Some(&message), None)?;
                state.db.append_update_job_log(&job_id, "warn", &message)?;
                state
                    .db
                    .set_update_job_status(&job_id, "auth_required", Some(&message))?;
                emit_snapshot(&app, &state, &job_id).await;
                break;
            }
            Err(error) => {
                state
                    .db
                    .complete_update_job_item(item.id, "failed", Some(&error), None)?;
                state.db.append_update_job_log(
                    &job_id,
                    "error",
                    &format!("{}: {}", item.title, error),
                )?;
            }
        }
        emit_snapshot(&app, &state, &job_id).await;
        tokio::time::sleep(std::time::Duration::from_millis(UPDATE_JOB_DELAY_MS)).await;
    }

    Ok(())
}

enum ItemOutcome {
    Done(String),
    Saved(i64, String),
    Skipped(String),
    AuthRequired(String),
}

async fn process_update_job_item(
    app: &tauri::AppHandle,
    state: &Arc<AppState>,
    job_id: &str,
    item: &UpdateJobItem,
    credentials: &UpdateCredentials,
) -> Result<ItemOutcome, String> {
    match item.item_type.as_str() {
        "work" => process_work_item(app, item, credentials).await,
        "target" => process_target_item(state, job_id, item, credentials).await,
        "candidate" => process_candidate_item(app, state, item, credentials).await,
        _ => Ok(ItemOutcome::Skipped(format!(
            "未対応のジョブ項目をスキップ: {}",
            item.item_type
        ))),
    }
}

async fn process_work_item(
    app: &tauri::AppHandle,
    item: &UpdateJobItem,
    credentials: &UpdateCredentials,
) -> Result<ItemOutcome, String> {
    let dl: DownloadEntry = serde_json::from_str(&item.payload_json).map_err(|e| e.to_string())?;
    if dl.source == "pixiv" {
        let Some(token) = pixiv_token(credentials) else {
            return Ok(ItemOutcome::AuthRequired("Pixiv連携が必要です".to_string()));
        };
        let metadata =
            super::downloader::fetch_pixiv_novel_metadata(dl.source_id.clone(), token.clone())
                .await?;
        let metadata_value = serde_json::to_value(&metadata).map_err(|e| e.to_string())?;
        let metadata_updated_at = string_at(&metadata_value, &[&["create_date"], &["createDate"]]);
        if dl.source_updated_at == metadata_updated_at && dl.source_updated_at.is_some() {
            return Ok(ItemOutcome::Skipped(format!("最新: {}", dl.title)));
        }
        let data = super::downloader::fetch_pixiv_novel(dl.source_id.clone(), token).await?;
        let value = serde_json::to_value(&data).map_err(|e| e.to_string())?;
        let title =
            string_at(&value, &[&["detail", "title"], &["title"]]).unwrap_or(dl.title.clone());
        let author_name = string_at(&value, &[&["detail", "user", "name"], &["user", "name"]])
            .unwrap_or(dl.author_name.clone());
        let author_id = string_at(&value, &[&["detail", "user", "id"], &["user", "id"]])
            .unwrap_or(dl.author_id.clone());
        let source_created_at = string_at(
            &value,
            &[
                &["detail", "create_date"],
                &["detail", "createDate"],
                &["create_date"],
                &["createDate"],
            ],
        );
        let excerpt = string_at(&value, &[&["detail", "caption"], &["caption"]]);
        let updated = super::downloader::download_and_save(
            app.clone(),
            value,
            "pixiv".to_string(),
            dl.source_id.clone(),
            title,
            author_name,
            author_id,
            "novel".to_string(),
            Some(pixiv_tags(
                &serde_json::to_value(&data).map_err(|e| e.to_string())?,
            )),
            excerpt,
            source_created_at,
            None,
            None,
        )
        .await?;
        if updated.current_version > dl.current_version {
            Ok(ItemOutcome::Saved(
                updated.id,
                format!("更新を保存: {}", updated.title),
            ))
        } else {
            Ok(ItemOutcome::Skipped(format!("最新: {}", updated.title)))
        }
    } else if dl.source == "fanbox" {
        let Some(cookie) = fanbox_cookie(credentials) else {
            return Ok(ItemOutcome::AuthRequired(
                "FANBOX連携が必要です".to_string(),
            ));
        };
        let user_agent = fanbox_user_agent(credentials);
        let post = super::downloader::fetch_fanbox_post(
            dl.source_id.clone(),
            cookie.clone(),
            user_agent.clone(),
        )
        .await?;
        let value = serde_json::to_value(&post).map_err(|e| e.to_string())?;
        let updated_at = string_at(&value, &[&["updatedDatetime"], &["updated_datetime"]]);
        if dl.source_updated_at == updated_at && dl.source_updated_at.is_some() {
            return Ok(ItemOutcome::Skipped(format!("最新: {}", dl.title)));
        }
        let title = string_at(&value, &[&["title"]]).unwrap_or(dl.title.clone());
        let author_name = string_at(&value, &[&["user", "name"]]).unwrap_or(dl.author_name.clone());
        let author_id = string_at(
            &value,
            &[&["creatorId"], &["creator_id"], &["user", "userId"]],
        )
        .unwrap_or(dl.author_id.clone());
        let content_type = string_at(&value, &[&["type"], &["postType"], &["post_type"]])
            .unwrap_or_else(|| "article".to_string());
        let tags = value
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let updated = super::downloader::download_and_save(
            app.clone(),
            value,
            "fanbox".to_string(),
            dl.source_id.clone(),
            title,
            author_name,
            author_id,
            content_type,
            Some(tags),
            string_at(
                &serde_json::to_value(&post).map_err(|e| e.to_string())?,
                &[&["excerpt"], &["body", "excerpt"]],
            ),
            string_at(
                &serde_json::to_value(&post).map_err(|e| e.to_string())?,
                &[&["publishedDatetime"], &["published_datetime"]],
            ),
            Some(cookie),
            Some(user_agent),
        )
        .await?;
        if updated.current_version > dl.current_version {
            Ok(ItemOutcome::Saved(
                updated.id,
                format!("更新を保存: {}", updated.title),
            ))
        } else {
            Ok(ItemOutcome::Skipped(format!("最新: {}", updated.title)))
        }
    } else {
        Ok(ItemOutcome::Skipped(format!(
            "未対応ソースをスキップ: {}",
            dl.source
        )))
    }
}

async fn process_target_item(
    state: &Arc<AppState>,
    job_id: &str,
    item: &UpdateJobItem,
    credentials: &UpdateCredentials,
) -> Result<ItemOutcome, String> {
    let target: crate::database::UpdateTarget =
        serde_json::from_str(&item.payload_json).map_err(|e| e.to_string())?;
    let snapshot = state.db.update_job_snapshot(job_id)?;
    let auto_save = snapshot.mode == "auto_save";

    let items: Vec<Value> = if target.source == "pixiv" && target.target_type == "author" {
        let Some(token) = pixiv_token(credentials) else {
            return Ok(ItemOutcome::AuthRequired("Pixiv連携が必要です".to_string()));
        };
        super::downloader::fetch_pixiv_user_novels(target.source_key.clone(), token)
            .await?
            .into_iter()
            .map(|item| serde_json::to_value(item).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?
    } else if target.source == "pixiv" && target.target_type == "series" {
        let Some(token) = pixiv_token(credentials) else {
            return Ok(ItemOutcome::AuthRequired("Pixiv連携が必要です".to_string()));
        };
        super::downloader::fetch_pixiv_series_novels(target.source_key.clone(), token)
            .await?
            .into_iter()
            .map(|item| serde_json::to_value(item).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?
    } else if target.source == "fanbox" && target.target_type == "author" {
        let Some(cookie) = fanbox_cookie(credentials) else {
            return Ok(ItemOutcome::AuthRequired(
                "FANBOX連携が必要です".to_string(),
            ));
        };
        let user_agent = fanbox_user_agent(credentials);
        super::downloader::fetch_fanbox_creator_posts(target.source_key.clone(), cookie, user_agent)
            .await?
            .into_iter()
            .map(|item| serde_json::to_value(item).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        Vec::new()
    };

    let mut found = 0i64;
    for source_item in &items {
        let Some(payload) = candidate_payload(
            &target.target_type,
            &target.display_name,
            &target.source,
            source_item,
        ) else {
            continue;
        };
        let source = payload
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or(&target.source)
            .to_string();
        let source_id = payload
            .get("sourceId")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if source_id.is_empty()
            || state
                .db
                .get_download_by_source(&source, &source_id)?
                .is_some()
        {
            continue;
        }
        let title = payload
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("無題")
            .to_string();
        state.db.insert_update_job_candidate(
            job_id,
            &item_from_payload(
                "candidate",
                if auto_save { "queued" } else { "candidate" },
                Some(source),
                Some(source_id),
                Some(target.target_type.clone()),
                title,
                payload,
            )?,
        )?;
        found += 1;
    }

    let first = items.first();
    let first_id = first.and_then(|value| {
        if target.source == "pixiv" {
            normalize_pixiv_novel_id(value)
        } else {
            string_at(value, &[&["id"]])
        }
    });
    let first_updated = first.and_then(|value| {
        if target.source == "pixiv" {
            string_at(value, &[&["create_date"], &["createDate"]])
        } else {
            string_at(value, &[&["updatedDatetime"], &["updated_datetime"]])
        }
    });
    state.db.mark_update_target_checked(
        &target.target_type,
        &target.source,
        &target.source_key,
        first_id.as_deref(),
        first_updated.as_deref(),
    )?;

    Ok(ItemOutcome::Done(format!(
        "{}: 新作候補 {} 件",
        target.display_name, found
    )))
}

async fn process_candidate_item(
    app: &tauri::AppHandle,
    state: &Arc<AppState>,
    item: &UpdateJobItem,
    credentials: &UpdateCredentials,
) -> Result<ItemOutcome, String> {
    let payload: Value = serde_json::from_str(&item.payload_json).map_err(|e| e.to_string())?;
    let source = item.source.as_deref().unwrap_or("");
    let source_id = item.source_id.as_deref().unwrap_or("");
    if state
        .db
        .get_download_by_source(source, source_id)?
        .is_some()
    {
        return Ok(ItemOutcome::Skipped(format!(
            "保存済みのためスキップ: {}",
            item.title
        )));
    }

    if source == "pixiv" {
        let Some(token) = pixiv_token(credentials) else {
            return Ok(ItemOutcome::AuthRequired("Pixiv連携が必要です".to_string()));
        };
        let data = super::downloader::fetch_pixiv_novel(source_id.to_string(), token).await?;
        let value = serde_json::to_value(&data).map_err(|e| e.to_string())?;
        let title =
            string_at(&value, &[&["detail", "title"], &["title"]]).unwrap_or(item.title.clone());
        let author_name = string_at(&value, &[&["detail", "user", "name"], &["user", "name"]])
            .or_else(|| string_at(&payload, &[&["originalData", "user", "name"]]))
            .unwrap_or_else(|| "unknown".to_string());
        let author_id = string_at(&value, &[&["detail", "user", "id"], &["user", "id"]])
            .or_else(|| string_at(&payload, &[&["originalData", "user", "id"]]))
            .unwrap_or_else(|| "0".to_string());
        let source_created_at = string_at(
            &value,
            &[
                &["detail", "create_date"],
                &["detail", "createDate"],
                &["create_date"],
                &["createDate"],
            ],
        );
        let excerpt = string_at(&value, &[&["detail", "caption"], &["caption"]]);
        let tags = pixiv_tags(&value);
        let updated = super::downloader::download_and_save(
            app.clone(),
            value.clone(),
            "pixiv".to_string(),
            source_id.to_string(),
            title,
            author_name,
            author_id,
            "novel".to_string(),
            Some(tags),
            excerpt,
            source_created_at,
            None,
            None,
        )
        .await?;
        if let Some((series_id, series_title)) = normalize_series(&payload["originalData"]) {
            let _ = state.db.upsert_update_target(&UpdateTargetInput {
                target_type: "series".to_string(),
                source: "pixiv".to_string(),
                source_key: series_id,
                display_name: series_title,
                enabled: true,
                metadata_json: None,
            });
        }
        Ok(ItemOutcome::Saved(
            updated.id,
            format!("新作を保存: {}", updated.title),
        ))
    } else if source == "fanbox" {
        let Some(cookie) = fanbox_cookie(credentials) else {
            return Ok(ItemOutcome::AuthRequired(
                "FANBOX連携が必要です".to_string(),
            ));
        };
        let user_agent = fanbox_user_agent(credentials);
        let post = super::downloader::fetch_fanbox_post(
            source_id.to_string(),
            cookie.clone(),
            user_agent.clone(),
        )
        .await?;
        let value = serde_json::to_value(&post).map_err(|e| e.to_string())?;
        let title = string_at(&value, &[&["title"]]).unwrap_or(item.title.clone());
        let author_name = string_at(&value, &[&["user", "name"]])
            .or_else(|| string_at(&payload, &[&["originalData", "user", "name"]]))
            .unwrap_or_else(|| "unknown".to_string());
        let author_id = string_at(
            &value,
            &[&["creatorId"], &["creator_id"], &["user", "userId"]],
        )
        .or_else(|| {
            string_at(
                &payload,
                &[
                    &["originalData", "creatorId"],
                    &["originalData", "creator_id"],
                ],
            )
        })
        .unwrap_or_else(|| "0".to_string());
        let tags = value
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let updated = super::downloader::download_and_save(
            app.clone(),
            value.clone(),
            "fanbox".to_string(),
            source_id.to_string(),
            title,
            author_name,
            author_id,
            string_at(&value, &[&["type"], &["postType"], &["post_type"]])
                .unwrap_or_else(|| "article".to_string()),
            Some(tags),
            string_at(&value, &[&["excerpt"], &["body", "excerpt"]]),
            string_at(&value, &[&["publishedDatetime"], &["published_datetime"]]),
            Some(cookie),
            Some(user_agent),
        )
        .await?;
        Ok(ItemOutcome::Saved(
            updated.id,
            format!("新作を保存: {}", updated.title),
        ))
    } else {
        Ok(ItemOutcome::Skipped(format!(
            "未対応ソースをスキップ: {}",
            source
        )))
    }
}
