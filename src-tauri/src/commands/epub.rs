//! EPUB エクスポート用 Tauri コマンド（進捗イベント付き）。

use crate::epub::builder::{sanitize_epub_filename, EpubBuilder};
use crate::epub::converter;
use crate::epub::intermediate::*;
use crate::epub::template::{template_file_purpose, EpubRenderer, TemplateManager, TEMPLATE_FILES};
use crate::epub::validate;
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{Emitter, Manager};

/// 書き出しごとに集める検査結果の上限。何百冊でも通知が壊れないようにする。
const MAX_REPORTED_ISSUES: usize = 50;

fn get_templates_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {}", e))?;
    Ok(app_data.join("templates"))
}

fn get_template_manager(app: &tauri::AppHandle) -> Result<TemplateManager, String> {
    let dir = get_templates_dir(app)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let manager = TemplateManager::new(dir);
    manager.initialize_defaults()?;
    Ok(manager)
}

fn emit_progress(app: &tauri::AppHandle, progress: &ExportProgress) {
    let _ = app.emit("epub-export-progress", progress);
}

fn apply_active_edit_to_epub_data(
    state: &Arc<AppState>,
    download_id: i64,
    source: &str,
    data: &mut serde_json::Value,
) {
    let Ok(reader) = state.db.get_reader_document(download_id, None) else {
        return;
    };
    if !reader.is_edited || reader.plain_text.trim().is_empty() {
        return;
    }

    if source == "pixiv" {
        data["text"] = serde_json::Value::String(reader.plain_text.clone());
        if let Some(detail) = data
            .get_mut("detail")
            .and_then(|value| value.as_object_mut())
        {
            detail.insert(
                "text".to_string(),
                serde_json::Value::String(reader.plain_text.clone()),
            );
        }
    } else if source == "fanbox" {
        let blocks = reader
            .plain_text
            .split("\n\n")
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| {
                serde_json::json!({
                    "type": "p",
                    "text": text,
                })
            })
            .collect::<Vec<_>>();
        if let Some(body) = data.get_mut("body").and_then(|value| value.as_object_mut()) {
            body.insert("blocks".to_string(), serde_json::Value::Array(blocks));
            body.insert(
                "text".to_string(),
                serde_json::Value::String(reader.plain_text),
            );
        }
    }
}

/// 保存済みの JSON を読んで中間形式に変換する。書き出しとプレビューの共通経路。
fn load_manifest(
    state: &Arc<AppState>,
    download_id: i64,
) -> Result<(EpubManifest, String, String), String> {
    let dl = state.db.get_download(download_id)?;
    let target_json_path = dl
        .original_json_path
        .clone()
        .filter(|path| Path::new(path).exists())
        .unwrap_or_else(|| dl.json_path.clone());

    let json_content = std::fs::read_to_string(&target_json_path)
        .map_err(|e| format!("JSONの読み込みに失敗: {}", e))?;
    let mut data: serde_json::Value =
        serde_json::from_str(&json_content).map_err(|e| format!("JSON パースエラー: {}", e))?;
    apply_active_edit_to_epub_data(state, download_id, &dl.source, &mut data);

    let assets_dir = Path::new(&target_json_path)
        .parent()
        .ok_or_else(|| "JSONパスに親ディレクトリがありません".to_string())?
        .join("data_assets");

    crate::downloader::asset_downloader::inject_local_paths(
        &mut data,
        "data_assets",
        dl.source == "fanbox",
    );

    let manifest = converter::convert_to_manifest(&data, &dl.source, &assets_dir)?;
    Ok((manifest, dl.source, dl.title))
}

fn resolve_template(manager: &TemplateManager, template_name: &str, source: &str) -> String {
    if template_name == "__auto__" {
        manager.resolve_for_source(source)
    } else {
        template_name.to_string()
    }
}

/// 一括書き出し用の衝突しないファイル名。
///
/// タイトルだけでは、別作者の同名作品や同一シリーズの同名話が互いを上書き
/// してしまう。配信元と作品 ID を末尾に残しつつ、Windows でも扱える 120 文字
/// 以内の名前にする。
fn batch_output_filename(
    title: &str,
    source: &str,
    manifest: &EpubManifest,
    download_id: i64,
) -> String {
    let provider_id = manifest
        .provider
        .novel_id
        .as_deref()
        .or(manifest.provider.post_id.as_deref())
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| download_id.to_string());

    let clean_stem = |value: &str| {
        sanitize_epub_filename(value)
            .strip_suffix(".epub")
            .unwrap_or("epub_export")
            .to_string()
    };
    let source = clean_stem(source);
    let provider_id = clean_stem(&provider_id);
    let suffix = format!("[{}-{}]", source, provider_id);
    let title = clean_stem(title);
    let title_limit = 120usize.saturating_sub(suffix.chars().count() + 1);
    let title: String = title.chars().take(title_limit.max(1)).collect();

    format!("{} {}.epub", title, suffix)
}

