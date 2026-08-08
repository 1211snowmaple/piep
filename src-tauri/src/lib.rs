mod auth;
pub mod commands;
mod database;
mod downloader;
pub mod epub;
pub mod fanbox_api;
pub mod pixiv_api;

use database::Database;
use std::sync::Arc;
use tauri::Manager;

/// アプリ全体で共有されるDB状態
pub struct AppState {
    pub db: Database,
}

// ============================================================
// アプリ起動
// ============================================================

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // DB / ストレージの初期化
            let app_data = app
                .path()
                .app_data_dir()
                .expect("Failed to get app data dir");
            let db_path = app_data.join("piep.db");
            let storage_dir = app_data.join("downloads");

            std::fs::create_dir_all(&app_data).expect("Failed to create app data dir");

            let db = Database::open(&db_path, &storage_dir).expect("Failed to open database");
            if let Err(e) = db.recover_update_jobs_on_startup() {
                log::warn!("Failed to recover update jobs: {}", e);
            }

            // デフォルトEPUBテンプレートの初期化
            let templates_dir = app_data.join("templates");
            std::fs::create_dir_all(&templates_dir).expect("Failed to create templates dir");
            let tm = epub::template::TemplateManager::new(templates_dir);
            if let Err(e) = tm.initialize_defaults() {
                log::warn!("デフォルトテンプレートの初期化に失敗: {}", e);
            }

            app.manage(Arc::new(AppState { db }));
            log::info!("Database initialized at {:?}", db_path);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // 認証 (commands::auth)
            commands::auth::verify_pixiv_token,
            commands::auth::verify_fanbox_session,
            commands::auth::login_pixiv_webview,
            commands::auth::login_fanbox_webview,
            // ブラウザ (commands::browser)
            commands::browser::open_embedded_browser,
            commands::browser::set_embedded_browser_bounds,
            commands::browser::set_embedded_browser_visible,
            commands::browser::navigate_embedded_browser,
            commands::browser::get_embedded_browser_url,
            commands::browser::close_embedded_browser,
            commands::browser::destroy_embedded_browser,
            commands::browser::go_back_embedded_browser,
            commands::browser::go_forward_embedded_browser,
            commands::browser::reload_embedded_browser,
            commands::browser::notify_url_changed,
            // データ取得 (commands::downloader)
            commands::downloader::fetch_pixiv_novel_metadata,
            commands::downloader::fetch_pixiv_novel,
            commands::downloader::fetch_pixiv_novel_by_url,
            commands::downloader::fetch_pixiv_series_novels,
            commands::downloader::fetch_pixiv_user_novels,
            commands::downloader::fetch_fanbox_post,
            commands::downloader::fetch_fanbox_creator_posts,
            // DB管理ダウンロード
            commands::downloader::download_and_save,
            commands::downloader::db_check_exists,
            // DB検索・閲覧 (commands::database)
            commands::database::search_downloads_v2,
            commands::database::search_suggest,
            commands::database::search_rebuild_index,
            commands::database::search_cancel_rebuild_index,
            commands::database::db_get_download,
            commands::database::db_get_downloads,
            commands::database::db_get_assets,
            commands::database::db_delete_download,
            commands::database::db_delete_downloads,
            commands::database::db_delete_downloads_for_search,
            commands::database::db_get_stats,
            commands::database::db_get_dashboard_summary,
            commands::database::db_seed_test_data,
            commands::database::db_get_filter_facets,
            commands::database::db_get_search_index_status,
            commands::database::db_search_filter_facets,
            commands::database::db_search_entity_facets,
            commands::database::read_file_content,
            commands::database::open_local_asset,
            // バージョン管理・更新監視 (commands::database)
            commands::database::db_get_versions,
            commands::database::db_get_version,
            commands::database::db_delete_version,
            commands::database::db_set_watch_updates,
            commands::database::db_set_watch_updates_for_search,
            commands::database::db_set_favorite,
            commands::database::db_set_flags_for_ids,
            commands::database::db_get_watched_downloads,
            commands::database::db_upsert_update_target,
            commands::database::db_list_update_targets,
            commands::database::db_set_update_target_enabled,
            commands::database::db_delete_update_target,
            commands::database::db_mark_update_target_checked,
            commands::database::db_list_download_relations,
            commands::database::db_get_person,
            commands::database::db_get_series,
            commands::database::db_list_entity_versions,
            commands::database::db_get_latest_entity_profile_json,
            commands::database::refresh_entity_profile,
            commands::database::db_get_download_by_source,
            commands::database::db_get_download_html,
            commands::database::db_get_reader_document,
            commands::database::db_get_editor_document,
            commands::database::db_save_work_draft,
            commands::database::db_activate_work_edit,
            commands::database::import_work_asset,
            // 更新ジョブ (commands::update_jobs)
            commands::update_jobs::start_update_job,
            commands::update_jobs::pause_update_job,
            commands::update_jobs::resume_update_job,
            commands::update_jobs::cancel_update_job,
            commands::update_jobs::get_update_job,
            commands::update_jobs::list_update_jobs,
            commands::update_jobs::save_update_job_candidates,
            commands::update_jobs::clear_update_job,
            // エクスポート / インポート (commands::archive)
            commands::archive::export_single,
            commands::archive::export_all_zip,
            commands::archive::export_entity_zip,
            commands::archive::import_zip,
            commands::archive::scan_and_reimport_downloads,
            // EPUB エクスポート (commands::epub)
            commands::epub::export_epub,
            commands::epub::export_epub_batch,
            commands::epub::list_epub_templates,
            commands::epub::get_template_files,
            commands::epub::read_template_file,
            commands::epub::save_template_file,
            commands::epub::create_epub_template,
            commands::epub::delete_epub_template,
            commands::archive::get_storage_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
