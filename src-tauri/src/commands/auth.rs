//! pixiv と FANBOX への接続を作り、解く入口。
//!
//! 仕事は `auth::pixiv` `auth::fanbox` `auth::webview` にあり、接続は
//! `auth::saved` が Windows の資格情報マネージャーに置く。ここは境界に出す
//! 形だけを決める薄皮である。
//!
//! **piep がパスワードを受け取ることはない。** 利用者は内蔵の WebView2 で
//! サービス自身のログイン画面に入力し、piep はその結果だけを持ち帰る。
//!
//! **鍵は画面へ返さない。** どの入口も、返すのは接続の様子（繋がっているか
//! と利用者の名前）とアカウントの識別子である。保存や更新確認は Rust の中で鍵を読む。
//!
//! **アカウントは取得元ごとに複数登録でき、使うのはそのうち一つ。** ログインは
//! 登録を一つ足し（同じ人なら差し替え）、足したものを使うアカウントにする。
//!
//! 持ち帰るものは取得元で違う。pixiv は OAuth の更新用トークンで、web の
//! セッション Cookie は**付け足しであって接続の条件ではない**。FANBOX は
//! セッション Cookie そのものが鍵になる。どちらも UA と対でだけ意味を持つ
//! ので、片方だけを保存しない。
//!
//! → [取得の方針](https://1211snowmaple.github.io/piep/policy/07-acquisition)

use crate::auth::fanbox::check_fanbox_session;
use crate::auth::pixiv::login_with_refresh_token;
use crate::auth::saved::{
    ConnectionStatus, FanboxAccount, PixivAccount, Vault, WebSession, DEFAULT_USER_AGENT,
};
use crate::auth::webview::{open_fanbox_login, open_pixiv_login};

/// 今の接続の様子。鍵そのものは返さない。
///
/// 保存されていることは繋がることを意味しない（トークンは無効化されうる）。
/// 通らなかったときは、取得の仕事がそれぞれ繋ぎ直しへ誘導する。
#[tauri::command]
pub fn connection_status() -> Result<ConnectionStatus, String> {
    Ok(Vault::app().status())
}

/// ログイン窓を開き、閉じるまで待って、持ち帰った接続を保存する。
///
/// web のセッション（Cookie と UA）は**付け足しであって接続の条件ではない**。
/// 取れなくてもログインは成功で、そのときは前の接続のセッションも残さない。
///
/// 窓を閉じられた、途中で失敗した、といった場合は `Err`。呼び出し側は待って
/// いる間、他のログインを始めさせないこと。
#[tauri::command]
pub async fn login_pixiv_webview(app: tauri::AppHandle) -> Result<ConnectionStatus, String> {
    let connection = open_pixiv_login(app).await?;
    let vault = Vault::app();
    vault.add_pixiv(
        &connection.user.id,
        &PixivAccount {
            refresh_token: connection.refresh_token,
            web_session: WebSession::pair(connection.cookie, connection.user_agent),
            user_name: connection.user.name,
        },
    )?;
    Ok(vault.status())
}

/// 手で貼られた更新用トークンを確かめ、通れば保存する。
///
/// 貼ったトークンには web のセッションが付かないので、更新確認は従来の
/// 経路になる。
#[tauri::command]
pub async fn connect_pixiv_token(refresh_token: String) -> Result<ConnectionStatus, String> {
    let refresh_token = refresh_token.trim().to_string();
    let user = login_with_refresh_token(&refresh_token)
        .await
        .map_err(|e| e.to_string())?
        .user
        .ok_or_else(|| "接続した利用者の情報を取得できません".to_string())?;
    let vault = Vault::app();
    vault.add_pixiv(
        &user.id,
        &PixivAccount {
            refresh_token,
            web_session: None,
            user_name: user.name,
        },
    )?;
    Ok(vault.status())
}

/// FANBOX のログイン窓を開き、閉じるまで待って、持ち帰った接続を保存する。
///
/// **セッション Cookie と UA は一緒に保存する。** Cookie は発行時の UA と対で
/// だけ意味を持つので、UA を捨てると次の確認から通らなくなる。
///
/// **ログインは一度に一つしか走らない。** すでに走っているときは `Err` で断る。
#[tauri::command]
pub async fn login_fanbox_webview(app: tauri::AppHandle) -> Result<ConnectionStatus, String> {
    let (session_id, user, user_agent) = open_fanbox_login(app).await?;
    let vault = Vault::app();
    vault.add_fanbox(
        &user.user_id,
        &FanboxAccount {
            session_id,
            user_agent,
            user_name: user.name,
        },
    )?;
    Ok(vault.status())
}

/// 手で貼られたセッション ID と UA の組を確かめ、通れば保存する。
///
/// **UA を省略しない。** FANBOX の Cookie は発行時の UA と対でだけ意味を持つ
/// （`cf_clearance` が UA に紐づく）ので、別の UA で送ると通らない。
#[tauri::command]
pub async fn connect_fanbox_session(
    session_id: String,
    user_agent: Option<String>,
) -> Result<ConnectionStatus, String> {
    let session_id = session_id.trim().to_string();
    let user_agent = user_agent
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_USER_AGENT.to_string());
    let user = check_fanbox_session(&session_id, &user_agent)
        .await
        .map_err(|e| e.to_string())?;
    let vault = Vault::app();
    vault.add_fanbox(
        &user.user_id,
        &FanboxAccount {
            session_id,
            user_agent,
            user_name: user.name,
        },
    )?;
    Ok(vault.status())
}

/// 登録したアカウントひとつの接続を解く。保存済みの作品には触れない。
///
/// 使っていたアカウントを解いたときは、残りの最初のアカウントを使う。
/// `key` は接続の様子の `accounts` に載っている印。
#[tauri::command]
pub fn disconnect_account(source: String, key: String) -> Result<ConnectionStatus, String> {
    let vault = Vault::app();
    vault.forget(&source, &key)?;
    Ok(vault.status())
}

/// 読み取れない取得元の接続を消し、設定から登録し直せるようにする。
#[tauri::command]
pub fn clear_unreadable_accounts(source: String) -> Result<ConnectionStatus, String> {
    let vault = Vault::app();
    vault.clear_unreadable(&source)?;
    Ok(vault.status())
}

/// 保存・更新確認に使うアカウントを切り替える。
///
/// 走っている仕事は始めたときの鍵で最後まで進み、次の仕事から切り替わる。
#[tauri::command]
pub fn activate_account(source: String, key: String) -> Result<ConnectionStatus, String> {
    let vault = Vault::app();
    vault.activate(&source, &key)?;
    Ok(vault.status())
}
