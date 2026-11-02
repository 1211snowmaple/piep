mod pc_client;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            app.manage(pc_client::PcClientState::new(
                data_dir.join("pc-connection.json"),
            ));
            app.manage(pc_client::offline::OfflineState::new(
                data_dir.join("offline"),
            ));
            app.manage(pc_client::reading_sync::ReadingSyncState::new(
                data_dir.join("reading-sync.json"),
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            pc_client::pc_pair,
            pc_client::pc_connection_status,
            pc_client::pc_disconnect,
            pc_client::pc_search_works,
            pc_client::pc_list_entities,
            pc_client::pc_list_collections,
            pc_client::pc_get_work_collections,
            pc_client::pc_get_collection,
            pc_client::pc_get_metadata,
            pc_client::pc_get_page,
            pc_client::pc_get_outline,
            pc_client::pc_get_asset,
            pc_client::pc_get_cover,
            pc_client::pc_get_collection_cover,
            pc_client::pc_get_entity_cover,
            pc_client::reading_sync::reading_record,
            pc_client::reading_sync::reading_sync,
            pc_client::reading_sync::reading_get,
            pc_client::offline::offline_list_works,
            pc_client::offline::offline_list_issues,
            pc_client::offline::offline_save_work,
            pc_client::offline::offline_get_page,
            pc_client::offline::offline_get_outline,
            pc_client::offline::offline_get_asset,
            pc_client::offline::offline_get_cover,
            pc_client::offline::offline_get_collection_cover,
            pc_client::offline::offline_get_entity_cover,
            pc_client::offline::offline_delete_work,
            pc_client::offline::offline_delete_corrupt_work,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
