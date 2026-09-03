/// 実データで挙動を確かめる example から触れるように公開している。
pub mod assist;
mod auth;
pub mod commands;
/// 実データで規則を確かめる example から触れるように公開している。
pub mod database;
mod downloader;
pub mod epub;
pub mod fanbox_api;
mod logging;
pub mod pixiv_api;

use database::Database;
use std::path::Path;
use std::sync::Arc;
use tauri::Manager;

const LIBRARY_LOCK_FILENAME: &str = ".piep-library.lock";

/// Process-wide ownership of one library directory.
///
/// The empty lock file deliberately remains on disk after shutdown. Deleting it
/// would allow another process to create and lock a different inode while an
/// existing process still holds the old one. The operating system releases the
/// advisory lock when this handle is dropped or its process terminates.
#[derive(Debug)]
struct LibraryProcessLock {
    _file: std::fs::File,
}

#[derive(Debug, thiserror::Error)]
enum LibraryLockError {
    #[error(
        "このライブラリは別のpiepプロセスで使用中です。ほかのpiepウィンドウを閉じてから、もう一度起動してください。"
    )]
    AlreadyInUse,
    #[error(
        "ライブラリを安全にロックできませんでした。ほかのpiepを終了し、保存先へのアクセス権を確認してから再起動してください。"
    )]
    Unavailable(#[source] std::io::Error),
}

impl LibraryProcessLock {
    fn acquire(app_data: &Path) -> Result<Self, LibraryLockError> {
        let lock_path = app_data.join(LIBRARY_LOCK_FILENAME);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(LibraryLockError::Unavailable)?;
        // 助言ロックは Rust 1.89 で標準ライブラリに入った。fs4 を使っていたのは
        // それ以前の名残で、挙動（プロセス終了で解放される排他ロック）は同じ。
        // 「別のプロセスが持っている」だけが WouldBlock で、他は本当の入出力失敗。
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => Err(LibraryLockError::AlreadyInUse),
            Err(std::fs::TryLockError::Error(error)) => Err(LibraryLockError::Unavailable(error)),
        }
    }
}

/// アプリ全体で共有されるDB状態
pub struct AppState {
    pub db: Database,
    /// Export holds a read guard while every library mutation holds a write
    /// guard, giving backups one coherent DB/filesystem generation.
    pub library_gate: Arc<tokio::sync::RwLock<()>>,
    // Keep this last: struct fields are dropped in declaration order, so the
    // database and its workers are gone before process ownership is released.
    _library_process_lock: Option<LibraryProcessLock>,
}

impl AppState {
    pub fn new(db: Database) -> Self {
        Self {
            db,
            library_gate: Arc::new(tokio::sync::RwLock::new(())),
            _library_process_lock: None,
        }
    }

    fn new_with_library_lock(db: Database, library_process_lock: LibraryProcessLock) -> Self {
        Self {
            db,
            library_gate: Arc::new(tokio::sync::RwLock::new(())),
            _library_process_lock: Some(library_process_lock),
        }
    }
}

