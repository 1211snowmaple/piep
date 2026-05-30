//! EPUB エクスポート用 Tauri コマンド（進捗イベント付き）。

use crate::epub::builder::{sanitize_epub_filename, EpubBuilder};
use crate::epub::converter;
use crate::epub::intermediate::*;
use crate::epub::template::TemplateManager;
use crate::AppState;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Emitter, Manager};

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
    Ok(TemplateManager::new(dir))
}

fn emit_progress(app: &tauri::AppHandle, progress: &ExportProgress) {
    let _ = app.emit("epub-export-progress", progress);
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
    let state = app.state::<Arc<AppState>>();
    let dl = state.db.get_download(download_id)?;
    let tm = get_template_manager(&app)?;
    tm.initialize_defaults()?;

    let compress = compress_options.unwrap_or_default();
    let title = dl.title.clone();

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

    // data.json / original.json 読み込み
    let target_json_path = dl
        .original_json_path
        .clone()
        .filter(|p| std::path::Path::new(p).exists())
        .unwrap_or_else(|| dl.json_path.clone());

    log::info!("EPUB export: reading json from {}", target_json_path);
    let json_content = std::fs::read_to_string(&target_json_path)
        .map_err(|e| format!("JSONの読み込みに失敗: {}", e))?;
    let mut data: serde_json::Value =
        serde_json::from_str(&json_content).map_err(|e| format!("JSON パースエラー: {}", e))?;

    let json_dir = std::path::Path::new(&target_json_path).parent().unwrap();
    let assets_dir = json_dir.join("data_assets");

    let is_fanbox = dl.source == "fanbox";
    crate::downloader::asset_downloader::inject_local_paths(&mut data, "data_assets", is_fanbox);

    // 中間形式に変換
    emit_progress(
        &app,
        &ExportProgress {
            phase: "converting".into(),
            current_title: title.clone(),
            current_index: 1,
            total_count: 1,
            message: format!("「{}」を中間形式に変換中...", title),
        },
    );
    let manifest = converter::convert_to_manifest(&data, &dl.source, &assets_dir)?;
    log::info!(
        "EPUB export: manifest created, {} pages, {} illustrations",
        manifest.content.pages.len(),
        manifest.content.illustrations.len()
    );

    // テンプレートロード
    let resolved_template = if template_name == "__auto__" {
        match dl.source.as_str() {
            "pixiv" => "pixiv".to_string(),
            _ => "default".to_string(),
        }
    } else {
        template_name.clone()
    };
    let template_contents = tm.load_template_contents(&resolved_template)?;
    log::info!(
        "EPUB export: template '{}' resolved and loaded ({} files)",
        resolved_template,
        template_contents.len()
    );

    // EPUB生成
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
        let builder = EpubBuilder::new(manifest, template_contents, compress);
        let result = builder.build(&out_path);
        if let Err(ref e) = result {
            log::error!("EPUB build failed: {}", e);
            emit_progress(
                &app_clone,
                &ExportProgress {
                    phase: "failed".into(),
                    current_title: title_clone.clone(),
                    current_index: 1,
                    total_count: 1,
                    message: format!("「{}」の生成に失敗: {}", title_clone, e),
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

#[tauri::command]
pub async fn export_epub_batch(
    app: tauri::AppHandle,
    download_ids: Vec<i64>,
    template_name: String,
    output_dir: String,
    compress_options: Option<ImageCompressOptions>,
) -> Result<ExportBatchResult, String> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let tm = get_template_manager(&app)?;
    tm.initialize_defaults()?;

    let compress = compress_options.unwrap_or_default();
    let output_base = PathBuf::from(&output_dir);
    std::fs::create_dir_all(&output_base).map_err(|e| e.to_string())?;

    // テンプレート自動判別用のキャッシュロード機構
    let is_auto = template_name == "__auto__";
    let mut template_cache = std::collections::HashMap::new();

    if !is_auto {
        let contents = tm.load_template_contents(&template_name)?;
        template_cache.insert(template_name.clone(), contents);
    }

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

    let mut success_count = 0u32;
    let mut failed_count = 0u32;
    let mut failed_ids = Vec::new();
    let mut output_files = Vec::new();

    for (idx, dl_id) in download_ids.iter().enumerate() {
        let current_idx = (idx + 1) as u32;
        let dl = match state.db.get_download(*dl_id) {
            Ok(d) => d,
            Err(e) => {
                log::error!("Download {} not found: {}", dl_id, e);
                failed_count += 1;
                failed_ids.push(*dl_id);
                continue;
            }
        };

        // 作品ごとにテンプレート解決
        let resolved_template = if is_auto {
            match dl.source.as_str() {
                "pixiv" => "pixiv".to_string(),
                _ => "default".to_string(),
            }
        } else {
            template_name.clone()
        };

        // キャッシュ解決（未ロードならロードしてキャッシュに格納）
        let current_template_contents =
            if let Some(contents) = template_cache.get(&resolved_template) {
                contents.clone()
            } else {
                let contents = match tm.load_template_contents(&resolved_template) {
                    Ok(c) => c,
                    Err(e) => {
                        log::error!(
                            "Failed to load template '{}' for ID {}: {}",
                            resolved_template,
                            dl_id,
                            e
                        );
                        failed_count += 1;
                        failed_ids.push(*dl_id);
                        continue;
                    }
                };
                template_cache.insert(resolved_template.clone(), contents.clone());
                contents
            };

        let title = dl.title.clone();
        emit_progress(
            &app,
            &ExportProgress {
                phase: "converting".into(),
                current_title: title.clone(),
                current_index: current_idx,
                total_count: total,
                message: format!("[{}/{}] 「{}」を変換中...", current_idx, total, title),
            },
        );

        let compress_clone = compress.clone();
        let current_template_contents_clone = current_template_contents.clone();

        let result: Result<String, String> = (|| {
            let target_json_path = dl
                .original_json_path
                .clone()
                .filter(|p| std::path::Path::new(p).exists())
                .unwrap_or_else(|| dl.json_path.clone());

            let json_content = std::fs::read_to_string(&target_json_path)
                .map_err(|e| format!("JSON読み込み失敗: {}", e))?;
            let mut data: serde_json::Value = serde_json::from_str(&json_content)
                .map_err(|e| format!("JSONパース失敗: {}", e))?;
            let json_dir = std::path::Path::new(&target_json_path).parent().unwrap();
            let assets_dir = json_dir.join("data_assets");

            let is_fanbox = dl.source == "fanbox";
            crate::downloader::asset_downloader::inject_local_paths(
                &mut data,
                "data_assets",
                is_fanbox,
            );

            let manifest = converter::convert_to_manifest(&data, &dl.source, &assets_dir)?;

            let filename = sanitize_epub_filename(&dl.title);
            let out_path = output_base.join(&filename);

            let builder =
                EpubBuilder::new(manifest, current_template_contents_clone, compress_clone);
            builder.build(&out_path)?;

            Ok(out_path.to_string_lossy().to_string())
        })();

        match result {
            Ok(path) => {
                success_count += 1;
                output_files.push(path);
                emit_progress(
                    &app,
                    &ExportProgress {
                        phase: "completed".into(),
                        current_title: title.clone(),
                        current_index: current_idx,
                        total_count: total,
                        message: format!("[{}/{}] 「{}」完了", current_idx, total, title),
                    },
                );
            }
            Err(e) => {
                log::error!("EPUB export error (ID {}): {}", dl_id, e);
                failed_count += 1;
                failed_ids.push(*dl_id);
                emit_progress(
                    &app,
                    &ExportProgress {
                        phase: "failed".into(),
                        current_title: title.clone(),
                        current_index: current_idx,
                        total_count: total,
                        message: format!("[{}/{}] 「{}」失敗: {}", current_idx, total, title, e),
                    },
                );
            }
        }
    }

    emit_progress(
        &app,
        &ExportProgress {
            phase: "completed".into(),
            current_title: String::new(),
            current_index: total,
            total_count: total,
            message: format!(
                "エクスポート完了: {}件成功, {}件失敗",
                success_count, failed_count
            ),
        },
    );

    Ok(ExportBatchResult {
        success_count,
        failed_count,
        failed_ids,
        output_files,
    })
}

// ============================================================
// テンプレート管理コマンド
// ============================================================

#[tauri::command]
pub async fn list_epub_templates(app: tauri::AppHandle) -> Result<Vec<TemplateInfo>, String> {
    let tm = get_template_manager(&app)?;
    tm.initialize_defaults()?;
    tm.list_templates()
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
pub async fn create_epub_template(
    app: tauri::AppHandle,
    template_name: String,
) -> Result<(), String> {
    get_template_manager(&app)?.create_template(&template_name)
}

#[tauri::command]
pub async fn delete_epub_template(
    app: tauri::AppHandle,
    template_name: String,
) -> Result<(), String> {
    get_template_manager(&app)?.delete_template(&template_name)
}