/// 出来上がった EPUB を開き直して検査し、問題を積む。
fn validate_and_collect(
    path: &Path,
    reported_path: &Path,
    issues: &mut Vec<EpubValidationIssue>,
) -> bool {
    match validate::validate_epub(path) {
        Ok(report) => {
            let name = reported_path
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_default();
            for issue in report.issues {
                if issue.severity == "error" {
                    log::warn!("EPUB 検証 [{}] {}: {}", name, issue.code, issue.message);
                }
                if issues.len() < MAX_REPORTED_ISSUES {
                    issues.push(EpubValidationIssue {
                        location: format!("{} › {}", name, issue.location),
                        ..issue
                    });
                }
            }
            report.valid
        }
        Err(error) => {
            log::warn!("EPUB を検証できませんでした: {}", error);
            if issues.len() < MAX_REPORTED_ISSUES {
                issues.push(EpubValidationIssue::error(
                    "EPUB-VALIDATION-FAILED",
                    reported_path.to_string_lossy().as_ref(),
                    error,
                ));
            }
            false
        }
    }
}

/// A same-directory staging file. Dropping it after any build/validation error
/// removes the partial EPUB and leaves an existing destination untouched.
struct StagedEpub {
    path: PathBuf,
    published: bool,
}

impl StagedEpub {
    fn create(destination: &Path) -> Result<Self, String> {
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let name = destination
            .file_name()
            .ok_or_else(|| "出力ファイル名がありません".to_string())?
            .to_string_lossy();
        for _ in 0..32 {
            let candidate = parent.join(format!(
                ".{name}.piep-{}-{:016x}.tmp",
                std::process::id(),
                rand::random::<u64>()
            ));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&candidate)
            {
                Ok(_) => {
                    return Ok(Self {
                        path: candidate,
                        published: false,
                    })
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("一時EPUBを作成できません: {error}")),
            }
        }
        Err("一時EPUBの名前を確保できません".to_string())
    }

    fn publish(mut self, destination: &Path) -> Result<(), String> {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.path)
            .and_then(|file| file.sync_all())
            .map_err(|error| format!("一時EPUBを同期できません: {error}"))?;
        atomic_replace_file(&self.path, destination)?;
        self.published = true;
        sync_output_parent(destination)
    }
}

impl Drop for StagedEpub {
    fn drop(&mut self) {
        if !self.published {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn build_validate_and_publish(
    destination: &Path,
    build: impl FnOnce(&Path) -> Result<(), String>,
    validate: impl FnOnce(&Path) -> bool,
) -> Result<bool, String> {
    let staged = StagedEpub::create(destination)?;
    build(&staged.path)?;
    if !validate(&staged.path) {
        return Ok(false);
    }
    staged.publish(destination)?;
    Ok(true)
}

#[cfg(windows)]
fn atomic_replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, ReplaceFileW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        REPLACEFILE_IGNORE_MERGE_ERRORS,
    };

    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        if destination.exists() {
            ReplaceFileW(
                PCWSTR(destination_wide.as_ptr()),
                PCWSTR(source_wide.as_ptr()),
                PCWSTR::null(),
                REPLACEFILE_IGNORE_MERGE_ERRORS,
                None,
                None,
            )
        } else {
            MoveFileExW(
                PCWSTR(source_wide.as_ptr()),
                PCWSTR(destination_wide.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
    };
    result.map_err(|error| {
        format!(
            "EPUBを原子的に置き換えられません ({}): {error}",
            destination.display()
        )
    })
}

#[cfg(not(windows))]
fn atomic_replace_file(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::rename(source, destination).map_err(|error| {
        format!(
            "EPUBを原子的に置き換えられません ({}): {error}",
            destination.display()
        )
    })
}

#[cfg(unix)]
fn sync_output_parent(destination: &Path) -> Result<(), String> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("EPUB出力先を同期できません: {error}"))
}

#[cfg(not(unix))]
fn sync_output_parent(_destination: &Path) -> Result<(), String> {
    Ok(())
}

fn deduplicate_download_ids(download_ids: Vec<i64>) -> Vec<i64> {
    let mut seen = HashSet::with_capacity(download_ids.len());
    download_ids
        .into_iter()
        .filter(|download_id| seen.insert(*download_id))
        .collect()
}

// ============================================================
// 単体エクスポート
// ============================================================

#[tauri::command]
pub async fn export_epub(
    app: tauri::AppHandle,
    download_id: i64,
    template_name: String,
    output_path: String,
    compress_options: Option<ImageCompressOptions>,
) -> Result<String, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let tm = get_template_manager(&app)?;
    let compress = compress_options.unwrap_or_default();

    let (manifest, source, title) = load_manifest(&state, download_id)?;
    emit_progress(
        &app,
        &ExportProgress {
            phase: "started".into(),
            current_title: title.clone(),
            current_index: 1,
            total_count: 1,
            message: format!("「{}」のエクスポートを開始", title),
        },
    );

    let resolved_template = resolve_template(&tm, &template_name, &source);
    let template_contents = tm.load_template_contents(&resolved_template)?;
    let settings = tm.read_settings(&resolved_template);
    log::info!(
        "EPUB export: template '{}' resolved and loaded ({} files)",
        resolved_template,
        template_contents.len()
    );