// ============================================================
// アプリ起動
// ============================================================

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() -> tauri::Result<()> {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        // 自動確認は画面を開いていないときにも走る。結果は OS の通知で届ける。
        .plugin(tauri_plugin_notification::init())
        // アプリ本体の更新。作品の更新確認とは別物で、こちらは piep 自身の版を
        // 上げる。入れ替えたあと再起動するために process も要る。
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            // DB / ストレージの初期化
            let app_data = app
                .path()
                .app_data_dir()
                .map_err(|e| std::io::Error::other(format!("Failed to get app data dir: {e}")))?;
            let db_path = app_data.join("piep.db");
            let storage_dir = app_data.join("downloads");

            std::fs::create_dir_all(&app_data).map_err(|e| {
                std::io::Error::other(format!("Failed to create app data dir: {e}"))
            })?;

            // 記録の行き先を、他の何よりも先に決める。ここから下の初期化は
            // どれも失敗しうるのに、失敗したことがどこにも残らなかった。
            logging::install(&app_data);

            // 埋め込みモデルの置き場は、開く前に決めておく。既定は起動した場所
            // なので、決めないと起動場所ごとに 465MB を落として置き去りにする。
            database::semantic_index::set_model_cache_dir(&storage_dir);

            // Claim the whole library before SQLite, restore recovery, search
            // sidecars, or downloaded files can be opened by this process.
            let library_process_lock = LibraryProcessLock::acquire(&app_data)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let db = Database::open(&db_path, &storage_dir)
                .map_err(|e| std::io::Error::other(format!("Failed to open database: {e}")))?;
            let state = Arc::new(AppState::new_with_library_lock(db, library_process_lock));
            // 中断した復元の後始末は、起動の条件にしない。片付かなかった分は
            // 控えを残したまま次の起動でもう一度試せる。ここで止めると、読める
            // 棚を持っている人が窓を開けられなくなる。
            for unresolved in commands::archive::recover_interrupted_restores(&state) {
                log::error!("前回の復元を回復できていません: {unresolved}");
            }
            // 分割復元はパートをまたぐと原子的にならない。途中で終わっていれば
            // 棚は「書庫由来」と「元のまま」が混ざった状態なので、黙っていない。
            match state.db.unfinished_restore_manifests() {
                Ok(unfinished) => {
                    for (path, done, total) in unfinished {
                        log::warn!(
                            "分割バックアップの復元が途中で終わっています（{done}/{total} パート）。同じマニフェストをもう一度開くと続きから再開します: {path}"
                        );
                    }
                }
                Err(error) => log::warn!("分割復元の記録を読めません: {error}"),
            }
            if let Err(e) = state.db.recover_update_jobs_on_startup() {
                log::warn!("Failed to recover update jobs: {}", e);
            }
            // 終わったジョブは放っておくと溜まり続ける。起動時に一度だけ整理する。
            match state.db.prune_update_jobs(
                commands::update_jobs::KEEP_UPDATE_JOBS,
                commands::update_jobs::KEEP_UPDATE_JOB_DAYS,
            ) {
                Ok(removed) if removed > 0 => log::info!("古い更新ジョブを{removed}件整理しました"),
                Ok(_) => {}
                Err(e) => log::warn!("Failed to prune update jobs: {}", e),
            }

            // デフォルトEPUBテンプレートの初期化
            let templates_dir = app_data.join("templates");
            std::fs::create_dir_all(&templates_dir).map_err(|e| {
                std::io::Error::other(format!("Failed to create templates dir: {e}"))
            })?;
            let tm = epub::template::TemplateManager::new(templates_dir);
            if let Err(e) = tm.initialize_defaults() {
                log::warn!("デフォルトテンプレートの初期化に失敗: {}", e);
            }

            app.manage(state);
            log::info!("Database initialized at {:?}", db_path);
            // The search index is derived data and has to look after itself:
            // anything that left it behind is caught up in the background
            // rather than waiting for someone to notice and press a button.
            commands::database::start_automatic_index_maintenance(app.handle().clone());
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
            commands::browser::open_standalone_browser,
            commands::browser::close_standalone_browser,
            commands::browser::get_standalone_browser_url,
            // データ取得 (commands::downloader)
            commands::downloader::fetch_pixiv_novel_by_url,
            commands::downloader::fetch_pixiv_series_novels,
            commands::downloader::fetch_pixiv_user_novels,
            commands::downloader::fetch_fanbox_post,
            commands::downloader::fetch_fanbox_creator_posts,
            // DB検索・閲覧 (commands::database)
            commands::database::search_downloads_v2,
            commands::database::search_suggest,
            commands::database::search_rebuild_index,
            commands::database::search_set_semantic_enabled,
            commands::database::search_cancel_rebuild_index,
            commands::database::db_get_downloads,
            commands::database::db_get_assets,
            commands::database::db_delete_download,
            commands::database::db_delete_downloads,
            commands::database::db_delete_downloads_for_search,
            commands::database::db_get_stats,
            commands::database::db_get_library_diagnostics,
            commands::database::db_maintain_library,
            commands::database::db_optimize_search_index,
            commands::database::db_get_dashboard_summary,
            commands::database::db_get_library_shelf_counts,
            commands::database::db_list_entity_series,
            commands::database::db_list_entity_series_paged,
            commands::database::db_list_entity_tags,
            commands::database::db_list_saved_searches,
            commands::database::db_upsert_saved_search,
            commands::database::db_delete_saved_search,
            commands::database::db_list_work_collections,
            commands::database::db_get_work_collection,
            commands::database::db_upsert_work_collection,
            commands::database::db_delete_work_collection,
            commands::database::db_add_work_collection_members,
            commands::database::db_add_downloads_to_collection,
            commands::database::db_create_collection_from_downloads,
            commands::database::db_sort_work_collection_members,
            commands::database::db_sweep_collection_candidates,
            commands::database::db_suggest_collection_additions,
            commands::database::db_dismiss_swept_suggestions,
            commands::database::db_propose_collection_names,
            commands::database::db_name_collection_with_model,
            // モデルに手伝ってもらう仕事。どれも押したときだけ動く。
            commands::assist::assist_discover_engines,
            commands::assist::assist_runtime_profile,
            commands::assist::assist_suggest_tags,
            commands::assist::assist_accept_tags,
            commands::assist::assist_work_tags,
            commands::assist::assist_remove_tag,
            commands::assist::assist_interpret_search,
            commands::assist::assist_describe_author,
            commands::assist::assist_propose_splits,
            commands::assist::assist_summarize_work,
            commands::assist::assist_recap_previous,
            commands::assist::assist_load_note,
            commands::assist::assist_delete_note,
            commands::database::db_try_naming_engine,
            commands::database::db_name_collection_suggestion,
            commands::database::db_remove_work_collection_members,
            commands::database::db_reorder_work_collection_members,
            commands::database::db_list_collections_for_work,
            commands::database::db_list_collections_for_person,
            commands::database::db_list_collections_for_series,
            commands::database::db_generate_collection_suggestion,
            commands::database::db_list_collection_suggestions,
            commands::database::db_accept_collection_suggestion,
            commands::database::db_reject_collection_suggestion,
            commands::database::db_dismiss_collection_suggestion,
            commands::database::db_get_filter_facets,
            commands::database::db_get_search_index_status,
            commands::database::db_search_filter_facets,
            commands::database::db_search_entity_facets,
            commands::database::db_count_entity_facets,
            commands::database::read_file_content,
            commands::database::open_local_asset,
            // バージョン管理・更新監視 (commands::database)
            commands::database::db_set_watch_updates,
            commands::database::db_set_favorite,
            commands::database::db_set_flags_for_ids,
            commands::database::db_upsert_update_target,
            commands::database::db_get_update_target,
            commands::database::db_list_update_targets,
            commands::database::db_set_update_target_enabled,
            commands::database::db_delete_update_target,
            commands::database::db_get_person,
            commands::database::db_get_series,
            commands::database::db_list_entity_versions,
            commands::database::db_get_latest_entity_profile_json,
            commands::database::refresh_entity_profile,
            commands::database::db_scan_fanbox_asset_gaps,
            commands::database::db_get_entity_profile_repair_status,
            commands::database::repair_incomplete_entity_profiles,
            commands::database::cancel_entity_profile_repair,
            commands::database::db_get_download_by_source,
            commands::database::db_get_reader_metadata,
            commands::database::db_get_reader_content_page,
            commands::database::db_search_reader_content,
            commands::database::db_get_editor_document,
            commands::database::db_save_work_draft,
            commands::database::db_activate_work_edit,
            commands::database::import_work_asset,
            // 更新ジョブ (commands::update_jobs)
            commands::update_jobs::start_update_job,
            commands::update_jobs::start_save_job,
            commands::update_jobs::list_update_job_item_states,
            commands::update_jobs::pause_update_job,
            commands::update_jobs::resume_update_job,
            commands::update_jobs::cancel_update_job,
            commands::update_jobs::get_update_job,
            commands::update_jobs::list_update_jobs,
            commands::update_jobs::save_update_job_candidates,
            commands::update_jobs::clear_update_job,
            commands::update_jobs::clear_finished_update_jobs,
            commands::update_jobs::dismiss_update_candidate,
            commands::update_jobs::count_dismissed_update_candidates,
            commands::update_jobs::list_pending_revisions,
            commands::update_jobs::preview_pending_revision,
            commands::update_jobs::restore_dismissed_update_candidates,
            // エクスポート / インポート (commands::archive)
            commands::archive::export_single,
            commands::archive::export_all_multipart,
            commands::archive::export_entity_zip,
            commands::archive::import_zip,
            commands::archive::cancel_archive_restore,
            commands::archive::import_multipart_backup,
            commands::archive::inspect_backup,
            commands::archive::inspect_multipart_backup,
            commands::archive::scan_and_reimport_downloads,
            // EPUB エクスポート (commands::epub)
            commands::epub::export_epub_batch,
            commands::epub::export_collection_epub,
            commands::epub::list_epub_templates,
            commands::epub::get_template_files,
            commands::epub::read_template_file,
            commands::epub::save_template_file,
            commands::epub::reset_template_file,
            commands::epub::create_epub_template,
            commands::epub::rename_epub_template,
            commands::epub::delete_epub_template,
            commands::epub::save_template_settings,
            commands::epub::list_template_file_kinds,
            commands::epub::preview_epub_template,
            commands::archive::get_storage_path,
            commands::shell::open_managed_path,
            commands::shell::reveal_managed_path,
        ])
        .run(tauri::generate_context!())
}

