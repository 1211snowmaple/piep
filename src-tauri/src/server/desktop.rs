//! デスクトップ版の側。内蔵サーバーの開始と停止、単独サーバーへの接続と送信。
//!
//! 設定は Rust の側で持つ。画面を開いていないときの自動送信が読むからである。
//! 管理者の鍵は Windows の資格情報マネージャーに置き、設定ファイルには書かない。

use super::http::{combined_router, ServerContext};
use super::library::Library;
use super::standalone::run_listener;
use super::store::{DeviceSummary, Role, ServerStore};
use super::sync_send::{
    admin_delete, admin_get, admin_post, pair_admin, prepare_snapshot, send_prepared, RemoteServer,
    SendHandle, SendProgress, CANCELED,
};
use super::{pairing_payload, tls, DEFAULT_PORT};
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;

const READER_PAIRING_LIFETIME: Duration = Duration::from_secs(10 * 60);
/// ダウンロードの途中で何度も送らないよう、変わってからこれだけ静かになるのを待つ。
const AUTO_SEND_QUIET: Duration = Duration::from_secs(5 * 60);
const AUTO_SEND_POLL: Duration = Duration::from_secs(30);
/// 起動直後の検索や修復と競わないように待つ。
const AUTO_SEND_STARTUP_DELAY: Duration = Duration::from_secs(2 * 60);
/// 失敗のあとは間を置く。送るたびに本棚の写しを作るので、すぐには繰り返さない。
const AUTO_SEND_RETRY: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct EmbeddedSettings {
    pub enabled: bool,
    /// 待ち受けるアドレス。`behind_proxy` なら 127.0.0.1 の平文。
    pub ip: Option<String>,
    pub behind_proxy: bool,
    /// リバースプロキシで外へ出すときの URL。
    pub public_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct LastSend {
    pub finished_at: Option<String>,
    pub generation: Option<i64>,
    pub works: Option<i64>,
    pub files_added: Option<i64>,
    pub files_removed: Option<i64>,
    pub missing_referenced: Option<i64>,
    pub unchanged: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct DesktopServerSettings {
    pub embedded: EmbeddedSettings,
    pub remote: Option<RemoteServer>,
    pub auto_send: bool,
    pub last_send: LastSend,
}

struct ActiveEmbedded {
    address: SocketAddr,
    url: String,
    fingerprint: Option<String>,
    handle: axum_server::Handle<SocketAddr>,
    task: tokio::task::JoinHandle<()>,
}

/// 送信を頼んだ結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendAttempt {
    /// 別の送信が動いていて、何もしなかった。
    Busy,
    /// 利用者が止めた。
    Canceled,
    /// 最後まで送った。`generation` は写しを作る前のライブラリの世代である。
    Completed { generation: Option<i64> },
}

/// 自動送信がどこまで届けたか。実際に送った世代だけを覚える。
#[derive(Debug)]
struct AutoSendProgress {
    sent_generation: Option<i64>,
    pending_startup_send: bool,
}

impl Default for AutoSendProgress {
    fn default() -> Self {
        Self {
            sent_generation: None,
            pending_startup_send: true,
        }
    }
}

impl AutoSendProgress {
    fn should_send(&self, quiet: bool, generation: Option<i64>) -> bool {
        self.pending_startup_send || (quiet && generation != self.sent_generation)
    }

    fn after_attempt(&mut self, attempt: SendAttempt, current: Option<i64>) {
        match attempt {
            // 動いている送信は今の世代を含まないかもしれない。送ったことにせず、次の周回で試す。
            SendAttempt::Busy => {}
            // 止めた世代を送り続けない。次に変わったら、また送る。
            SendAttempt::Canceled => {
                self.pending_startup_send = false;
                self.sent_generation = current;
            }
            SendAttempt::Completed { generation } => {
                self.pending_startup_send = false;
                self.sent_generation = generation;
            }
        }
    }

    fn after_failure(&mut self) {
        self.pending_startup_send = false;
    }
}

pub struct DesktopServer {
    dir: PathBuf,
    app_data: PathBuf,
    store: Arc<ServerStore>,
    settings: Mutex<DesktopServerSettings>,
    active: AsyncMutex<Option<ActiveEmbedded>>,
    send: SendHandle,
    send_lock: AsyncMutex<()>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopServerStatus {
    pub embedded_running: bool,
    pub embedded_url: Option<String>,
    pub embedded_fingerprint: Option<String>,
    pub embedded: EmbeddedSettings,
    pub devices: Vec<DeviceSummary>,
    pub remote: Option<RemoteServer>,
    pub auto_send: bool,
    pub send: SendProgress,
    pub last_send: LastSend,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingDetails {
    pub payload: String,
    pub expires_in_seconds: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalInterface {
    pub name: String,
    pub ip: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairingText {
    version: u32,
    role: Role,
    url: String,
    secret: String,
    fingerprint: Option<String>,
}

fn save_settings(path: &Path, settings: &DesktopServerSettings) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|error| format!("設定を記録できません: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, bytes).map_err(|error| format!("設定を記録できません: {error}"))?;
    fs::rename(&temporary, path).map_err(|error| format!("設定を確定できません: {error}"))
}

mod credential {
    //! 管理者の鍵は Windows の資格情報マネージャーに置く。
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
        CRED_TYPE_GENERIC,
    };

    const NOT_FOUND_HRESULT: i32 = 0x8007_0490_u32 as i32;

    fn target() -> Vec<u16> {
        "io.github.u1211snowmaple.piep/server-admin-token"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    }

    pub fn store(token: &str) -> Result<(), String> {
        let mut target = target();
        let mut blob = token.as_bytes().to_vec();
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target.as_mut_ptr()),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        unsafe { CredWriteW(&credential, 0) }
            .map_err(|_| "サーバーの鍵を資格情報マネージャーに保存できません".to_string())
    }

    pub fn load() -> Result<Option<String>, String> {
        let target = target();
        let mut pointer: *mut CREDENTIALW = std::ptr::null_mut();
        if let Err(error) = unsafe {
            CredReadW(
                PCWSTR(target.as_ptr()),
                CRED_TYPE_GENERIC,
                None,
                &mut pointer,
            )
        } {
            if error.code().0 == NOT_FOUND_HRESULT {
                return Ok(None);
            }
            return Err("サーバーの鍵を資格情報マネージャーから読めません".into());
        }
        // CredReadW が確保した領域は、中身を読めなくても必ず CredFree する。
        let value = unsafe {
            let credential = &*pointer;
            let bytes = std::slice::from_raw_parts(
                credential.CredentialBlob,
                credential.CredentialBlobSize as usize,
            );
            String::from_utf8(bytes.to_vec())
        };
        unsafe { CredFree(pointer as *const _) };
        value
            .map(Some)
            .map_err(|_| "保存されたサーバーの鍵を読み取れません".to_string())
    }

    pub fn delete() -> Result<(), String> {
        let target = target();
        match unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()),
            Err(error) if error.code().0 == NOT_FOUND_HRESULT => Ok(()),
            Err(_) => Err("サーバーの鍵を資格情報マネージャーから消せません".into()),
        }
    }
}

impl DesktopServer {
    pub fn new(app_data: &Path) -> Result<Self, String> {
        let dir = app_data.join("server");
        fs::create_dir_all(&dir)
            .map_err(|error| format!("サーバーの置き場を作れません: {error}"))?;
        let store = Arc::new(ServerStore::open(&dir.join("state.db"))?);
        let settings = fs::read(dir.join("settings.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Ok(Self {
            dir,
            app_data: app_data.to_path_buf(),
            store,
            settings: Mutex::new(settings),
            active: AsyncMutex::new(None),
            send: SendHandle::new(),
            send_lock: AsyncMutex::new(()),
        })
    }

    fn settings(&self) -> DesktopServerSettings {
        self.settings
            .lock()
            .map(|settings| settings.clone())
            .unwrap_or_default()
    }

    fn change_settings(
        &self,
        change: impl FnOnce(&mut DesktopServerSettings),
    ) -> Result<(), String> {
        let mut settings = self.settings.lock().map_err(|_| "設定を開けません")?;
        change(&mut settings);
        save_settings(&self.dir.join("settings.json"), &settings)
    }

    pub async fn status(&self) -> DesktopServerStatus {
        let settings = self.settings();
        let active = self.active.lock().await;
        let running = active.as_ref().filter(|server| !server.task.is_finished());
        DesktopServerStatus {
            embedded_running: running.is_some(),
            embedded_url: running.map(|server| server.url.clone()),
            embedded_fingerprint: running.and_then(|server| server.fingerprint.clone()),
            embedded: settings.embedded,
            devices: self.store.list_devices().unwrap_or_default(),
            remote: settings.remote,
            auto_send: settings.auto_send,
            send: self
                .send
                .progress
                .lock()
                .map(|progress| progress.clone())
                .unwrap_or_default(),
            last_send: settings.last_send,
        }
    }

    pub async fn start_embedded(
        &self,
        library: Arc<AppState>,
        settings: EmbeddedSettings,
        port: u16,
    ) -> Result<(), String> {
        let mut active = self.active.lock().await;
        if active
            .as_ref()
            .is_some_and(|server| !server.task.is_finished())
        {
            return Err("サーバーはすでに動いています".into());
        }
        *active = None;
        let public_url = settings
            .public_url
            .clone()
            .filter(|url| !url.trim().is_empty());
        if public_url
            .as_ref()
            .is_some_and(|url| !url.starts_with("https://"))
        {
            return Err("公開 URL は https:// で始めてください".into());
        }
        let (address, material) = if settings.behind_proxy {
            (SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port), None)
        } else {
            let ip: Ipv4Addr = settings
                .ip
                .as_deref()
                .and_then(|ip| ip.parse().ok())
                .ok_or("待ち受ける IPv4 アドレスを選んでください")?;
            if !ip.is_private() && !ip.is_loopback() {
                return Err("この PC のプライベート IPv4 アドレスを選んでください".into());
            }
            (
                SocketAddr::new(IpAddr::V4(ip), port),
                Some(tls::self_signed(&self.dir.join("tls")).await?),
            )
        };
        let fingerprint = material
            .as_ref()
            .and_then(|material| material.fingerprint.clone());
        let tls_config = material.map(|material| material.config);
        let url = public_url.clone().unwrap_or_else(|| {
            if tls_config.is_some() {
                format!("https://{address}/")
            } else {
                format!("http://{address}/")
            }
        });
        let context = ServerContext::new(
            Library::App(library),
            Arc::clone(&self.store),
            self.app_data.clone(),
            None,
            public_url,
            fingerprint.clone(),
        );
        let router = combined_router(context);
        let handle = axum_server::Handle::new();
        // 実際のポートと、待ち受けの失敗をここで確かめる。
        let probe = std::net::TcpListener::bind(address)
            .map_err(|error| format!("{address} で待ち受けられません: {error}"))?;
        let address = probe
            .local_addr()
            .map_err(|error| format!("{address} を確認できません: {error}"))?;
        drop(probe);
        let run_handle = handle.clone();
        let task = tokio::spawn(async move {
            if let Err(error) = run_listener(address, tls_config, router, run_handle).await {
                log::error!("内蔵サーバーが止まりました: {error}");
            }
        });
        let url = if port == 0 {
            url.replace(":0/", &format!(":{}/", address.port()))
        } else {
            url
        };
        *active = Some(ActiveEmbedded {
            address,
            url,
            fingerprint,
            handle,
            task,
        });
        drop(active);
        self.change_settings(|current| {
            current.embedded = EmbeddedSettings {
                enabled: true,
                ..settings
            };
        })
    }

    pub async fn stop_embedded(&self) -> Result<(), String> {
        let mut active = self.active.lock().await;
        if let Some(server) = active.take() {
            log::info!("内蔵サーバーを止めます: {}", server.address);
            server.handle.shutdown();
            let mut task = server.task;
            if tokio::time::timeout(Duration::from_secs(2), &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
        }
        drop(active);
        self.change_settings(|current| current.embedded.enabled = false)
    }

    /// 携帯の登録文字列。単独サーバーにつないでいればそちらで、無ければ内蔵サーバーで発行する。
    pub async fn create_reader_pairing(&self) -> Result<PairingDetails, String> {
        if let Some(remote) = self.settings().remote {
            let token =
                credential::load()?.ok_or("サーバーの鍵がありません。登録し直してください")?;
            let body = admin_post(
                &remote,
                &token,
                "pairings",
                &serde_json::json!({ "url": remote.url }),
            )
            .await?;
            let payload = body
                .get("payload")
                .and_then(|value| value.as_str())
                .ok_or("サーバーの応答を読み取れません")?
                .to_string();
            let expires_in_seconds = body
                .get("expiresInSeconds")
                .and_then(|value| value.as_u64())
                .unwrap_or(600);
            return Ok(PairingDetails {
                payload,
                expires_in_seconds,
            });
        }
        let active = self.active.lock().await;
        let server = active
            .as_ref()
            .filter(|server| !server.task.is_finished())
            .ok_or("サーバーを動かしてから追加してください")?;
        if !server.url.starts_with("https://") {
            return Err("リバースプロキシの裏で動かすときは、公開 URL を設定してください".into());
        }
        let secret = self
            .store
            .issue_pairing(Role::Reader, READER_PAIRING_LIFETIME)?;
        Ok(PairingDetails {
            payload: pairing_payload(
                Role::Reader,
                &server.url,
                server.fingerprint.as_deref(),
                &secret,
            ),
            expires_in_seconds: READER_PAIRING_LIFETIME.as_secs(),
        })
    }

    pub async fn list_remote_devices(&self) -> Result<Vec<serde_json::Value>, String> {
        let remote = self
            .settings()
            .remote
            .ok_or("単独サーバーにつないでいません")?;
        let token = credential::load()?.ok_or("サーバーの鍵がありません。登録し直してください")?;
        let body = admin_get(&remote, &token, "devices").await?;
        Ok(body.as_array().cloned().unwrap_or_default())
    }

    pub async fn remote_status(&self) -> Result<serde_json::Value, String> {
        let remote = self
            .settings()
            .remote
            .ok_or("単独サーバーにつないでいません")?;
        let token = credential::load()?.ok_or("サーバーの鍵がありません。登録し直してください")?;
        admin_get(&remote, &token, "status").await
    }

    /// デスクトップ版の方が新しければ、サーバーにも更新を頼む。
    async fn update_remote_if_behind(&self) {
        let Ok(status) = self.remote_status().await else {
            return;
        };
        let behind = status["version"]
            .as_str()
            .is_some_and(|version| super::update::is_newer(env!("CARGO_PKG_VERSION"), version));
        if behind && status["update"].is_object() {
            if let Err(error) = self.update_remote().await {
                log::warn!("サーバーの更新を頼めません: {error}");
            }
        }
    }

    pub async fn update_remote(&self) -> Result<(), String> {
        let remote = self
            .settings()
            .remote
            .ok_or("単独サーバーにつないでいません")?;
        let token = credential::load()?.ok_or("サーバーの鍵がありません。登録し直してください")?;
        admin_post(&remote, &token, "update", &serde_json::json!({})).await?;
        Ok(())
    }

    pub async fn revoke_device(&self, device_id: &str) -> Result<(), String> {
        if let Some(remote) = self.settings().remote {
            let token =
                credential::load()?.ok_or("サーバーの鍵がありません。登録し直してください")?;
            return admin_delete(&remote, &token, &format!("devices/{device_id}")).await;
        }
        if self.store.revoke_device(device_id)? {
            Ok(())
        } else {
            Err("端末が見つかりません".into())
        }
    }

    pub async fn connect_remote(&self, pairing: &str, device_name: &str) -> Result<(), String> {
        let parsed: PairingText = serde_json::from_str(pairing.trim())
            .map_err(|_| "登録文字列を読み取れません".to_string())?;
        if parsed.version != 2 {
            return Err("この版の登録文字列には対応していません".into());
        }
        if parsed.role != Role::Admin {
            return Err("これは携帯用の登録文字列です。サーバーの piep-server init か admin-pair の出力を貼ってください".into());
        }
        let remote = RemoteServer {
            url: parsed.url,
            fingerprint: parsed.fingerprint,
        };
        let token = pair_admin(&remote, &parsed.secret, device_name).await?;
        credential::store(&token)?;
        self.change_settings(|settings| {
            settings.remote = Some(remote);
            settings.last_send = LastSend::default();
        })
    }

    pub async fn disconnect_remote(&self) -> Result<(), String> {
        self.send.cancel.store(true, Ordering::Relaxed);
        let _running = self.send_lock.lock().await;
        credential::delete()?;
        self.change_settings(|settings| {
            settings.remote = None;
            settings.auto_send = false;
            settings.last_send = LastSend::default();
        })
    }

    pub fn set_auto_send(&self, enabled: bool) -> Result<(), String> {
        self.change_settings(|settings| settings.auto_send = enabled)
    }

    pub fn stop_send(&self) {
        self.send.cancel.store(true, Ordering::Relaxed);
    }

    /// 送信を 1 回行う。すでに送っていれば何もしない。
    pub async fn send_once(&self, state: Arc<AppState>) -> Result<SendAttempt, String> {
        let Ok(_running) = self.send_lock.try_lock() else {
            return Ok(SendAttempt::Busy);
        };
        // 写しを作る前に読む。この間に変わっても、次回もう一度送るだけで取りこぼさない。
        let generation = state.db.library_generation().ok();
        let remote = self
            .settings()
            .remote
            .ok_or("単独サーバーにつないでいません")?;
        let token = credential::load()?.ok_or("サーバーの鍵がありません。登録し直してください")?;
        self.send.cancel.store(false, Ordering::Relaxed);
        if let Ok(mut progress) = self.send.progress.lock() {
            *progress = SendProgress {
                running: true,
                phase: "snapshot".into(),
                ..Default::default()
            };
        }
        let snapshot = self.dir.join("outgoing").join("piep-snapshot.db");
        let result = async {
            let (report, files, skipped) =
                prepare_snapshot(&state, &self.app_data, &snapshot).await?;
            if let Ok(mut progress) = self.send.progress.lock() {
                progress.outside_paths = report.outside;
                progress.skipped_names = skipped;
            }
            send_prepared(&remote, &token, &snapshot, files, &self.send).await
        }
        .await;
        let _ = fs::remove_file(&snapshot);
        let finished_at = Some(chrono::Utc::now().to_rfc3339());
        let (phase, record) = match result {
            Ok(outcome) => (
                "done",
                LastSend {
                    finished_at,
                    generation: Some(outcome.generation),
                    works: Some(outcome.works),
                    files_added: Some(outcome.files_added),
                    files_removed: Some(outcome.files_removed),
                    missing_referenced: Some(outcome.missing_referenced),
                    unchanged: outcome.unchanged,
                    error: None,
                },
            ),
            Err(error) if error == CANCELED => (
                "canceled",
                LastSend {
                    finished_at,
                    error: Some(error),
                    ..Default::default()
                },
            ),
            Err(error) => {
                log::warn!("単独サーバーへの送信に失敗しました: {error}");
                (
                    "failed",
                    LastSend {
                        finished_at,
                        error: Some(error),
                        ..Default::default()
                    },
                )
            }
        };
        if let Ok(mut progress) = self.send.progress.lock() {
            progress.running = false;
            progress.phase = phase.into();
            progress.message = record.error.clone();
        }
        let failed = record.error.clone();
        self.change_settings(|settings| settings.last_send = record)?;
        if phase == "done" {
            self.update_remote_if_behind().await;
        }
        match failed {
            Some(error) if error == CANCELED => Ok(SendAttempt::Canceled),
            Some(error) => Err(error),
            None => Ok(SendAttempt::Completed { generation }),
        }
    }

    pub fn start_background(self: &Arc<Self>, state: Arc<AppState>) {
        let server = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let settings = server.settings();
            if settings.embedded.enabled {
                let port = DEFAULT_PORT;
                if let Err(error) = server
                    .start_embedded(Arc::clone(&state), settings.embedded, port)
                    .await
                {
                    log::warn!("内蔵サーバーを始められません: {error}");
                }
            }
            tokio::time::sleep(AUTO_SEND_STARTUP_DELAY).await;
            // 起動して落ち着いたあとに一度送る。変わっていなければ問い合わせだけで終わる。
            let mut progress = AutoSendProgress::default();
            let mut seen = None;
            let mut changed_at = None;
            let mut retry_at: Option<Instant> = None;
            loop {
                if server.settings().auto_send && server.settings().remote.is_some() {
                    let generation = state.db.library_generation().ok();
                    if generation != seen {
                        seen = generation;
                        changed_at = Some(Instant::now());
                    }
                    let quiet =
                        changed_at.is_none_or(|at: Instant| at.elapsed() >= AUTO_SEND_QUIET);
                    let waited = retry_at.is_none_or(|at| Instant::now() >= at);
                    if waited && progress.should_send(quiet, generation) {
                        match server.send_once(Arc::clone(&state)).await {
                            Ok(attempt) => {
                                progress.after_attempt(attempt, generation);
                                retry_at = None;
                            }
                            Err(error) => {
                                progress.after_failure();
                                log::warn!("自動送信に失敗しました: {error}");
                                retry_at = Some(Instant::now() + AUTO_SEND_RETRY);
                            }
                        }
                    }
                }
                tokio::time::sleep(AUTO_SEND_POLL).await;
            }
        });
    }
}