    emit_progress(
        &app,
        &ExportProgress {
            phase: "building".into(),
            current_title: title.clone(),
            current_index: 1,
            total_count: 1,
            message: format!("「{}」のEPUBを生成中...", title),
        },
    );

    let out_path = PathBuf::from(&output_path);
    let app_clone = app.clone();
    let title_clone = title.clone();

    tokio::task::spawn_blocking(move || {
        let builder = EpubBuilder::new(manifest, template_contents, settings, compress);
        let mut issues = Vec::new();
        let result = build_validate_and_publish(
            &out_path,
            |staged_path| builder.build(staged_path),
            |staged_path| validate_and_collect(staged_path, &out_path, &mut issues),
        )
        .and_then(|valid| {
            if valid {
                return Ok(());
            }
            let detail = issues
                .iter()
                .take(3)
                .map(|issue| format!("{}: {}", issue.code, issue.message))
                .collect::<Vec<_>>()
                .join(" / ");
            Err(format!(
                "生成後のEPUB検証を通過しませんでした{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            ))
        });
        if let Err(error) = &result {
            log::error!("EPUB build failed: {}", error);
            emit_progress(
                &app_clone,
                &ExportProgress {
                    phase: "failed".into(),
                    current_title: title_clone.clone(),
                    current_index: 1,
                    total_count: 1,
                    message: format!("「{}」の生成に失敗: {}", title_clone, error),
                },
            );
        }
        result
    })
    .await
    .map_err(|e| format!("スレッドパニック: {}", e))??;

    emit_progress(
        &app,
        &ExportProgress {
            phase: "completed".into(),
            current_title: title.clone(),
            current_index: 1,
            total_count: 1,
            message: format!("「{}」のEPUBエクスポートが完了しました！", title),
        },
    );

    Ok(output_path)
}

// ============================================================
// バッチエクスポート
// ============================================================

/// Image conversion is CPU- and memory-heavy. Two workers keep the async
/// runtime responsive without multiplying the per-book image quota unchecked.
const MAX_CONCURRENT_EPUB_BUILDS: usize = 2;

enum BatchItemStatus {
    Published(String),
    Invalid,
    Failed(String),
}

struct BatchItemResult {
    index: u32,
    download_id: i64,
    status: BatchItemStatus,
    issues: Vec<EpubValidationIssue>,
}

fn aggregate_batch_results(mut completed: Vec<BatchItemResult>) -> ExportBatchResult {
    // Completion order is intentionally concurrent; result arrays retain the
    // caller's order so retry queues and file lists remain deterministic.
    completed.sort_by_key(|item| item.index);
    let mut result = ExportBatchResult {
        success_count: 0,
        failed_count: 0,
        failed_ids: Vec::new(),
        invalid_ids: Vec::new(),
        output_files: Vec::new(),
        invalid_count: 0,
        issues: Vec::new(),
    };
    for item in completed {
        for issue in item.issues {
            if result.issues.len() >= MAX_REPORTED_ISSUES {
                break;
            }
            result.issues.push(issue);
        }
        match item.status {
            BatchItemStatus::Published(path) => {
                result.success_count += 1;
                result.output_files.push(path);
            }
            BatchItemStatus::Invalid => {
                // Invalid is a failure (and a more specific subset of it), not
                // a successful output with a warning. No file was published.
                result.failed_count += 1;
                result.invalid_count += 1;
                result.invalid_ids.push(item.download_id);
            }
            BatchItemStatus::Failed(error) => {
                log::debug!("EPUB ID {} failed: {}", item.download_id, error);
                result.failed_count += 1;
                result.failed_ids.push(item.download_id);
            }
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn export_batch_item(
    app: tauri::AppHandle,
    state: Arc<AppState>,
    templates_dir: PathBuf,
    download_id: i64,
    index: u32,
    total: u32,
    template_name: String,
    output_base: PathBuf,
    compress: ImageCompressOptions,
) -> BatchItemResult {
    let title = state
        .db
        .get_download(download_id)
        .map(|download| download.title)
        .unwrap_or_else(|_| format!("ID {download_id}"));
    emit_progress(
        &app,
        &ExportProgress {
            phase: "converting".into(),
            current_title: title.clone(),
            current_index: index,
            total_count: total,
            message: format!("[{index}/{total}] 「{title}」を変換中..."),
        },
    );

    let mut issues = Vec::new();
    let result = (|| {
        let (manifest, source, loaded_title) = load_manifest(&state, download_id)?;
        let manager = TemplateManager::new(templates_dir);
        let resolved = resolve_template(&manager, &template_name, &source);
        let contents = manager.load_template_contents(&resolved)?;
        let settings = manager.read_settings(&resolved);
        let filename = batch_output_filename(&loaded_title, &source, &manifest, download_id);
        let destination = output_base.join(filename);
        let builder = EpubBuilder::new(manifest, contents, settings, compress);
        let valid = build_validate_and_publish(
            &destination,
            |staged_path| builder.build(staged_path),
            |staged_path| validate_and_collect(staged_path, &destination, &mut issues),
        )?;
        Ok::<_, String>((destination, valid))
    })();

    let status = match result {
        Ok((destination, true)) => {
            emit_progress(
                &app,
                &ExportProgress {
                    phase: "completed".into(),
                    current_title: title.clone(),
                    current_index: index,
                    total_count: total,
                    message: format!("[{index}/{total}] 「{title}」完了"),
                },
            );
            BatchItemStatus::Published(destination.to_string_lossy().to_string())
        }
        Ok((_destination, false)) => {
            emit_progress(
                &app,
                &ExportProgress {
                    phase: "failed".into(),
                    current_title: title.clone(),
                    current_index: index,
                    total_count: total,
                    message: format!("[{index}/{total}] 「{title}」失敗: EPUB検証エラー"),
                },
            );
            BatchItemStatus::Invalid
        }
        Err(error) => {
            log::error!("EPUB export error (ID {}): {}", download_id, error);
            emit_progress(
                &app,
                &ExportProgress {
                    phase: "failed".into(),
                    current_title: title.clone(),
                    current_index: index,
                    total_count: total,
                    message: format!("[{index}/{total}] 「{title}」失敗: {error}"),
                },
            );
            BatchItemStatus::Failed(error)
        }
    };

    BatchItemResult {
        index,
        download_id,
        status,
        issues,
    }
}

#[tauri::command]
pub async fn export_epub_batch(
    app: tauri::AppHandle,
    download_ids: Vec<i64>,
    template_name: String,
    output_dir: String,
    compress_options: Option<ImageCompressOptions>,
) -> Result<ExportBatchResult, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let templates_dir = get_templates_dir(&app)?;
    TemplateManager::new(templates_dir.clone()).initialize_defaults()?;
    let compress = compress_options.unwrap_or_default();
    let output_base = PathBuf::from(&output_dir);
    std::fs::create_dir_all(&output_base).map_err(|e| e.to_string())?;

    let download_ids = deduplicate_download_ids(download_ids);
    let total = download_ids.len() as u32;

    emit_progress(
        &app,
        &ExportProgress {
            phase: "started".into(),
            current_title: String::new(),
            current_index: 0,
            total_count: total,
            message: format!("{}件のEPUBエクスポートを開始", total),
        },
    );

    let mut tasks = tokio::task::JoinSet::new();
    let mut pending = download_ids.into_iter().enumerate();
    let mut completed = Vec::with_capacity(total as usize);
    loop {
        while tasks.len() < MAX_CONCURRENT_EPUB_BUILDS {
            let Some((index, download_id)) = pending.next() else {
                break;
            };
            let app = app.clone();
            let state = state.clone();
            let templates_dir = templates_dir.clone();
            let template_name = template_name.clone();
            let output_base = output_base.clone();
            let compress = compress.clone();
            tasks.spawn_blocking(move || {
                export_batch_item(
                    app,
                    state,
                    templates_dir,
                    download_id,
                    (index + 1) as u32,
                    total,
                    template_name,
                    output_base,
                    compress,
                )
            });
        }
        if tasks.is_empty() {
            break;
        }
        let joined = tasks
            .join_next()
            .await
            .ok_or_else(|| "EPUBワーカーが予期せず終了しました".to_string())?
            .map_err(|error| format!("EPUBワーカーがパニックしました: {error}"))?;
        completed.push(joined);
    }

    let result = aggregate_batch_results(completed);

    emit_progress(
        &app,
        &ExportProgress {
            phase: "completed".into(),
            current_title: String::new(),
            current_index: total,
            total_count: total,
            message: format!(
                "エクスポート完了: {}件成功, {}件失敗",
                result.success_count, result.failed_count
            ),
        },
    );

    Ok(result)
}

// ============================================================
// テンプレート管理コマンド
// ============================================================

#[tauri::command]
pub async fn list_epub_templates(app: tauri::AppHandle) -> Result<Vec<TemplateInfo>, String> {
    get_template_manager(&app)?.list_templates()
}

#[tauri::command]
pub async fn get_template_files(
    app: tauri::AppHandle,
    template_name: String,
) -> Result<Vec<TemplateFile>, String> {
    get_template_manager(&app)?.get_template_files(&template_name)
}

#[tauri::command]
pub async fn read_template_file(
    app: tauri::AppHandle,
    template_name: String,
    filename: String,
) -> Result<String, String> {
    get_template_manager(&app)?.read_template_file(&template_name, &filename)
}

#[tauri::command]
pub async fn save_template_file(
    app: tauri::AppHandle,
    template_name: String,
    filename: String,
    content: String,
) -> Result<(), String> {
    get_template_manager(&app)?.save_template_file(&template_name, &filename, &content)
}

#[tauri::command]
pub async fn reset_template_file(
    app: tauri::AppHandle,
    template_name: String,
    filename: String,
) -> Result<String, String> {
    get_template_manager(&app)?.reset_template_file(&template_name, &filename)
}

#[tauri::command]
pub async fn create_epub_template(
    app: tauri::AppHandle,
    template_name: String,
    base_template: Option<String>,
) -> Result<(), String> {
    get_template_manager(&app)?.create_template(
        &template_name,
        base_template.as_deref().unwrap_or("default"),
    )
}

#[tauri::command]
pub async fn rename_epub_template(
    app: tauri::AppHandle,
    template_name: String,
    next_name: String,
) -> Result<(), String> {
    get_template_manager(&app)?.rename_template(&template_name, &next_name)
}

#[tauri::command]
pub async fn delete_epub_template(
    app: tauri::AppHandle,
    template_name: String,
) -> Result<(), String> {
    get_template_manager(&app)?.delete_template(&template_name)
}

#[tauri::command]
pub async fn get_template_settings(
    app: tauri::AppHandle,
    template_name: String,
) -> Result<TemplateSettings, String> {
    Ok(get_template_manager(&app)?.read_settings(&template_name))
}

#[tauri::command]
pub async fn save_template_settings(
    app: tauri::AppHandle,
    template_name: String,
    settings: TemplateSettings,
) -> Result<TemplateSettings, String> {
    get_template_manager(&app)?.save_settings(&template_name, settings)
}

/// 編集画面が出す、ファイルごとの役割の一覧。
#[tauri::command]
pub async fn list_template_file_kinds() -> Result<Vec<TemplateFileKind>, String> {
    Ok(TEMPLATE_FILES
        .iter()
        .map(|filename| TemplateFileKind {
            filename: filename.to_string(),
            purpose: template_file_purpose(filename).to_string(),
            language: if filename.ends_with(".css.j2") {
                "css".into()
            } else {
                "xml".into()
            },
        })
        .collect())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateFileKind {
    pub filename: String,
    pub purpose: String,
    /// 編集画面の色分け用。
    pub language: String,
}

// ============================================================
// プレビューとデータ辞書
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplatePreview {
    /// プレビューに使った作品。
    pub sample_title: String,
    pub sample_source: String,
    pub sample_download_id: Option<i64>,
    pub css: String,
    pub cover: Option<String>,
    pub info: Option<String>,
    pub page: Option<String>,
    pub nav: String,
    pub opf: String,
    pub ncx: Option<String>,
    /// 描画した文書を整形式かどうか検査した結果。
    pub issues: Vec<EpubValidationIssue>,
    pub fields: Vec<DataField>,
}

/// テンプレートから差し込める値ひとつ。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataField {
    /// テンプレートに書く式 (`core.name` など)。
    pub path: String,
    pub group: String,
    pub label: String,
    pub sample: String,
    /// この作品で値が入っているか。
    pub available: bool,
}

#[tauri::command]
pub async fn preview_epub_template(
    app: tauri::AppHandle,
    template_name: String,
    download_id: Option<i64>,
) -> Result<TemplatePreview, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let tm = get_template_manager(&app)?;

    let (manifest, source, title, resolved_id) = match download_id {
        Some(id) => {
            let (manifest, source, title) = load_manifest(&state, id)?;
            (manifest, source, title, Some(id))
        }
        None => {
            let manifest = sample_manifest();
            (
                manifest,
                "sample".to_string(),
                "見本の作品".to_string(),
                None,
            )
        }
    };

    let contents = tm.load_template_contents(&template_name)?;
    let settings = tm.read_settings(&template_name);
    let renderer = EpubRenderer::new(contents, settings.clone());

    // 画像はプレビューの中で完結させたいので、その場でデータ URI に変える。
    let cover_uri = manifest
        .content
        .cover_image
        .as_ref()
        .and_then(|image| thumbnail_data_uri(Path::new(&image.local_path)));

    let css = renderer.render_style(&manifest)?;
    let cover = match (&cover_uri, settings.include_cover_page) {
        (Some(uri), true) => Some(renderer.render_cover_page(&manifest, uri, 0, 0)?),
        _ => None,
    };
    let info = settings
        .include_info_page
        .then(|| renderer.render_info_page(&manifest, cover_uri.as_deref()))
        .transpose()?;

    let page = match manifest.content.pages.first() {
        Some(first) => {
            let html = preview_page_html(first, &manifest);
            Some(renderer.render_page(&manifest, first, &html)?)
        }
        None => None,
    };

    let entries = preview_nav_entries(&manifest, &settings);
    let nav = renderer.render_nav(
        &manifest,
        &entries,
        cover.is_some(),
        Some("text/page_001.xhtml"),
    )?;
    let ncx = settings
        .include_ncx
        .then(|| renderer.render_ncx(&manifest, &entries, &manifest.core.id_))
        .transpose()?;
    let opf = renderer.render_content_opf(
        &manifest,
        &[],
        &[],
        cover_uri.as_ref().map(|_| "cover-image"),
        &crate::epub::meta::to_utc_timestamp(&manifest.core.date_published)
            .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string()),
        crate::epub::meta::to_utc_timestamp(&manifest.core.date_published).as_deref(),
    )?;

    // 描いたそばから整形式かどうかを見る。テンプレートの書き間違いは
    // 書き出してからではなく、編集している最中に分かるのが望ましい。
    let mut issues = Vec::new();
    for (name, document) in [
        ("cover_page.xhtml.j2", cover.as_deref()),
        ("info_page.xhtml.j2", info.as_deref()),
        ("page_wrapper.xhtml.j2", page.as_deref()),
        ("nav.xhtml.j2", Some(nav.as_str())),
        ("toc.ncx.j2", ncx.as_deref()),
        ("content.opf.j2", Some(opf.as_str())),
    ] {
        let Some(document) = document else { continue };
        if let Err(error) = validate::check_well_formed(document) {
            issues.push(EpubValidationIssue::error(
                "TEMPLATE-NOT-WELL-FORMED",
                name,
                format!("整形式ではない出力になります: {}", error),
            ));
        }
    }

    Ok(TemplatePreview {
        sample_title: title,
        sample_source: source,
        sample_download_id: resolved_id,
        css,
        cover,
        info,
        page,
        nav,
        opf,
        ncx,
        issues,
        fields: describe_manifest(&manifest),
    })
}

/// プレビュー用の本文。挿絵はデータ URI に置き換え、長すぎる本文は切り詰める。
fn preview_page_html(page: &EpubPage, manifest: &EpubManifest) -> String {
    let html = crate::epub::xhtml::sanitize_fragment_with(&page.html_content, &mut |src| {
        let stem = Path::new(src.rsplit('/').next().unwrap_or(src))
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_string();
        let matched = manifest
            .content
            .illustrations
            .iter()
            .find(|image| image.local_path.contains(&stem))
            .and_then(|image| thumbnail_data_uri(Path::new(&image.local_path)));
        match matched {
            Some(uri) => crate::epub::xhtml::ImageRef::Keep(uri),
            None => crate::epub::xhtml::ImageRef::Drop,
        }
    });
    // 一画面ぶん見えれば足りる。全文を渡すと編集のたびに重くなる。
    if html.chars().count() > 6000 {
        let truncated: String = html.chars().take(6000).collect();
        return crate::epub::xhtml::sanitize_fragment(&truncated);
    }
    html
}

fn preview_nav_entries(
    manifest: &EpubManifest,
    settings: &TemplateSettings,
) -> Vec<crate::epub::template::NavEntry> {
    use crate::epub::template::NavEntry;
    let mut entries = Vec::new();
    let mut order = 0u32;
    if settings.include_info_page {
        order += 1;
        entries.push(NavEntry {
            id: "info".into(),
            order,
            href: "text/info.xhtml".into(),
            title: settings
                .strings
                .get("INFO_TITLE")
                .cloned()
                .unwrap_or_else(|| "作品情報".into()),
            children: Vec::new(),
        });
    }
    for page in &manifest.content.pages {
        order += 1;
        let page_order = order;
        let href = format!("text/page_{:03}.xhtml", page.order);
        let mut children = Vec::new();
        if settings.chapter_toc {
            for chapter in &page.chapters {
                order += 1;
                children.push(NavEntry {
                    id: chapter.id.clone(),
                    order,
                    href: format!("{}#{}", href, chapter.id),
                    title: chapter.title.clone(),
                    children: Vec::new(),
                });
            }
        }
        entries.push(NavEntry {
            id: format!("page-{:03}", page.order),
            order: page_order,
            href,
            title: page
                .title
                .clone()
                .unwrap_or_else(|| format!("ページ {}", page.order)),
            children,
        });
    }
    entries
}

/// プレビューに埋める小さな画像。
fn thumbnail_data_uri(path: &Path) -> Option<String> {
    use base64::Engine;
    let image = image::ImageReader::open(path).ok()?.decode().ok()?;
    let thumbnail = image.thumbnail(480, 720);
    let mut buffer = std::io::Cursor::new(Vec::new());
    thumbnail
        .write_to(&mut buffer, image::ImageFormat::Jpeg)
        .ok()?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(buffer.into_inner());
    Some(format!("data:image/jpeg;base64,{}", encoded))
}

// ============================================================
// データ辞書
// ============================================================

/// テンプレートに書ける値の一覧を、実際の中身つきで作る。
///
/// 一覧を手で持つと、抽出側が増えたときに必ず食い違う。中間形式そのものから
/// 起こすことで、「取り出せるもの」と「差し込めるもの」を常に一致させる。
fn describe_manifest(manifest: &EpubManifest) -> Vec<DataField> {
    let Ok(value) = serde_json::to_value(manifest) else {
        return Vec::new();
    };
    let mut fields = Vec::new();
    walk_value(&value, "", "", &mut fields);
    fields.sort_by(|a, b| a.group.cmp(&b.group).then(a.path.cmp(&b.path)));
    fields
}

fn walk_value(value: &serde_json::Value, path: &str, group: &str, out: &mut Vec<DataField>) {
    // 入れ子は 3 段まで。それより深いものはテンプレートから触る対象ではない。
    if path.matches('.').count() > 3 {
        return;
    }
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let next = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", path, key)
                };
                let group = if group.is_empty() {
                    key.as_str()
                } else {
                    group
                };
                walk_value(child, &next, group, out);
            }
        }
        serde_json::Value::Array(items) => {
            out.push(DataField {
                label: format!("{}（{}件）", field_label(path), items.len()),
                sample: if items.is_empty() {
                    "（空）".into()
                } else {
                    format!("{}件", items.len())
                },
                available: !items.is_empty(),
                group: group.to_string(),
                path: path.to_string(),
            });
            if let Some(first) = items.first() {
                walk_value(first, &format!("{}[0]", path), group, out);
            }
        }
        serde_json::Value::Null => out.push(DataField {
            label: field_label(path),
            sample: "（なし）".into(),
            available: false,
            group: group.to_string(),
            path: path.to_string(),
        }),
        other => {
            let text = match other {
                serde_json::Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            let sample: String = text.chars().take(120).collect();
            out.push(DataField {
                label: field_label(path),
                available: !sample.trim().is_empty(),
                sample: if sample.trim().is_empty() {
                    "（空）".into()
                } else {
                    sample
                },
                group: group.to_string(),
                path: path.to_string(),
            });
        }
    }
}

