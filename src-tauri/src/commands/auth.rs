use crate::auth::fanbox::{check_fanbox_session, FanboxUser};
use crate::auth::pixiv::{login_with_refresh_token, PixivUser};
use crate::auth::webview::{open_fanbox_login, open_pixiv_login};

#[tauri::command]
pub async fn verify_pixiv_token(refresh_token: String) -> Result<PixivUser, String> {
    match login_with_refresh_token(&refresh_token).await {
        Ok(res) => res
            .user
            .ok_or_else(|| "Login success, but user data not found".into()),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
pub async fn verify_fanbox_session(
    session_id: String,
    user_agent: String,
) -> Result<FanboxUser, String> {
    check_fanbox_session(&session_id, &user_agent)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn login_pixiv_webview(app: tauri::AppHandle) -> Result<(String, PixivUser), String> {
    open_pixiv_login(app).await
}

#[tauri::command]
pub async fn login_fanbox_webview(
    app: tauri::AppHandle,
) -> Result<(String, FanboxUser, String), String> {
    open_fanbox_login(app).await
}
