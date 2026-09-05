//! pixiv と FANBOX への接続を作り、確かめる四つの入口。
//!
//! 仕事は `auth::pixiv` `auth::fanbox` `auth::webview` にある。ここは境界に
//! 出す形だけを決める薄皮である。
//!
//! **piep がパスワードを受け取ることはない。** 利用者は内蔵の WebView2 で
//! サービス自身のログイン画面に入力し、piep はその結果だけを持ち帰る。
//!
//! 持ち帰るものは取得元で違う。pixiv は OAuth の更新用トークンで、web の
//! セッション Cookie は**付け足しであって接続の条件ではない**。FANBOX は
//! セッション Cookie そのものが鍵になる。どちらも UA と対でだけ意味を持つ
//! ので、片方だけを保存しない。
//!
//! → [取得の方針](https://1211snowmaple.github.io/piep/policy/07-acquisition)

use crate::auth::fanbox::{check_fanbox_session, FanboxUser};
use crate::auth::pixiv::{login_with_refresh_token, PixivUser};
use crate::auth::webview::{open_fanbox_login, open_pixiv_login, PixivConnection};

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
pub async fn login_pixiv_webview(app: tauri::AppHandle) -> Result<PixivConnection, String> {
    open_pixiv_login(app).await
}

#[tauri::command]
pub async fn login_fanbox_webview(
    app: tauri::AppHandle,
) -> Result<(String, FanboxUser, String), String> {
    open_fanbox_login(app).await
}
