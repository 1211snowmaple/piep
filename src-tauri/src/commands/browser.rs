use crate::auth::webview::{
    close_child_webview, destroy_child_webview, get_child_webview_url, go_back_child_webview,
    go_forward_child_webview, navigate_child_webview, open_child_webview, reload_child_webview,
};
use tauri::{Emitter, Manager};

#[tauri::command]
pub async fn open_embedded_browser(
    app: tauri::AppHandle,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    user_agent: Option<String>,
) -> Result<(), String> {
    open_child_webview(app, url, x, y, width, height, user_agent)
}

#[tauri::command]
pub async fn navigate_embedded_browser(app: tauri::AppHandle, url: String) -> Result<(), String> {
    navigate_child_webview(app, url)
}

#[tauri::command]
pub async fn get_embedded_browser_url(app: tauri::AppHandle) -> Result<String, String> {
    get_child_webview_url(app)
}

#[tauri::command]
pub async fn close_embedded_browser(app: tauri::AppHandle) -> Result<(), String> {
    close_child_webview(app)
}

#[tauri::command]
pub async fn destroy_embedded_browser(app: tauri::AppHandle) -> Result<(), String> {
    destroy_child_webview(app)
}

#[tauri::command]
pub async fn go_back_embedded_browser(app: tauri::AppHandle) -> Result<(), String> {
    go_back_child_webview(app)
}

#[tauri::command]
pub async fn go_forward_embedded_browser(app: tauri::AppHandle) -> Result<(), String> {
    go_forward_child_webview(app)
}

#[tauri::command]
pub async fn reload_embedded_browser(app: tauri::AppHandle) -> Result<(), String> {
    reload_child_webview(app)
}

#[tauri::command]
pub async fn notify_url_changed(app: tauri::AppHandle, url: String) -> Result<(), String> {
    if let Some(main_window) = app.get_webview_window("main") {
        let _ = main_window.emit("url-changed", url);
    }
    Ok(())
}
