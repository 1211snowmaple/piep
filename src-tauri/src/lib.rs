/// 実データで挙動を確かめる example から触れるように公開している。
pub mod assist;
mod auth;
pub mod commands;
mod credential_manager;
/// 実データで規則を確かめる example から触れるように公開している。
pub mod database;
mod downloader;
pub mod epub;
pub mod fanbox_api;
/// 元ファイルを保持する新しいライブラリの保管・履歴モデル。
pub mod library;
pub mod library_assist;
pub mod library_collection_edit;
pub mod library_collections;
pub mod library_entity;
pub mod library_entity_list;
pub mod library_epub;
pub mod library_facets;
pub mod library_health;
pub mod library_original_images;
pub mod library_search;
pub mod library_suggest;
pub mod library_suggestion_corpus;
pub mod library_suggestions;
pub mod library_view;
pub mod library_work_edit;
mod logging;
/// 棚の実物で取り出しを確かめる example から触れるように公開している。
pub mod pdf;
pub mod pixiv_api;
pub mod search_suggest;
/// 単独サーバー `piep-server` からも使うので公開している。
pub mod server;
mod smoke_test;
mod source_assets;
mod source_time;

use std::path::Path;
use std::sync::Arc;
use tauri::{Emitter, Manager};

const LIBRARY_LOCK_FILENAME: &str = ".piep-library.lock";
/// 移行や後始末を走らせるスレッドの余裕。既定の 2MB では、メインスレッドで
/// 動いていた頃より狭くなる。
const STARTUP_THREAD_STACK_BYTES: usize = 8 * 1024 * 1024;

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

/// アプリ全体で共有される状態。
pub struct AppState {
    /// 起動時に開いた棚。取得ワーカーも同じ口から参照する。
    pub(crate) shelf: Arc<commands::shelf::Shelf>,
    /// Export holds a read guard while every library mutation holds a write
    /// guard, giving backups one coherent DB/filesystem generation.
    pub library_gate: Arc<tokio::sync::RwLock<()>>,
    // Keep this last: struct fields are dropped in declaration order, so the
    // library and its workers are gone before process ownership is released.
    _library_process_lock: Option<LibraryProcessLock>,
}

impl AppState {
    pub fn new(shelf: Arc<commands::shelf::Shelf>) -> Self {
        Self {
            shelf,
            library_gate: Arc::new(tokio::sync::RwLock::new(())),
            _library_process_lock: None,
        }
    }

    fn new_with_library_lock(
        shelf: Arc<commands::shelf::Shelf>,
        library_process_lock: LibraryProcessLock,
    ) -> Self {
        Self {
            shelf,
            library_gate: Arc::new(tokio::sync::RwLock::new(())),
            _library_process_lock: Some(library_process_lock),
        }
    }
}

/// 棚を開いて、コマンドが使う状態を登録する。メインスレッドの外で走る。
///
/// 終わるまで画面は他のコマンドを呼ばない（`startup_wait_for_library`）。
fn prepare_library(
    app: &tauri::AppHandle,
    app_data: &Path,
    library_process_lock: LibraryProcessLock,
) -> Result<(), String> {
    // 埋め込みモデルの置き場は、開く前に決めておく。既定は起動した場所
    // なので、決めないと起動場所ごとに 465MB を落として置き去りにする。
    database::semantic_index::set_model_cache_dir(app_data);

    // 添付 PDF を読むための pdfium。配布物では実行ファイルの隣に配られる
    // （`tauri.conf.json` の `bundle.resources`）。見つからなくても起動は
    // 止めない。**止めると、添付を持たない作品まで開けなくなる。**
    match app.path().resolve(
        "vendor/pdfium/pdfium.dll",
        tauri::path::BaseDirectory::Resource,
    ) {
        // `resolve` は道を組み立てるだけで、在るかどうかは見ない。
        // **同梱から漏れても、添付 PDF を開くまで誰も気づかない。**
        // 起動時に一度だけ確かめて、記録に残す。
        Ok(path) if path.is_file() => pdf::set_library_path(path),
        Ok(path) => log::warn!(
            "pdfium が同梱されていない。添付 PDF の本文は取り込めない: {}",
            path.display()
        ),
        Err(error) => log::warn!("pdfium の置き場所を決められない: {error}"),
    }

    let shelf = app.state::<Arc<commands::shelf::Shelf>>().inner().clone();
    commands::shelf::open_for_startup(Arc::clone(&shelf), app_data)?;
    let state = Arc::new(AppState::new_with_library_lock(shelf, library_process_lock));
    if let Err(e) = state.jobs().recover_update_jobs_on_startup() {
        log::warn!("Failed to recover update jobs: {}", e);
    }
    // 終わったジョブは放っておくと溜まり続ける。起動時に一度だけ整理する。
    match state.jobs().prune_update_jobs(
        commands::update_jobs::KEEP_UPDATE_JOBS,
        commands::update_jobs::KEEP_UPDATE_JOB_DAYS,
    ) {
        Ok(removed) if removed > 0 => log::info!("古い更新ジョブを{removed}件整理しました"),
        Ok(_) => {}
        Err(e) => log::warn!("Failed to prune update jobs: {}", e),
    }

    // デフォルトEPUBテンプレートの初期化
    let templates_dir = app_data.join("templates");
    std::fs::create_dir_all(&templates_dir)
        .map_err(|e| format!("Failed to create templates dir: {e}"))?;
    let tm = epub::template::TemplateManager::new(templates_dir);
    if let Err(e) = tm.initialize_defaults() {
        log::warn!("デフォルトテンプレートの初期化に失敗: {}", e);
    }

    // サーバーの機能が使えなくても、手元の本棚は開ける。
    match server::desktop::DesktopServer::new(app_data) {
        Ok(desktop_server) => {
            let desktop_server = Arc::new(desktop_server);
            let events = app.clone();
            desktop_server.set_favorites_listener(Arc::new(move || {
                let _ = events.emit("favorites-changed", ());
            }));
            desktop_server.start_background(Arc::clone(&state));
            app.manage(desktop_server);
        }
        Err(e) => log::error!("サーバーの機能を初期化できません: {e}"),
    }
    app.manage(state);
    Ok(())
}