fn field_label(path: &str) -> String {
    let known = match path {
        "core.name" => "作品タイトル",
        "core.id_" => "識別子 (URN)",
        "core.author.name" => "作者名",
        "core.author.id" => "作者ID",
        "core.author.account" => "アカウント名",
        "core.author.url" => "作者ページURL",
        "core.author.iconUrl" => "作者アイコンURL",
        "core.description" => "紹介文 (XHTML)",
        "core.descriptionText" => "紹介文 (テキスト)",
        "core.keywords" => "タグ名の一覧",
        "core.tags" => "タグ (英訳つき)",
        "core.datePublished" => "公開日時",
        "core.dateModified" => "更新日時",
        "core.mainEntityOfPage" => "配信元URL",
        "core.isPartOf.name" => "シリーズ名",
        "core.isPartOf.order" => "シリーズ内の話数",
        "core.isPartOf.url" => "シリーズURL",
        "core.language" => "言語",
        "core.publisher" => "配信元",
        "provider.source" => "取得元 (pixiv / fanbox)",
        "provider.novelId" => "pixiv 小説ID",
        "provider.novel_id" => "pixiv 小説ID",
        "provider.post_id" => "FANBOX 投稿ID",
        "provider.series_id" => "シリーズID",
        "provider.postType" => "投稿種別",
        "stats.textLength" => "文字数",
        "stats.pageCount" => "ページ数",
        "stats.chapterCount" => "章の数",
        "stats.imageCount" => "画像点数",
        "stats.attachmentCount" => "添付ファイル数",
        "stats.likeCount" => "いいね / ブックマーク",
        "stats.commentCount" => "コメント数",
        "stats.feeRequired" => "必要な支援額",
        "stats.adult" => "年齢制限",
        "content.pages" => "本文ページ",
        "content.illustrations" => "挿絵",
        "content.attachments" => "添付ファイル",
        "content.cover_image" => "表紙画像",
        "content.text_length" => "文字数",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    path.rsplit('.').next().unwrap_or(path).to_string()
}

/// 作品を選ばずにテンプレートを見比べるための見本。
fn sample_manifest() -> EpubManifest {
    let pages = vec![EpubPage {
        title: Some("第一章 はじまり".into()),
        html_content: "<h2 id=\"chapter-001\">第一章 はじまり</h2>\n<p>雨の匂いがした。<ruby>硝子<rp>(</rp><rt>ガラス</rt><rp>)</rp></ruby>越しの街は、いつもより遠く見える。</p>\n<p class=\"blank-line\"><br /></p>\n<p>「行こうか」と、彼女は言った。</p>".into(),
        order: 1,
        chapters: vec![EpubChapter {
            id: "chapter-001".into(),
            title: "第一章 はじまり".into(),
        }],
    }];
    EpubManifest {
        core: EpubCore {
            id_: "urn:uuid:00000000-0000-4000-8000-000000000000".into(),
            name: "見本の作品 ― 雨と硝子".into(),
            author: EpubAuthor {
                name: "見本 作者".into(),
                id: "0".into(),
                account: Some("sample".into()),
                url: Some("https://www.pixiv.net/users/0".into()),
                icon_url: None,
            },
            description: Some("<p>テンプレートの見え方を確かめるための紹介文です。</p>".into()),
            description_text: Some("テンプレートの見え方を確かめるための紹介文です。".into()),
            keywords: vec!["見本".into(), "テンプレート".into()],
            tags: vec![
                EpubTag {
                    name: "見本".into(),
                    translated: Some("sample".into()),
                },
                EpubTag {
                    name: "テンプレート".into(),
                    translated: None,
                },
            ],
            date_published: "2024-04-01T09:00:00+09:00".into(),
            date_modified: Some("2024-05-02T12:00:00+09:00".into()),
            main_entity_of_page: "https://www.pixiv.net/novel/show.php?id=0".into(),
            is_part_of: Some(EpubSeries {
                name: "見本のシリーズ".into(),
                order: Some(2),
                id: Some("0".into()),
                url: Some("https://www.pixiv.net/novel/series/0".into()),
            }),
            language: "ja".into(),
            publisher: "pixiv".into(),
        },
        provider: ProviderData {
            source: "sample".into(),
            novel_id: Some("0".into()),
            post_id: None,
            series_id: Some("0".into()),
            post_type: None,
        },
        content: EpubContent {
            pages,
            cover_image: None,
            illustrations: Vec::new(),
            attachments: Vec::new(),
            text_length: 1234,
        },
        stats: EpubStats {
            text_length: 1234,
            page_count: 1,
            chapter_count: 1,
            image_count: 0,
            attachment_count: 0,
            like_count: Some(321),
            comment_count: Some(12),
            fee_required: None,
            adult: false,
        },
    }
}

// ============================================================
// 検証
// ============================================================

#[tauri::command]
pub async fn validate_epub_file(path: String) -> Result<EpubValidationReport, String> {
    tokio::task::spawn_blocking(move || validate::validate_epub(Path::new(&path)))
        .await
        .map_err(|e| format!("スレッドパニック: {}", e))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_data_dictionary_covers_everything_a_template_can_place() {
        let fields = describe_manifest(&sample_manifest());
        let paths: Vec<&str> = fields.iter().map(|field| field.path.as_str()).collect();
        for expected in [
            "core.name",
            "core.author.name",
            "core.isPartOf.name",
            "core.tags",
            "stats.textLength",
            "stats.likeCount",
            "content.pages",
            "provider.source",
        ] {
            assert!(paths.contains(&expected), "{expected} が一覧にありません");
        }
        let title = fields
            .iter()
            .find(|field| field.path == "core.name")
            .unwrap();
        assert_eq!(title.label, "作品タイトル");
        assert!(title.available);
        assert_eq!(title.group, "core");

        // 値の無い項目は「差し込めるが今は空」と分かるように残す。
        let cover = fields
            .iter()
            .find(|field| field.path == "content.cover_image")
            .unwrap();
        assert!(!cover.available);
    }

    #[test]
    fn batch_filenames_keep_provider_ids_and_do_not_collide_on_equal_titles() {
        let pixiv = sample_manifest();
        let pixiv_name = batch_output_filename("同じ/題名", "pixiv", &pixiv, 10);
        assert_eq!(pixiv_name, "同じ_題名 [pixiv-0].epub");

        let mut fanbox = sample_manifest();
        fanbox.provider.novel_id = None;
        fanbox.provider.post_id = Some("99".into());
        let fanbox_name = batch_output_filename("同じ/題名", "fanbox", &fanbox, 11);
        assert_eq!(fanbox_name, "同じ_題名 [fanbox-99].epub");
        assert_ne!(pixiv_name, fanbox_name);

        let long_name = batch_output_filename(&"あ".repeat(300), "pixiv", &pixiv, 10);
        assert!(long_name.ends_with(" [pixiv-0].epub"));
        assert_eq!(long_name.trim_end_matches(".epub").chars().count(), 120);
    }

    #[test]
    fn duplicate_batch_ids_are_removed_without_reordering() {
        assert_eq!(
            deduplicate_download_ids(vec![7, 3, 7, 9, 3, 11]),
            vec![7, 3, 9, 11]
        );
    }

    fn atomic_test_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "piep_epub_atomic_{label}_{}_{:016x}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&path).expect("test directory");
        path
    }

    #[test]
    fn a_failed_build_never_truncates_an_existing_output() {
        let dir = atomic_test_dir("failure");
        let destination = dir.join("existing.epub");
        std::fs::write(&destination, b"known-good").unwrap();

        let result = build_validate_and_publish(
            &destination,
            |staged| {
                std::fs::write(staged, b"partial").unwrap();
                Err("injected build failure".into())
            },
            |_| true,
        );

        assert!(result.is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"known-good");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn invalid_staged_epub_is_not_published_or_counted_as_success() {
        let dir = atomic_test_dir("invalid");
        let destination = dir.join("existing.epub");
        std::fs::write(&destination, b"known-good").unwrap();
        let published = build_validate_and_publish(
            &destination,
            |staged| std::fs::write(staged, b"invalid").map_err(|error| error.to_string()),
            |_| false,
        )
        .unwrap();
        assert!(!published);
        assert_eq!(std::fs::read(&destination).unwrap(), b"known-good");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);

        let result = aggregate_batch_results(vec![BatchItemResult {
            index: 1,
            download_id: 42,
            status: BatchItemStatus::Invalid,
            issues: vec![EpubValidationIssue::error("TEST", "content.opf", "invalid")],
        }]);
        assert_eq!(result.success_count, 0);
        assert_eq!(result.failed_count, 1);
        assert_eq!(result.invalid_count, 1);
        assert_eq!(result.invalid_ids, vec![42]);
        assert!(result.output_files.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn valid_staged_epub_replaces_the_destination() {
        let dir = atomic_test_dir("success");
        let destination = dir.join("existing.epub");
        std::fs::write(&destination, b"old").unwrap();
        let published = build_validate_and_publish(
            &destination,
            |staged| std::fs::write(staged, b"new").map_err(|error| error.to_string()),
            |_| true,
        )
        .unwrap();
        assert!(published);
        assert_eq!(std::fs::read(&destination).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
