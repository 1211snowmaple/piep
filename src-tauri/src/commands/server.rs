//! サーバーモードのコマンド。実体は `crate::server::desktop` にある。

use crate::server::desktop::{
    list_local_interfaces, DesktopServer, DesktopServerStatus, EmbeddedSettings, LocalInterface,
    PairingDetails,
};
use crate::server::DEFAULT_PORT;
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

/// 内蔵サーバーと単独サーバーへの接続の、今の状態を返す。
#[tauri::command]
pub async fn server_status(app: tauri::AppHandle) -> Result<DesktopServerStatus, String> {
    Ok(controller(&app)?.status().await)
}

/// この PC をサーバーにする。設定を記録するので、次の起動でも動き出す。
#[tauri::command]
pub async fn server_embedded_start(
    app: tauri::AppHandle,
    settings: EmbeddedSettings,
) -> Result<DesktopServerStatus, String> {
    let server = controller(&app)?;
    server
        .start_embedded(library(&app), settings, DEFAULT_PORT)
        .await?;
    Ok(server.status().await)
}

/// 内蔵サーバーを止める。次の起動でも動かさない。
#[tauri::command]
pub async fn server_embedded_stop(app: tauri::AppHandle) -> Result<DesktopServerStatus, String> {
    let server = controller(&app)?;
    server.stop_embedded().await?;
    Ok(server.status().await)
}

/// 携帯の登録文字列を発行する。単独サーバーにつないでいれば、そちらで発行する。
#[tauri::command]
pub async fn server_create_reader_pairing(app: tauri::AppHandle) -> Result<PairingDetails, String> {
    controller(&app)?.create_reader_pairing().await
}

/// 端末の登録を解除する。すぐに効く。
#[tauri::command]
pub async fn server_revoke_device(
    app: tauri::AppHandle,
    device_id: String,
) -> Result<DesktopServerStatus, String> {
    let server = controller(&app)?;
    server.revoke_device(&device_id).await?;
    Ok(server.status().await)
}

/// 単独サーバーに登録されている端末の一覧を返す。
#[tauri::command]
pub async fn server_list_remote_devices(
    app: tauri::AppHandle,
) -> Result<Vec<serde_json::Value>, String> {
    controller(&app)?.list_remote_devices().await
}

/// 単独サーバーの版・作品数・索引の進み具合を返す。
#[tauri::command]
pub async fn server_remote_status(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    controller(&app)?.remote_status().await
}

/// 単独サーバーに、新しい版を確かめて当てるよう頼む。
#[tauri::command]
pub async fn server_update_remote(app: tauri::AppHandle) -> Result<(), String> {
    controller(&app)?.update_remote().await
}

/// 単独サーバーに、新しいバージョンがあるかを確かめてもらう。
#[tauri::command]
pub async fn server_check_remote_update(
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    controller(&app)?.check_remote_update().await
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

/// 内蔵サーバーが待ち受けられる、この PC のアドレスを返す。
#[tauri::command]
pub fn server_list_interfaces() -> Result<Vec<LocalInterface>, String> {
    list_local_interfaces()
}
