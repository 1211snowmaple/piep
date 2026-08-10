use crate::auth::webview::{
    close_child_webview, close_standalone_webview, destroy_child_webview, get_child_webview_url,
    go_back_child_webview, go_forward_child_webview, navigate_child_webview, open_child_webview,
    open_standalone_webview, reload_child_webview, set_child_webview_bounds,
    set_child_webview_visible, standalone_webview_url,
};

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
pub async fn set_embedded_browser_bounds(
    app: tauri::AppHandle,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<bool, String> {
    set_child_webview_bounds(app, x, y, width, height)
}

#[tauri::command]
pub async fn set_embedded_browser_visible(
    app: tauri::AppHandle,
    visible: bool,
) -> Result<bool, String> {
    set_child_webview_visible(app, visible)
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
pub async fn open_standalone_browser(
    app: tauri::AppHandle,
    url: String,
    source: String,
    user_agent: Option<String>,
) -> Result<bool, String> {
    open_standalone_webview(app, url, source, user_agent)
}

#[tauri::command]
pub async fn close_standalone_browser(
    app: tauri::AppHandle,
    source: String,
) -> Result<bool, String> {
    close_standalone_webview(app, source)
}

/// Returns the page the large window is showing, or null when it is not open.
#[tauri::command]
pub async fn get_standalone_browser_url(
    app: tauri::AppHandle,
    source: String,
) -> Result<Option<String>, String> {
    standalone_webview_url(app, source)
}
