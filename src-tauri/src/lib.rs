mod app_update;
mod pc_client;
mod secret_cipher;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .register_asynchronous_uri_scheme_protocol(
            pc_client::cover_protocol::SCHEME,
            pc_client::cover_protocol::handle,
        )
        .setup(|app| {
            #[cfg(mobile)]
            app.handle().plugin(tauri_plugin_barcode_scanner::init())?;
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            #[cfg(target_os = "android")]
            let cipher: std::sync::Arc<dyn secret_cipher::SecretCipher> = {
                app.handle().plugin(secret_cipher::init())?;
                std::sync::Arc::new(
                    app.state::<secret_cipher::KeystoreCipher<tauri::Wry>>()
                        .inner()
                        .clone(),
                )
            };
            #[cfg(not(target_os = "android"))]
            let cipher: std::sync::Arc<dyn secret_cipher::SecretCipher> =
                std::sync::Arc::new(secret_cipher::DevelopmentCipher);
            app.manage(pc_client::PcClientState::new(
                data_dir.join("pc-connection.json"),
                cipher,
            ));
            app.manage(pc_client::offline::OfflineState::new(
                data_dir.join("offline"),
            ));
            app.manage(pc_client::reading_sync::ReadingSyncState::new(
                data_dir.join("reading-sync.json"),
            ));
            let covers = pc_client::cover_protocol::CoverCacheState::new(
                app.path().app_cache_dir()?.join("covers"),
            );
            covers.trim();
            app.manage(covers);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_update::app_update_check,
            app_update::app_update_download,
            app_update::app_update_installed_version,
            pc_client::pc_pair,
            pc_client::pc_connection_status,
            pc_client::pc_disconnect,
            pc_client::pc_search_works,
            pc_client::pc_list_entities,
            pc_client::pc_get_entity_detail,
            pc_client::pc_list_entity_series,
            pc_client::pc_list_collections,
            pc_client::pc_get_work_collections,
            pc_client::pc_get_collection,
            pc_client::pc_set_favorite,
            pc_client::pc_get_metadata,
            pc_client::pc_get_page,
            pc_client::pc_get_outline,
            pc_client::reading_sync::reading_record,
            pc_client::reading_sync::reading_sync,
            pc_client::reading_sync::reading_get,
            pc_client::offline::offline_list_works,
            pc_client::offline::offline_list_issues,
            pc_client::offline::offline_save_work,
            pc_client::offline::offline_get_page,
            pc_client::offline::offline_get_outline,
            pc_client::offline::offline_get_asset,
            pc_client::offline::offline_delete_work,
            pc_client::offline::offline_delete_corrupt_work,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