// ============================================================
// アプリ起動
// ============================================================

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() -> tauri::Result<()> {
    // Set the identifier before plugins and webviews resolve any application paths.
    let mut context = tauri::generate_context!();
    let smoke_run_id = std::env::var(smoke_test::RUN_ID_ENV).ok();
    smoke_test::configure(context.config_mut(), smoke_run_id.as_deref())?;
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
        .manage(commands::startup::StartupGate::default())
        .manage(commands::startup::WindowSchemeFile::default())
        .manage(Arc::new(commands::shelf::Shelf::default()))
        // 新しい棚の画像（評価文書 4.1 の 2）。開いていなければ 503。
        .register_asynchronous_uri_scheme_protocol(
            commands::shelf::URI_SCHEME,
            |context, request, responder| {
                let state =
                    Arc::clone(&context.app_handle().state::<Arc<commands::shelf::Shelf>>());
                let path = request.uri().path().to_owned();
                tauri::async_runtime::spawn_blocking(move || {
                    responder.respond(commands::shelf::respond(&state, &path));
                });
            },
        )
        .setup(|app| {
            let app_data = app
                .path()
                .app_data_dir()
                .map_err(|e| std::io::Error::other(format!("Failed to get app data dir: {e}")))?;

            std::fs::create_dir_all(&app_data).map_err(|e| {
                std::io::Error::other(format!("Failed to create app data dir: {e}"))
            })?;

            // 記録の行き先を、他の何よりも先に決める。ここから下の初期化は
            // どれも失敗しうるのに、失敗したことがどこにも残らなかった。
            logging::install(&app_data);

            // 前の版が設定のファイルに平文で置いた鍵を、資格情報マネージャーへ移す。
            // 画面が設定のファイルを開く前（窓を作る前）でないと、古い中身を書き戻される。
            match auth::saved::move_from_settings(
                &auth::saved::Vault::app(),
                &app_data.join("settings.json"),
            ) {
                Ok(true) => log::info!("取得用の接続を資格情報マネージャーへ移しました"),
                Ok(false) => {}
                Err(error) => log::error!("取得用の接続を移せません: {error}"),
            }

            // Claim the whole library before SQLite, restore recovery, search
            // sidecars, or downloaded files can be opened by this process.
            let library_process_lock = LibraryProcessLock::acquire(&app_data)
                .map_err(|e| std::io::Error::other(e.to_string()))?;

            // 窓は設定から自動で作らせない（`create: false`）。作られた時点で色が
            // 決まるので、前回の配色を読んでから自分で作る。
            let window_config = app
                .config()
                .app
                .windows
                .first()
                .cloned()
                .ok_or_else(|| std::io::Error::other("Main window is not configured"))?;
            tauri::WebviewWindowBuilder::from_config(app.handle(), &window_config)?
                .background_color(commands::startup::window_background(&app_data))
                .build()?;

            // ここから先は棚の規模に比例して時間がかかる。`setup` はメインスレッド
            // なので、ここで待つと窓が固まり、Windows が「応答なし」を付ける。
            let handle = app.handle().clone();
            std::thread::Builder::new()
                .name("piep-startup".into())
                .stack_size(STARTUP_THREAD_STACK_BYTES)
                .spawn(move || {
                    let started = std::time::Instant::now();
                    let result = prepare_library(&handle, &app_data, library_process_lock);
                    match &result {
                        Ok(()) => log::info!(
                            "棚の準備が終わりました: {} ms",
                            started.elapsed().as_millis()
                        ),
                        Err(error) => log::error!("棚を開けません: {error}"),
                    }
                    handle
                        .state::<commands::startup::StartupGate>()
                        .finish(result);
                })?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // 起動 (commands::startup)
            commands::startup::startup_wait_for_library,
            commands::startup::startup_remember_color_scheme,
            // 新しい棚の読み取り (commands::shelf)
            commands::shelf::shelf_status,
            commands::shelf::shelf_set_user_state,
            commands::shelf::shelf_remove_work,
            commands::shelf::shelf_edit_collection,
            commands::shelf::shelf_editor,
            commands::shelf::shelf_edit_history,
            commands::shelf::shelf_job_history,
            commands::shelf::shelf_job_history_items,
            commands::shelf::shelf_job_history_logs,
            commands::shelf::shelf_job_history_visibility,
            commands::shelf::shelf_save_editor,
            commands::shelf::shelf_edit_pointer,
            commands::shelf::shelf_import_edit_image,
            commands::shelf::shelf_reading_position,
            commands::shelf::shelf_set_reading_position,
            commands::shelf::shelf_clear_reading_positions,
            commands::shelf::shelf_adopt_device_positions,
            commands::shelf::shelf_search,
            commands::shelf::shelf_decorations,
            commands::shelf::shelf_work,
            commands::shelf::shelf_content,
            commands::shelf::shelf_outline,
            commands::shelf::shelf_search_content,
            commands::shelf::shelf_neighbors,
            commands::shelf::shelf_media,
            commands::shelf::shelf_render_pdf_page,
            commands::shelf::shelf_entity,
            commands::shelf::shelf_entity_tags,
            commands::shelf::shelf_search_suggest,
            commands::shelf::shelf_person_series,
            commands::shelf::shelf_entity_original,
            commands::shelf::shelf_entities,
            commands::shelf::shelf_entity_collections,
            commands::shelf::shelf_work_collections,
            // 新しい棚の数と手入れ (commands::library_health)
            commands::library_health::shelf_search_index_status,
            commands::library_health::shelf_dashboard,
            commands::library_health::shelf_counts,
            commands::library_health::shelf_diagnostics,
            commands::library_health::shelf_maintain,
            commands::library_health::shelf_optimize_search_index,
            commands::library_health::shelf_verify,
            commands::library_health::shelf_reclaim,
            commands::library_health::shelf_filter_facets,
            commands::library_health::shelf_search_filter_facets,
            commands::library_health::shelf_find_work,
            commands::shelf::shelf_collections,
            commands::shelf::shelf_collection,
            commands::shelf::shelf_revision_snapshot,
            commands::shelf::shelf_original,
            // サーバーモード (commands::server)
            commands::server::server_status,
            commands::server::server_create_reader_pairing,
            commands::server::server_list_remote_devices,
            commands::server::server_set_remote_shelf_reader,
            commands::server::server_remote_status,
            commands::server::server_update_remote,
            commands::server::server_check_remote_update,
            commands::server::server_admin_link,
            commands::server::server_connect_remote,
            commands::server::server_disconnect_remote,
            commands::server::server_set_auto_send,
            commands::server::server_send_now,
            commands::server::server_send_stop,
            // 認証 (commands::auth)
            commands::auth::connection_status,
            commands::auth::login_pixiv_webview,
            commands::auth::connect_pixiv_token,
            commands::auth::login_fanbox_webview,
            commands::auth::connect_fanbox_session,
            commands::auth::disconnect_account,
            // ブラウザ (commands::browser)
            commands::browser::open_embedded_browser,
            commands::browser::set_embedded_browser_bounds,
            commands::browser::set_embedded_browser_visible,
            commands::browser::navigate_embedded_browser,
            commands::browser::get_embedded_browser_url,
            commands::browser::read_browser_page_work_candidates,
            commands::browser::read_browser_page_user_candidates,
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
            commands::database::search_rebuild_index,
            commands::database::search_set_semantic_enabled,
            commands::database::search_cancel_rebuild_index,
            commands::database::db_list_saved_searches,
            commands::database::db_upsert_saved_search,
            commands::database::db_delete_saved_search,
            commands::database::db_sweep_collection_candidates,
            commands::database::db_suggest_collection_additions,
            commands::database::db_dismiss_swept_suggestions,
            commands::database::db_propose_collection_names,
            commands::database::db_name_collection_with_model,
            // モデルに手伝ってもらう仕事。どれも押したときだけ動く。
            commands::assist::assist_discover_engines,
            commands::assist::assist_runtime_profile,
            commands::assist::assist_proofread,
            commands::assist::shelf_assist_related_tags,
            commands::assist::shelf_assist_accept_related_tags,
            commands::assist::shelf_assist_remove_related_tag,
            commands::assist::shelf_assist_suggest_tags,
            commands::assist::shelf_assist_accept_tags,
            commands::assist::shelf_assist_work_tags,
            commands::assist::shelf_assist_remove_tag,
            commands::assist::shelf_assist_interpret_search,
            commands::assist::shelf_assist_describe_author,
            commands::assist::shelf_assist_propose_splits,
            commands::assist::shelf_assist_summarize_work,
            commands::assist::shelf_assist_recap_previous,
            commands::assist::shelf_assist_load_note,
            commands::assist::shelf_assist_delete_note,
            commands::database::db_try_naming_engine,
            commands::database::db_name_collection_suggestion,
            commands::database::db_generate_collection_suggestion,
            commands::database::db_list_collection_suggestions,
            commands::database::db_accept_collection_suggestion,
            commands::database::db_reject_collection_suggestion,
            commands::database::db_dismiss_collection_suggestion,
            commands::database::db_get_search_index_status,
            // バージョン管理・更新監視 (commands::database)
            commands::database::db_upsert_update_target,
            commands::database::db_get_update_target,
            commands::database::db_list_update_targets,
            commands::database::db_set_update_target_enabled,
            commands::database::db_delete_update_target,
            commands::database::db_list_watch_groups,
            commands::database::db_create_watch_group,
            commands::database::db_rename_watch_group,
            commands::database::db_delete_watch_group,
            commands::database::db_set_watch_group_members,
            commands::database::refresh_entity_profile,
            commands::database::db_get_entity_profile_repair_status,
            commands::database::repair_incomplete_entity_profiles,
            commands::database::cancel_entity_profile_repair,
            commands::original_images::shelf_original_image_status,
            commands::backup_shelf::shelf_backup,
            commands::backup_shelf::shelf_export_entity_archive,
            commands::backup_shelf::shelf_import_archive,
            commands::reimport_shelf::shelf_reimport_folders,
            commands::backup_shelf::shelf_inspect_backup,
            commands::backup_shelf::shelf_schedule_restore,
            commands::backup_shelf::shelf_restore_state,
            commands::backup_shelf::shelf_cancel_restore,
            commands::original_images::shelf_upgrade_pixiv_images_to_original,
            commands::original_images::cancel_original_image_upgrade,
            // 更新ジョブ (commands::update_jobs)
            commands::update_jobs::start_update_job,
            commands::update_jobs::start_save_job,
            commands::update_jobs::list_update_job_item_states,
            commands::update_jobs::list_update_job_held_item_states,
            commands::update_jobs::pause_update_job,
            commands::update_jobs::resume_update_job,
            commands::update_jobs::cancel_update_job,
            commands::update_jobs::get_update_job,
            commands::update_jobs::list_update_jobs,
            commands::update_jobs::save_update_job_candidates,
            commands::update_jobs::clear_update_job,
            commands::update_jobs::clear_finished_update_jobs,
            commands::update_jobs::dismiss_update_candidate,
            commands::update_jobs::list_deferred_update_candidates,
            commands::update_jobs::count_saved_deferred_update_candidates,
            commands::update_jobs::list_incomplete_save_repair_works,
            commands::update_jobs::record_incomplete_save_repair_attempts,
            commands::update_jobs::recheck_deferred_update_candidates,
            commands::update_jobs::count_dismissed_update_candidates,
            commands::update_jobs::list_pending_revisions,
            commands::update_jobs::preview_pending_revision,
            commands::update_jobs::restore_dismissed_update_candidates,
            // EPUB エクスポート (commands::epub)
            commands::epub::export_epub_batch,
            commands::epub::cancel_epub_export,
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
            commands::shell::open_managed_path,
            commands::shell::reveal_managed_path,
        ])
        .run(context)
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
