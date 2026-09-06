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

/// 保存してある更新用トークンが今も通るか確かめ、通れば利用者を返す。
///
/// 起動時と設定画面から呼ぶ。**トークンは無効化されうる**（利用者が pixiv 側で
/// セッションを切る、期限が来る）ので、保存されていることは繋がることを意味
/// しない。失敗したら繋ぎ直しへ誘導する。
///
/// 読むだけで、何も保存しない。
#[tauri::command]
pub async fn verify_pixiv_token(refresh_token: String) -> Result<PixivUser, String> {
    match login_with_refresh_token(&refresh_token).await {
        Ok(res) => res
            .user
            .ok_or_else(|| "Login success, but user data not found".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// セッション ID と UA の組が今も通るか確かめ、通れば利用者を返す。
///
/// **UA を省略しない。** FANBOX の Cookie は発行時の UA と対でだけ意味を持つ
/// （`cf_clearance` が UA に紐づく）ので、別の UA で送ると通らない。保存して
/// あるものをそのまま渡すこと。
///
/// 読むだけで、何も保存しない。
#[tauri::command]
pub async fn verify_fanbox_session(
    session_id: String,
    user_agent: String,
) -> Result<FanboxUser, String> {
    check_fanbox_session(&session_id, &user_agent)
        .await
        .map_err(|e| e.to_string())
}

/// ログイン窓を開き、閉じるまで待って、接続に要るものを持ち帰る。
///
/// **piep がパスワードを受け取ることはない。** 利用者は pixiv 自身の画面に入力
/// し、ここが受け取るのは結果だけである。
///
/// 返る `PixivConnection` のうち `cookie` と `userAgent` は**付け足しであって
/// 接続の条件ではない**。取れなくてもログインは成功で、`None` が返る。この二つ
/// は対でだけ意味を持つので、片方だけを保存しない。
///
/// 窓を閉じられた、途中で失敗した、といった場合は `Err`。呼び出し側は待って
/// いる間、他のログインを始めさせないこと。
#[tauri::command]
pub async fn login_pixiv_webview(app: tauri::AppHandle) -> Result<PixivConnection, String> {
    open_pixiv_login(app).await
}

/// FANBOX のログイン窓を開き、閉じるまで待って、接続に要るものを持ち帰る。
///
/// 返るのは `(セッション Cookie, 利用者, UA)` の三つ組である。**三つとも一緒に
/// 保存する。** Cookie は発行時の UA と対でだけ意味を持つので、UA を捨てると
/// 次の確認から通らなくなる。
///
/// **ログインは一度に一つしか走らない。** すでに走っているときは `Err` で断る。
/// 残っている古い窓があれば、開く前に閉じる。
#[tauri::command]
pub async fn login_fanbox_webview(
    app: tauri::AppHandle,
) -> Result<(String, FanboxUser, String), String> {
    open_fanbox_login(app).await
}