pub fn list_local_interfaces() -> Result<Vec<LocalInterface>, String> {
    let preferred = local_ip_address::local_ip().ok();
    let mut interfaces = local_ip_address::list_afinet_netifas()
        .map_err(|error| format!("PC のネットワークを確認できません: {error}"))?
        .into_iter()
        .filter_map(|(name, address)| match address {
            IpAddr::V4(ip) if ip.is_private() || ip.is_loopback() => Some(LocalInterface {
                name,
                ip: ip.to_string(),
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    interfaces.push(LocalInterface {
        name: "この PC のみ（Android エミュレーター）".into(),
        ip: Ipv4Addr::LOCALHOST.to_string(),
    });
    interfaces.sort_by(|left, right| {
        let left_preferred = preferred.is_some_and(|ip| ip.to_string() == left.ip);
        let right_preferred = preferred.is_some_and(|ip| ip.to_string() == right.ip);
        right_preferred
            .cmp(&left_preferred)
            .then_with(|| left.ip.cmp(&right.ip))
    });
    interfaces.dedup_by(|left, right| left.ip == right.ip);
    Ok(interfaces)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_busy_attempt_does_not_count_as_sent() {
        let mut progress = AutoSendProgress::default();
        assert!(progress.should_send(false, Some(1)));
        // 手で始めた g1 の送信中に g2 へ変わり、自動送信はロックで見送られる。
        progress.after_attempt(SendAttempt::Busy, Some(2));
        assert!(progress.should_send(true, Some(2)));
        progress.after_attempt(
            SendAttempt::Completed {
                generation: Some(2),
            },
            Some(2),
        );
        assert!(!progress.should_send(true, Some(2)));
    }

    #[test]
    fn the_generation_written_during_a_send_is_still_owed() {
        let mut progress = AutoSendProgress::default();
        // g1 の写しを送っている間に g2 になった。
        progress.after_attempt(
            SendAttempt::Completed {
                generation: Some(1),
            },
            Some(2),
        );
        assert!(progress.should_send(true, Some(2)));
    }

    #[test]
    fn a_canceled_generation_waits_for_the_next_change() {
        let mut progress = AutoSendProgress::default();
        progress.after_attempt(SendAttempt::Canceled, Some(3));
        assert!(!progress.should_send(true, Some(3)));
        assert!(progress.should_send(true, Some(4)));
    }

    #[test]
    fn a_failure_keeps_the_generation_owed() {
        let mut progress = AutoSendProgress::default();
        progress.after_failure();
        assert!(!progress.pending_startup_send);
        assert!(progress.should_send(true, Some(1)));
    }
}