#[cfg(test)]
mod library_lock_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const CHILD_DIR_ENV: &str = "PIEP_TEST_LIBRARY_LOCK_CHILD_DIR";
    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    struct TestDirectory(std::path::PathBuf);

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "piep-library-lock-{label}-{}-{sequence}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_second_handle_is_rejected_without_exposing_the_library_path() {
        let directory = TestDirectory::new("secret-library-name");
        let first = LibraryProcessLock::acquire(&directory.0).unwrap();
        let error = LibraryProcessLock::acquire(&directory.0).unwrap_err();

        assert!(matches!(error, LibraryLockError::AlreadyInUse));
        let displayed = error.to_string();
        assert!(displayed.contains("別のpiepプロセス"));
        assert!(displayed.contains("もう一度起動してください"));
        assert!(!displayed.contains("secret-library-name"));
        assert!(!displayed.contains(&directory.0.display().to_string()));

        let unavailable = LibraryLockError::Unavailable(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "secret-path-from-the-os",
        ));
        let unavailable_display = unavailable.to_string();
        assert!(unavailable_display.contains("アクセス権を確認"));
        assert!(!unavailable_display.contains("secret-path-from-the-os"));

        drop(first);
        LibraryProcessLock::acquire(&directory.0).unwrap();
    }

    #[test]
    fn library_lock_child_process() {
        let Some(directory) = std::env::var_os(CHILD_DIR_ENV) else {
            return;
        };
        let directory = std::path::PathBuf::from(directory);
        let _lock = LibraryProcessLock::acquire(&directory).unwrap();
        std::fs::write(directory.join("child-acquired"), b"locked").unwrap();
        std::process::abort();
    }

    #[test]
    fn an_aborted_process_releases_the_library_lock() {
        let directory = TestDirectory::new("crash-release");
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("library_lock_tests::library_lock_child_process")
            .arg("--nocapture")
            .env(CHILD_DIR_ENV, &directory.0)
            .output()
            .unwrap();

        assert!(!output.status.success(), "child was expected to abort");
        assert!(
            directory.0.join("child-acquired").is_file(),
            "child did not acquire the lock before aborting; stdout: {}; stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        LibraryProcessLock::acquire(&directory.0).unwrap();
    }
}
