//! サーバーモードのコマンド。実体は `crate::server::desktop` にある。

use crate::server::desktop::{DesktopServer, DesktopServerStatus};
use crate::AppState;
use std::sync::Arc;
use tauri::Manager;

fn controller(app: &tauri::AppHandle) -> Result<Arc<DesktopServer>, String> {
    app.try_state::<Arc<DesktopServer>>()
        .map(|state| state.inner().clone())
        .ok_or_else(|| "サーバーの機能を初期化できませんでした".to_string())
}

fn library(app: &tauri::AppHandle) -> Arc<AppState> {
    app.state::<Arc<AppState>>().inner().clone()
}

/// 単独サーバーへの接続の、今の状態を返す。
#[tauri::command]
pub async fn server_status(app: tauri::AppHandle) -> Result<DesktopServerStatus, String> {
    Ok(controller(&app)?.status().await)
}

/// 単独サーバーの版・配信状況・管理画面の状態を返す。
#[tauri::command]
pub async fn server_remote_status(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    controller(&app)?.remote_status().await
}

/// 管理画面へ入る、一度きりのリンクを返す。
#[tauri::command]
pub async fn server_admin_link(app: tauri::AppHandle) -> Result<String, String> {
    controller(&app)?.admin_link().await
}

/// 管理者用の登録文字列で単独サーバーにつなぐ。鍵は資格情報マネージャーに置く。
#[tauri::command]
pub async fn server_connect_remote(
    app: tauri::AppHandle,
    pairing: String,
) -> Result<DesktopServerStatus, String> {
    let server = controller(&app)?;
    let device_name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "デスクトップ版".into());
    server.connect_remote(&pairing, &device_name).await?;
    Ok(server.status().await)
}

/// 単独サーバーとのつながりを解除し、手元の鍵を消す。サーバーのライブラリはそのまま残る。
#[tauri::command]
pub async fn server_disconnect_remote(
    app: tauri::AppHandle,
) -> Result<DesktopServerStatus, String> {
    let server = controller(&app)?;
    server.disconnect_remote().await?;
    Ok(server.status().await)
}

/// 本棚が変わったら自動で送るかどうかを切り替える。
#[tauri::command]
pub async fn server_set_auto_send(
    app: tauri::AppHandle,
    enabled: bool,
) -> Result<DesktopServerStatus, String> {
    let server = controller(&app)?;
    server.set_auto_send(enabled)?;
    Ok(server.status().await)
}

/// 単独サーバーへの送信を後ろで始める。進み具合は状態で返す。
#[tauri::command]
pub async fn server_send_now(app: tauri::AppHandle) -> Result<DesktopServerStatus, String> {
    let server = controller(&app)?;
    let state = library(&app);
    let background = Arc::clone(&server);
    tauri::async_runtime::spawn(async move {
        if let Err(error) = background.send_once(state).await {
            log::warn!("単独サーバーへの送信に失敗しました: {error}");
        }
    });
    Ok(server.status().await)
}

/// 送信を止める。送った分はサーバーに残り、次の送信が続きから使う。
#[tauri::command]
pub async fn server_send_stop(app: tauri::AppHandle) -> Result<DesktopServerStatus, String> {
    let server = controller(&app)?;
    server.stop_send();
    Ok(server.status().await)
}
