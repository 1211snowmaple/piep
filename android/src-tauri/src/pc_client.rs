//! piep サーバーへの通信を Android の Rust 側に閉じ込める。
//! 認証鍵は WebView の JavaScript に返さない。

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use piep_mobile_api::{MobileDateField, MobileEntitySort, MobileSearchRequest, MobileSortOrder};
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::State;
use url::Url;

use crate::secret_cipher::SecretCipher;
// サーバーから受け取る型。サーバーと同じ定義（crates/piep-mobile-api）を使う。
pub use piep_mobile_api::{MobileCollection, MobilePage, MobilePerson, MobileSeriesMembership};

pub mod cover_protocol;
pub(crate) mod device_db;
mod image_validation;
pub(crate) mod local_reading;
pub(crate) mod offline;
pub(crate) mod shelf;
pub(crate) mod shelf_reader;
pub(crate) mod shelf_reading;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
const ASSET_TIMEOUT: Duration = Duration::from_secs(60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
// 回線が切り替わると、使い回す接続は黙って死ぬ。長く持ち越さない。
// 読んでいるあいだは数十秒おきにしか要求しない。そのたびに接続し直すと、ページ送りごとに TLS の往復が増える。
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
// HTTP/2 は一本の接続に要求を載せる。切れた接続に気づかないと全部が待たされるので、確かめ続ける。
const HTTP2_PING_INTERVAL: Duration = Duration::from_secs(15);
const HTTP2_PING_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ASSET_BYTES: usize = 25 * 1024 * 1024;
const MAX_JSON_BYTES: usize = 16 * 1024 * 1024;
const API_VERSION: u32 = 2;
const PAIRING_VERSION: u32 = 2;

pub struct PcClientState {
    file: PathBuf,
    cipher: Arc<dyn SecretCipher>,
    saved: Mutex<Option<SavedConnection>>,
    // 保存直前の確認から履歴の永続化まで、登録変更と排他にする。
    connection_generation: Mutex<u64>,
    shelf_scope: Mutex<Option<shelf::ShelfScope>>,
    // 要求ごとに TLS を張り直すと、外からの接続では 1 枚の画像ごとに往復が増える。
    client: Mutex<Option<(String, reqwest::Client)>>,
}

impl PcClientState {
    pub fn new(file: PathBuf, cipher: Arc<dyn SecretCipher>) -> Self {
        Self {
            file,
            cipher,
            saved: Mutex::new(None),
            connection_generation: Mutex::new(0),
            shelf_scope: Mutex::new(None),
            client: Mutex::new(None),
        }
    }

    /// 接続先に合う検証方法のクライアントを返す。同じ接続先のあいだは使い回す。
    fn client(&self, saved: &SavedConnection) -> Result<reqwest::Client, ClientError> {
        let key = format!(
            "{}|{}",
            saved.url,
            saved.fingerprint.as_deref().unwrap_or("")
        );
        let mut cache = self.client.lock().map_err(|_| ClientError::storage())?;
        if let Some((cached, client)) = cache.as_ref()
            && *cached == key
        {
            return Ok(client.clone());
        }
        let client = match &saved.certificate_der {
            Some(certificate) => pinned_client(
                &URL_SAFE_NO_PAD
                    .decode(certificate)
                    .map_err(|_| ClientError::storage())?,
            )?,
            None => trusted_client()?,
        };
        *cache = Some((key, client.clone()));
        Ok(client)
    }

    fn sealed_file(&self) -> PathBuf {
        self.file.with_extension("sealed")
    }

    fn load(&self) -> Result<Option<SavedConnection>, ClientError> {
        // Keystore への問い合わせは数ミリ秒かかるので、画像ごとに開かない。
        let mut cache = self.saved.lock().map_err(|_| ClientError::storage())?;
        if let Some(saved) = cache.as_ref() {
            return Ok(Some(saved.clone()));
        }
        let saved = match fs::read(self.sealed_file()) {
            Ok(sealed) => {
                let bytes = self
                    .cipher
                    .open(&sealed)
                    .map_err(|_| ClientError::secret_lost())?;
                parse_saved(&bytes)?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ClientError::storage()),
        };
        *cache = Some(saved.clone());
        Ok(Some(saved))
    }

    fn save(&self, saved: &SavedConnection) -> Result<(), ClientError> {
        let mut generation = self
            .connection_generation
            .lock()
            .map_err(|_| ClientError::storage())?;
        let mut cache = self.saved.lock().map_err(|_| ClientError::storage())?;
        self.write_sealed(saved)?;
        *cache = Some(saved.clone());
        *generation += 1;
        Ok(())
    }

    fn connection_snapshot(&self) -> Result<(u64, Option<SavedConnection>), ClientError> {
        let generation = self
            .connection_generation
            .lock()
            .map_err(|_| ClientError::storage())?;
        Ok((*generation, self.load()?))
    }

    fn with_connection<T>(
        &self,
        expected: u64,
        action: impl FnOnce() -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        let generation = self
            .connection_generation
            .lock()
            .map_err(|_| ClientError::storage())?;
        if *generation != expected {
            return Err(ClientError {
                code: "connection_changed",
                message: "接続先が変更されました。現在の接続先で同期し直してください",
            });
        }
        action()
    }

    fn refresh_capabilities(
        &self,
        expected: u64,
        capabilities: Vec<String>,
    ) -> Result<(), ClientError> {
        let _generation = self
            .connection_generation
            .lock()
            .map_err(|_| ClientError::storage())?;
        if *_generation != expected {
            return Ok(());
        }
        let mut cache = self.saved.lock().map_err(|_| ClientError::storage())?;
        if let Some(current) = cache.as_mut() {
            let mut updated = current.clone();
            updated.capabilities = capabilities;
            self.write_sealed(&updated)?;
            *current = updated;
        }
        Ok(())
    }

    fn write_sealed(&self, saved: &SavedConnection) -> Result<(), ClientError> {
        let bytes = serde_json::to_vec(saved).map_err(|_| ClientError::storage())?;
        let sealed = self
            .cipher
            .seal(&bytes)
            .map_err(|_| ClientError::storage())?;
        let target = self.sealed_file();
        let temporary = target.with_extension("sealed.tmp");
        fs::write(&temporary, sealed).map_err(|_| ClientError::storage())?;
        fs::rename(&temporary, &target).map_err(|_| ClientError::storage())?;
        Ok(())
    }

    fn forget(&self) -> Result<(), ClientError> {
        let mut generation = self
            .connection_generation
            .lock()
            .map_err(|_| ClientError::storage())?;
        let mut cache = self.saved.lock().map_err(|_| ClientError::storage())?;
        remove_if_present(&self.sealed_file())?;
        *cache = None;
        *generation += 1;
        Ok(())
    }
}

fn remove_if_present(file: &std::path::Path) -> Result<(), ClientError> {
    match fs::remove_file(file) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ClientError::storage()),
    }
}

fn parse_saved(bytes: &[u8]) -> Result<SavedConnection, ClientError> {
    let saved: SavedConnection =
        serde_json::from_slice(bytes).map_err(|_| ClientError::storage())?;
    validate_url(&saved.url)?;
    match (&saved.fingerprint, &saved.certificate_der) {
        (Some(expected), Some(certificate)) => {
            validate_fingerprint(expected)?;
            let certificate = URL_SAFE_NO_PAD
                .decode(certificate)
                .map_err(|_| ClientError::storage())?;
            if fingerprint(&certificate) != *expected {
                return Err(ClientError::storage());
            }
        }
        (None, None) => {}
        _ => return Err(ClientError::storage()),
    }
    if URL_SAFE_NO_PAD
        .decode(&saved.token)
        .map_err(|_| ClientError::storage())?
        .len()
        != 32
    {
        return Err(ClientError::storage());
    }
    Ok(saved)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientError {
    code: &'static str,
    message: &'static str,
}

impl ClientError {
    fn invalid_pairing() -> Self {
        Self {
            code: "invalid_pairing",
            message: "登録文字列を確認してください",
        }
    }
    fn secret_lost() -> Self {
        Self {
            code: "secret_lost",
            message: "この端末に保存した登録を読めません。登録し直してください",
        }
    }
    fn storage() -> Self {
        Self {
            code: "storage",
            message: "端末内の接続設定を読み書きできません",
        }
    }
    fn device_records() -> Self {
        Self {
            code: "device_records",
            message: "端末内の読書の記録を読み書きできません。空き容量を確認してください",
        }
    }
    fn offline_storage() -> Self {
        Self {
            code: "offline_storage",
            message: "保存した作品を読み書きできません。空き容量と保存状態を確認してください",
        }
    }
    fn admin_pairing() -> Self {
        Self {
            code: "invalid_pairing",
            message: "これは管理者用の登録文字列です。デスクトップ版の「携帯を追加」で出した文字列を使ってください",
        }
    }
    fn offline() -> Self {
        Self {
            code: "offline",
            message: "サーバーに接続できません。通信状態と、サーバーが動いているかを確認してください",
        }
    }
    fn certificate_changed() -> Self {
        Self {
            code: "certificate_changed",
            message: "サーバーの証明書が登録時と異なります",
        }
    }
    fn certificate_untrusted() -> Self {
        Self {
            code: "certificate_untrusted",
            message: "サーバーの証明書を確認できません。URL が正しいか確かめてください",
        }
    }
    fn protocol() -> Self {
        Self {
            code: "protocol_mismatch",
            message: "サーバーの接続 API と互換性がありません。アプリとサーバーを、どちらも新しい版にしてください",
        }
    }
    fn revoked() -> Self {
        Self {
            code: "revoked",
            message: "この端末の登録がサーバー側で解除されています",
        }
    }
    fn not_ready() -> Self {
        Self {
            code: "not_ready",
            message: "サーバーは本棚の準備中です。しばらくしてからもう一度試してください",
        }
    }
    fn remote() -> Self {
        Self {
            code: "remote_error",
            message: "サーバーの作品を読み込めません",
        }
    }
    /// 通信の失敗を、利用者が次に何をすべきかで分ける。
    fn from_send(error: &reqwest::Error, pinned: bool) -> Self {
        if !pinned && rejected_certificate(error) {
            Self::certificate_untrusted()
        } else {
            Self::offline()
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PairingPayload {
    version: u32,
    role: String,
    url: String,
    secret: String,
    #[serde(default)]
    fingerprint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedConnection {
    url: String,
    /// 自己署名の接続先だけが持つ。無ければ OS が信頼する CA で検証する。
    #[serde(default)]
    fingerprint: Option<String>,
    #[serde(default)]
    certificate_der: Option<String>,
    token: String,
    device_id: String,
    device_name: String,
    #[serde(default)]
    capabilities: Vec<String>,
}

impl SavedConnection {
    fn pinned(&self) -> bool {
        self.fingerprint.is_some()
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    paired: bool,
    reachable: bool,
    url: Option<String>,
    device_name: Option<String>,
    issue: Option<&'static str>,
    capabilities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    shelf: Option<shelf::ShelfStatus>,
}

impl ConnectionStatus {
    fn empty() -> Self {
        Self {
            paired: false,
            reachable: false,
            url: None,
            device_name: None,
            issue: None,
            capabilities: Vec::new(),
            shelf: None,
        }
    }
    fn from_saved(saved: &SavedConnection, reachable: bool, issue: Option<&'static str>) -> Self {
        Self {
            paired: true,
            reachable,
            url: Some(saved.url.clone()),
            device_name: Some(saved.device_name.clone()),
            issue,
            capabilities: saved.capabilities.clone(),
            shelf: None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hello {
    api_version: u32,
    server_name: String,
    #[serde(default)]
    capabilities: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairResult {
    device_id: String,
    token: String,
    api_version: u32,
}

fn validate_url(value: &str) -> Result<Url, ClientError> {
    let url = Url::parse(value).map_err(|_| ClientError::invalid_pairing())?;
    if url.scheme() != "https"
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ClientError::invalid_pairing());
    }
    Ok(url)
}

/// 登録文字列を読む。通信の前に確かめられることをここで全部確かめる。
fn parse_pairing(payload: &str, device_name: &str) -> Result<PairingPayload, ClientError> {
    let mut pairing: PairingPayload =
        serde_json::from_str(payload).map_err(|_| ClientError::invalid_pairing())?;
    if pairing.version != PAIRING_VERSION
        || device_name.trim().is_empty()
        || device_name.chars().count() > 64
    {
        return Err(ClientError::invalid_pairing());
    }
    match pairing.role.as_str() {
        "reader" => {}
        "admin" => return Err(ClientError::admin_pairing()),
        _ => return Err(ClientError::invalid_pairing()),
    }
    // 端点は `{url}/api/...` で組み立てるので、末尾の / を落として持つ。
    pairing.url = validate_url(&pairing.url)?
        .as_str()
        .trim_end_matches('/')
        .to_string();
    if let Some(expected) = &pairing.fingerprint {
        validate_fingerprint(expected)?;
        pairing.fingerprint = Some(expected.to_ascii_lowercase());
    }
    if URL_SAFE_NO_PAD
        .decode(&pairing.secret)
        .map_err(|_| ClientError::invalid_pairing())?
        .len()
        != 32
    {
        return Err(ClientError::invalid_pairing());
    }
    Ok(pairing)
}

fn validate_fingerprint(value: &str) -> Result<(), ClientError> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(ClientError::invalid_pairing())
    }
}

fn fingerprint(certificate: &[u8]) -> String {
    Sha256::digest(certificate)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn initialize_crypto() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

fn probe_client() -> Result<reqwest::Client, ClientError> {
    initialize_crypto();
    reqwest::Client::builder()
        .tls_backend_rustls()
        .tls_danger_accept_invalid_certs(true)
        .tls_info(true)
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(PROBE_TIMEOUT)
        .build()
        .map_err(|_| ClientError::offline())
}

fn pinned_client(certificate: &[u8]) -> Result<reqwest::Client, ClientError> {
    initialize_crypto();
    let certificate =
        reqwest::Certificate::from_der(certificate).map_err(|_| ClientError::storage())?;
    reqwest::Client::builder()
        .tls_backend_rustls()
        .tls_certs_only([certificate])
        // 接続先は IP、共有証明書はアドレス変更後も同じものを使う。
        // 信頼する証明書は直前に指紋を照合した 1 枚に限定する。
        .tls_danger_accept_invalid_hostnames(true)
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(CONNECT_TIMEOUT)
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .http2_keep_alive_interval(HTTP2_PING_INTERVAL)
        .http2_keep_alive_timeout(HTTP2_PING_TIMEOUT)
        .http2_keep_alive_while_idle(true)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| ClientError::certificate_changed())
}

/// 指紋の無い接続先用。OS が信頼する CA と host 名で検証する。
fn trusted_client() -> Result<reqwest::Client, ClientError> {
    initialize_crypto();
    let builder = reqwest::Client::builder().tls_backend_rustls();
    #[cfg(target_os = "android")]
    let builder = builder.tls_certs_only(android_system_certificates()?.iter().cloned());
    builder
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(CONNECT_TIMEOUT)
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .http2_keep_alive_interval(HTTP2_PING_INTERVAL)
        .http2_keep_alive_timeout(HTTP2_PING_TIMEOUT)
        .http2_keep_alive_while_idle(true)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| ClientError::offline())
}

/// アプリ内更新の取得に使う。配布先は別のホストへ転送し、大きいファイルも取るので、
/// サーバーとの通信とは別に作る。
pub(crate) fn update_client(allow_plain_http: bool) -> Result<reqwest::Client, String> {
    initialize_crypto();
    let unavailable = |_| "更新を確かめられません".to_string();
    let builder = reqwest::Client::builder().tls_backend_rustls();
    #[cfg(target_os = "android")]
    let builder = builder.tls_certs_only(
        android_system_certificates()
            .map_err(|_| "更新を確かめられません".to_string())?
            .iter()
            .cloned(),
    );
    builder
        .https_only(!allow_plain_http)
        .redirect(reqwest::redirect::Policy::limited(5))
        .no_proxy()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(unavailable)
}

/// Android の信頼する CA を読む。
///
/// reqwest の既定の検証器は Android では JNI の初期化と Kotlin 側の部品を要求する。
/// 同じ情報は端末のファイルにあり、アプリが読める。利用者が足した CA は、Android の
/// 既定どおりアプリでは信頼しない。
#[cfg(target_os = "android")]
fn android_system_certificates() -> Result<&'static Vec<reqwest::Certificate>, ClientError> {
    static CERTIFICATES: std::sync::OnceLock<Vec<reqwest::Certificate>> =
        std::sync::OnceLock::new();
    let certificates = CERTIFICATES.get_or_init(|| {
        // Android 14 以降は更新可能な apex 側が本物で、system 側は古いまま残る。
        for directory in [
            "/apex/com.android.conscrypt/cacerts",
            "/system/etc/security/cacerts",
        ] {
            let Ok(entries) = fs::read_dir(directory) else {
                continue;
            };
            let certificates: Vec<_> = entries
                .flatten()
                .filter_map(|entry| fs::read(entry.path()).ok())
                .filter_map(|pem| reqwest::Certificate::from_pem(&pem).ok())
                .collect();
            if !certificates.is_empty() {
                return certificates;
            }
        }
        Vec::new()
    });
    if certificates.is_empty() {
        return Err(ClientError::certificate_untrusted());
    }
    Ok(certificates)
}

/// TLS の証明書で断られたかどうかを、エラーの連なりから見分ける。
fn rejected_certificate(error: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(cause) = current {
        // io::Error は二重に包まれて届き、source() は包んだ中身を飛ばす。直接ほどく。
        let mut inner: &(dyn std::error::Error + 'static) = cause;
        while let Some(wrapped) = inner
            .downcast_ref::<std::io::Error>()
            .and_then(|io| io.get_ref())
        {
            inner = wrapped;
        }
        if matches!(
            inner.downcast_ref::<rustls::Error>(),
            Some(rustls::Error::InvalidCertificate(_))
        ) {
            return true;
        }
        current = cause.source();
    }
    false
}

async fn hello(
    client: &reqwest::Client,
    base: &str,
    pinned: bool,
) -> Result<reqwest::Response, ClientError> {
    let response = client
        .get(format!("{base}/api/hello"))
        .send()
        .await
        .map_err(|error| ClientError::from_send(&error, pinned))?;
    if !response.status().is_success() {
        return Err(ClientError::protocol());
    }
    Ok(response)
}

fn certificate_from_response(response: &reqwest::Response) -> Result<Vec<u8>, ClientError> {
    response
        .extensions()
        .get::<reqwest::tls::TlsInfo>()
        .and_then(|info| info.peer_certificate())
        .map(|certificate| certificate.to_vec())
        .ok_or_else(ClientError::certificate_changed)
}

#[tauri::command]
pub async fn pc_pair(
    state: State<'_, PcClientState>,
    payload: String,
    device_name: String,
) -> Result<ConnectionStatus, ClientError> {
    let pairing = parse_pairing(&payload, &device_name)?;
    let pinned = pairing.fingerprint.is_some();
    let (client, certificate, response) = match &pairing.fingerprint {
        Some(expected) => {
            // 無認証の接続確認だけを先に行い、登録鍵を送る前に証明書を照合する。
            let response = hello(&probe_client()?, &pairing.url, true).await?;
            let certificate = certificate_from_response(&response)?;
            if fingerprint(&certificate) != *expected {
                return Err(ClientError::certificate_changed());
            }
            (pinned_client(&certificate)?, Some(certificate), response)
        }
        None => {
            let client = trusted_client()?;
            let response = hello(&client, &pairing.url, false).await?;
            (client, None, response)
        }
    };
    let hello: Hello = response.bounded_json().await?;
    if hello.api_version != API_VERSION || hello.server_name != "piep" {
        return Err(ClientError::protocol());
    }
    let response = client
        .post(format!("{}/api/pair", pairing.url))
        .json(&serde_json::json!({"secret": pairing.secret, "deviceName": device_name.trim()}))
        .send()
        .await
        .map_err(|error| ClientError::from_send(&error, pinned))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(ClientError::invalid_pairing());
    }
    if !response.status().is_success() {
        return Err(ClientError::remote());
    }
    let result: PairResult = response.bounded_json().await?;
    if result.api_version != API_VERSION
        || URL_SAFE_NO_PAD
            .decode(&result.token)
            .map_err(|_| ClientError::protocol())?
            .len()
            != 32
    {
        return Err(ClientError::protocol());
    }
    let saved = SavedConnection {
        url: pairing.url,
        fingerprint: pairing.fingerprint,
        certificate_der: certificate.map(|certificate| URL_SAFE_NO_PAD.encode(certificate)),
        token: result.token,
        device_id: result.device_id,
        device_name: device_name.trim().to_string(),
        capabilities: hello.capabilities,
    };
    state.save(&saved)?;
    Ok(ConnectionStatus::from_saved(&saved, true, None))
}

#[tauri::command]
pub async fn pc_connection_status(
    state: State<'_, PcClientState>,
) -> Result<ConnectionStatus, ClientError> {
    let (generation, loaded) = match state.connection_snapshot() {
        Err(error) if error.code == "secret_lost" => {
            return Ok(ConnectionStatus {
                issue: Some("secret_lost"),
                ..ConnectionStatus::empty()
            });
        }
        loaded => loaded?,
    };
    let Some(mut saved) = loaded else {
        return Ok(ConnectionStatus::empty());
    };
    let client = state.client(&saved)?;
    match hello(&client, &saved.url, saved.pinned()).await {
        Ok(response) => {
            let hello: Hello = response.bounded_json().await?;
            if hello.api_version != API_VERSION || hello.server_name != "piep" {
                Ok(ConnectionStatus::from_saved(
                    &saved,
                    false,
                    Some("protocol_mismatch"),
                ))
            } else {
                // サーバーの更新で機能が増減したら、次の読書から従う。
                if hello.capabilities != saved.capabilities {
                    saved.capabilities = hello.capabilities;
                    state.refresh_capabilities(generation, saved.capabilities.clone())?;
                }
                // /hello は無認証。登録解除された端末を「接続中」と表示しない。
                let about = client
                    .get(format!("{}/api/about", saved.url))
                    .bearer_auth(&saved.token)
                    .timeout(PROBE_TIMEOUT)
                    .send();
                let (authorized, shelf) =
                    tokio::join!(about, shelf::discover(&client, &saved, generation),);
                let issue = match authorized {
                    // 登録は生きているが、棚を読めない（サーバーに棚がまだ無いか、この端末に許可が無い）。
                    Ok(response) if response.status().is_success() => {
                        shelf.is_none().then_some("no_library")
                    }
                    Ok(response) if response.status() == reqwest::StatusCode::UNAUTHORIZED => {
                        Some("revoked")
                    }
                    Ok(response)
                        if response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE =>
                    {
                        Some("not_ready")
                    }
                    Ok(_) => Some("remote_error"),
                    Err(_) => Some("offline"),
                };
                state.with_connection(generation, || {
                    if issue.is_none() {
                        *state
                            .shelf_scope
                            .lock()
                            .map_err(|_| ClientError::storage())? =
                            shelf.as_ref().map(shelf::ShelfStatus::scope);
                    }
                    Ok(ConnectionStatus {
                        shelf,
                        ..ConnectionStatus::from_saved(&saved, issue.is_none(), issue)
                    })
                })
            }
        }
        Err(error) if error.code == "certificate_untrusted" => Ok(ConnectionStatus::from_saved(
            &saved,
            false,
            Some("certificate_untrusted"),
        )),
        Err(_) => {
            // 証明書が変わった場合と、サーバーが停止している場合を区別する。
            let issue = match (&saved.fingerprint, probe_client()) {
                (Some(expected), Ok(probe)) => match hello(&probe, &saved.url, true).await {
                    Ok(response) => match certificate_from_response(&response) {
                        Ok(current) if fingerprint(&current) != *expected => "certificate_changed",
                        _ => "offline",
                    },
                    Err(_) => "offline",
                },
                _ => "offline",
            };
            Ok(ConnectionStatus::from_saved(&saved, false, Some(issue)))
        }
    }
}

#[tauri::command]
pub async fn pc_disconnect(
    state: State<'_, PcClientState>,
) -> Result<ConnectionStatus, ClientError> {
    state.forget()?;
    Ok(ConnectionStatus::empty())
}

// キャッシュの名前空間を決めた接続と、実際の送信先を一致させる。
async fn authenticated_get_with_saved(
    state: &PcClientState,
    saved: &SavedConnection,
    endpoint: &str,
    timeout: Duration,
) -> Result<reqwest::Response, ClientError> {
    let client = state.client(saved)?;
    let response = client
        .get(format!("{}{}", saved.url, endpoint))
        .bearer_auth(&saved.token)
        .timeout(timeout)
        .send()
        .await
        .map_err(|error| ClientError::from_send(&error, saved.pinned()))?;
    check_response(response)
}

fn check_response(response: reqwest::Response) -> Result<reqwest::Response, ClientError> {
    match response.status() {
        reqwest::StatusCode::UNAUTHORIZED => Err(ClientError::revoked()),
        reqwest::StatusCode::SERVICE_UNAVAILABLE => Err(ClientError::not_ready()),
        status if !status.is_success() => Err(ClientError::remote()),
        _ => Ok(response),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PcSearchCriteria {
    text: String,
    source: String,
    content_type: String,
    /// 作者とタグは一件ずつの並び。区切り文字を含むタグもそのまま一件として送る
    /// （自由入力の分け方は入力欄が受け持つ）。
    author: Vec<String>,
    author_exclude: Vec<String>,
    favorite: bool,
    tag: Vec<String>,
    tag_exclude: Vec<String>,
    min_chars: String,
    max_chars: String,
    sort: String,
    tag_mode: Option<String>,
    min_assets: Option<String>,
    max_assets: Option<String>,
    revision: Option<String>,
    edit: Option<String>,
    cover: Option<String>,
    date_field: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PcWorkScope {
    kind: String,
    source: String,
    key: String,
}

/// 一件ずつの作者・タグから空のものを除く。分けることはしない。
fn search_terms(values: &[String]) -> Option<Vec<String>> {
    let terms: Vec<String> = values
        .iter()
        .map(|term| term.trim())
        .filter(|term| !term.is_empty())
        .map(str::to_string)
        .collect();
    (!terms.is_empty()).then_some(terms)
}

fn parse_search_count(value: &str) -> Result<Option<i64>, ClientError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > 10 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ClientError::remote());
    }
    value
        .parse::<i64>()
        .map(Some)
        .map_err(|_| ClientError::remote())
}

/// 画面の空の選択は「指さない」。
fn search_choice(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// 共通の語彙（並べ方・向き・日付の欄）の一語を読む。
fn vocabulary<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, ClientError> {
    serde_json::from_value(serde_json::Value::String(value.to_string()))
        .map_err(|_| ClientError::remote())
}

/// 画面の条件を、サーバーと共通の `MobileSearchRequest` に組む。検査もサーバーと同じものを通す。
fn search_request(
    criteria: &PcSearchCriteria,
    cursor: Option<String>,
    offset: Option<i64>,
    scope: Option<PcWorkScope>,
) -> Result<MobileSearchRequest, ClientError> {
    // 並べ方は `鍵:向き`（`@shared/sortOptions` と同じ）。標準は関連度か保存日で、サーバーが決める。
    let (sort_by, sort_order) = match criteria.sort.as_str() {
        "standard" => (None, None),
        value => {
            let (key, order) = value.split_once(':').ok_or_else(ClientError::remote)?;
            (Some(vocabulary(key)?), Some(vocabulary(order)?))
        }
    };
    let date_from = search_choice(criteria.date_from.as_deref());
    let date_to = search_choice(criteria.date_to.as_deref());
    let date_field = match criteria.date_field.as_deref().map(str::trim) {
        None | Some("") => MobileDateField::default(),
        Some(value) => vocabulary(value)?,
    };
    let tags_include = search_terms(&criteria.tag);
    let mut request = MobileSearchRequest {
        text: search_choice(Some(&criteria.text)),
        cursor,
        offset,
        limit: Some(20),
        source: search_choice(Some(&criteria.source)),
        content_type: search_choice(Some(&criteria.content_type)),
        authors_include: search_terms(&criteria.author),
        authors_exclude: search_terms(&criteria.author_exclude),
        tag_filter_mode: search_choice(criteria.tag_mode.as_deref())
            // 「すべて」は既定なので送らない。「どれか」はタグを指したときだけ意味がある。
            .filter(|mode| mode != "and" && (mode != "or" || tags_include.is_some())),
        tags_include,
        tags_exclude: search_terms(&criteria.tag_exclude),
        min_char_count: parse_search_count(&criteria.min_chars)?,
        max_char_count: parse_search_count(&criteria.max_chars)?,
        min_asset_count: parse_search_count(criteria.min_assets.as_deref().unwrap_or(""))?,
        max_asset_count: parse_search_count(criteria.max_assets.as_deref().unwrap_or(""))?,
        revision_filter: search_choice(criteria.revision.as_deref()),
        edit_filter: search_choice(criteria.edit.as_deref()),
        cover_filter: search_choice(criteria.cover.as_deref()),
        date_field: (date_from.is_some() || date_to.is_some()).then_some(date_field),
        date_from,
        date_to,
        favorite: criteria.favorite.then_some(true),
        sort_by,
        sort_order,
        ..MobileSearchRequest::default()
    };
    if let Some(scope) = scope {
        match scope.kind.as_str() {
            "person" => {
                request.person_source = Some(scope.source);
                request.person_key = Some(scope.key);
            }
            "series" => {
                request.series_source = Some(scope.source);
                request.series_key = Some(scope.key);
            }
            _ => return Err(ClientError::remote()),
        }
        if request.person_source.is_none() && request.series_source.is_none() {
            return Err(ClientError::remote());
        }
    }
    request.validate().map_err(|_| ClientError::remote())?;
    Ok(request)
}

fn valid_entity_identity(source: &str, key: &str) -> bool {
    matches!(source, "pixiv" | "fanbox") && !key.is_empty() && key.chars().count() <= 200
}

fn valid_entity_sort(sort: Option<&str>, order: Option<&str>) -> bool {
    sort.is_none_or(|value| vocabulary::<MobileEntitySort>(value).is_ok())
        && order.is_none_or(|value| vocabulary::<MobileSortOrder>(value).is_ok())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileOutlineEntry {
    page: usize,
    index: usize,
    title: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileAssetData {
    mime_type: String,
    base64: String,
}

// 上限は受信後ではなく受信中に適用する。Content-Length がない応答にも適用する。
pub(crate) async fn bounded_response_bytes(
    mut response: reqwest::Response,
    maximum: usize,
) -> Result<Vec<u8>, ClientError> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(ClientError::remote());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ClientError::offline())? {
        if chunk.len() > maximum.saturating_sub(bytes.len()) {
            return Err(ClientError::remote());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

trait BoundedJson {
    async fn bounded_json<T: serde::de::DeserializeOwned>(self) -> Result<T, ClientError>;
}

impl BoundedJson for reqwest::Response {
    async fn bounded_json<T: serde::de::DeserializeOwned>(self) -> Result<T, ClientError> {
        bounded_json_response(self, MAX_JSON_BYTES).await
    }
}

async fn bounded_json_response<T: serde::de::DeserializeOwned>(
    response: reqwest::Response,
    maximum: usize,
) -> Result<T, ClientError> {
    let bytes = bounded_response_bytes(response, maximum).await?;
    serde_json::from_slice(&bytes).map_err(|_| ClientError::protocol())
}

async fn image_response(response: reqwest::Response) -> Result<MobileAssetData, ClientError> {
    let mime_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(ClientError::protocol)?
        .to_string();
    if !matches!(
        mime_type.as_str(),
        "image/jpeg" | "image/png" | "image/webp" | "image/gif" | "image/avif"
    ) {
        return Err(ClientError::protocol());
    }
    let bytes = bounded_response_bytes(response, MAX_ASSET_BYTES).await?;
    Ok(MobileAssetData {
        mime_type,
        base64: STANDARD.encode(bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use piep_mobile_api::MAX_NUMBERED_OFFSET;

    pub(super) async fn local_response(wire_response: impl Into<String>) -> reqwest::Response {
        use std::io::{Read, Write};
        let wire_response = wire_response.into();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(wire_response.as_bytes());
        });
        reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap()
            .get(format!("http://{address}/"))
            .send()
            .await
            .unwrap()
    }

    #[test]
    fn bounds_response_body_with_and_without_content_length() {
        tauri::async_runtime::block_on(async {
            let response =
                local_response("HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\n\r\n").await;
            assert_eq!(
                bounded_response_bytes(response, 4).await.unwrap_err().code,
                "remote_error"
            );
            let response = local_response("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n3\r\ndef\r\n0\r\n\r\n").await;
            assert_eq!(
                bounded_response_bytes(response, 4).await.unwrap_err().code,
                "remote_error"
            );
            let response = local_response("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nab\r\n2\r\ncd\r\n0\r\n\r\n").await;
            assert_eq!(bounded_response_bytes(response, 4).await.unwrap(), b"abcd");
        });
    }

    #[test]
    fn json_decoder_bounds_http_bodies_and_reports_invalid_json() {
        tauri::async_runtime::block_on(async {
            let response =
                local_response("HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\n\r\n").await;
            assert_eq!(
                response
                    .bounded_json::<serde_json::Value>()
                    .await
                    .unwrap_err()
                    .code,
                "remote_error"
            );
            let response = local_response("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n[1,2\r\n5\r\n,3,4]\r\n0\r\n\r\n").await;
            assert_eq!(
                bounded_json_response::<serde_json::Value>(response, 8)
                    .await
                    .unwrap_err()
                    .code,
                "remote_error"
            );
            let response = local_response("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"v\"\r\n3\r\n:1}\r\n0\r\n\r\n").await;
            assert_eq!(
                bounded_json_response::<serde_json::Value>(response, 7)
                    .await
                    .unwrap(),
                serde_json::json!({"v": 1})
            );
            let response = local_response("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nbad").await;
            assert_eq!(
                response
                    .bounded_json::<serde_json::Value>()
                    .await
                    .unwrap_err()
                    .code,
                "protocol_mismatch"
            );
        });
    }

    fn search_criteria(mut changes: serde_json::Value) -> PcSearchCriteria {
        let mut value = serde_json::json!({
            "text": "", "source": "", "contentType": "", "author": [],
            "authorExclude": [], "favorite": false, "tag": [],
            "tagExclude": [], "minChars": "", "maxChars": "", "sort": "standard"
        });
        for (key, field) in changes.as_object_mut().expect("changes object") {
            value[key] = field.take();
        }
        serde_json::from_value(value).expect("criteria")
    }

    #[test]
    fn recognizes_a_rejected_certificate_inside_nested_io_errors() {
        use std::io::{Error, ErrorKind};
        let rejected = rustls::Error::InvalidCertificate(rustls::CertificateError::NotValidForName);
        let nested = Error::other(Error::new(ErrorKind::InvalidData, rejected));
        assert!(rejected_certificate(&nested));
        let refused = Error::new(ErrorKind::ConnectionRefused, "refused");
        assert!(!rejected_certificate(&refused));
    }

    #[test]
    fn accepts_any_https_host_and_rejects_everything_else() {
        for address in [
            "http://192.168.1.2:38777",
            "http://example.com/",
            "https://user@example.com/",
            "https://user:secret@example.com/",
            "https://example.com/?x=1",
            "https://example.com/#x",
            "not a url",
        ] {
            assert!(validate_url(address).is_err(), "{address}");
        }
        for address in [
            "https://192.168.1.2:38777",
            "https://127.0.0.1:38777/",
            "https://8.8.8.8:8443/",
            "https://example.com/",
            "https://example.com:8443/",
            "https://example.com/piep/",
        ] {
            assert!(validate_url(address).is_ok(), "{address}");
        }
    }

    fn pairing(changes: serde_json::Value) -> String {
        let mut value = serde_json::json!({
            "version": 2,
            "role": "reader",
            "url": "https://example.com/piep/",
            "secret": URL_SAFE_NO_PAD.encode([7_u8; 32]),
            "fingerprint": null,
        });
        for (key, field) in changes.as_object().expect("changes object") {
            value[key] = field.clone();
        }
        value.to_string()
    }

    #[test]
    fn pairing_reads_version_2_and_normalizes_the_address() {
        let parsed = parse_pairing(&pairing(serde_json::json!({})), "phone").expect("pairing");
        assert_eq!(parsed.url, "https://example.com/piep");
        assert_eq!(parsed.fingerprint, None);

        let upper = "A".repeat(64);
        let parsed = parse_pairing(
            &pairing(serde_json::json!({ "fingerprint": upper })),
            "phone",
        )
        .expect("pinned pairing");
        assert_eq!(parsed.fingerprint, Some("a".repeat(64)));
    }

    #[test]
    fn pairing_refuses_old_strings_admin_strings_and_bad_values() {
        let old = serde_json::json!({
            "version": 1,
            "url": "https://192.168.1.2:38777",
            "fingerprint": "a".repeat(64),
            "secret": URL_SAFE_NO_PAD.encode([7_u8; 32]),
        });
        assert_eq!(
            parse_pairing(&old.to_string(), "phone").unwrap_err().code,
            "invalid_pairing"
        );

        let admin =
            parse_pairing(&pairing(serde_json::json!({ "role": "admin" })), "phone").unwrap_err();
        assert!(admin.message.contains("管理者用"), "{}", admin.message);

        for changes in [
            serde_json::json!({ "version": 1 }),
            serde_json::json!({ "role": "owner" }),
            serde_json::json!({ "url": "http://example.com/" }),
            serde_json::json!({ "fingerprint": "short" }),
            serde_json::json!({ "secret": "short" }),
            serde_json::json!({ "extra": true }),
        ] {
            assert!(
                parse_pairing(&pairing(changes.clone()), "phone").is_err(),
                "{changes}"
            );
        }
        assert!(parse_pairing(&pairing(serde_json::json!({})), "  ").is_err());
        assert!(parse_pairing(&pairing(serde_json::json!({})), &"x".repeat(65)).is_err());
    }

    fn saved(changes: serde_json::Value) -> serde_json::Value {
        let mut value = serde_json::json!({
            "url": "https://example.com",
            "token": URL_SAFE_NO_PAD.encode([9_u8; 32]),
            "deviceId": "device",
            "deviceName": "phone",
        });
        for (key, field) in changes.as_object().expect("changes object") {
            value[key] = field.clone();
        }
        value
    }

    fn load_saved(
        name: &str,
        value: serde_json::Value,
    ) -> Result<Option<SavedConnection>, ClientError> {
        let directory =
            std::env::temp_dir().join(format!("piep-android-{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("directory");
        let state = PcClientState::new(directory.join("connection.json"), Arc::new(TestCipher));
        let sealed = TestCipher.seal(value.to_string().as_bytes()).expect("seal");
        fs::write(state.sealed_file(), sealed).expect("write");
        let loaded = state.load();
        fs::remove_dir_all(&directory).expect("cleanup");
        loaded
    }

    /// 中身が平文のまま見えないことと、開けないときの動きを試すためのもの。
    struct TestCipher;

    impl SecretCipher for TestCipher {
        fn seal(&self, plain: &[u8]) -> Result<Vec<u8>, ()> {
            Ok(plain.iter().map(|byte| byte ^ 0x5a).collect())
        }
        fn open(&self, sealed: &[u8]) -> Result<Vec<u8>, ()> {
            Ok(sealed.iter().map(|byte| byte ^ 0x5a).collect())
        }
    }

    struct BrokenCipher;

    impl SecretCipher for BrokenCipher {
        fn seal(&self, _: &[u8]) -> Result<Vec<u8>, ()> {
            Err(())
        }
        fn open(&self, _: &[u8]) -> Result<Vec<u8>, ()> {
            Err(())
        }
    }

    fn secret_fixture() -> (PathBuf, PathBuf, serde_json::Value) {
        let directory =
            std::env::temp_dir().join(format!("piep-android-secret-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("directory");
        let file = directory.join("pc-connection.json");
        let value = serde_json::json!({
            "url": "https://server.example/",
            "token": URL_SAFE_NO_PAD.encode([7_u8; 32]),
            "deviceId": "device",
            "deviceName": "phone",
        });
        (directory, file, value)
    }

    #[test]
    fn the_connection_is_kept_only_sealed() {
        let (directory, file, value) = secret_fixture();
        let state = PcClientState::new(file.clone(), Arc::new(TestCipher));
        state
            .save(&serde_json::from_value(value).unwrap())
            .expect("save");
        assert!(!file.exists());
        let sealed = fs::read(state.sealed_file()).expect("sealed");
        let token = URL_SAFE_NO_PAD.encode([7_u8; 32]);
        assert!(!String::from_utf8_lossy(&sealed).contains(&token));
        // 開き直しても同じ登録を読める。
        let reopened = PcClientState::new(file, Arc::new(TestCipher));
        assert_eq!(reopened.load().expect("load").expect("saved").token, token);
        reopened.forget().expect("forget");
        assert!(!reopened.sealed_file().exists());
        assert!(reopened.load().expect("load").is_none());
        fs::remove_dir_all(&directory).expect("cleanup");
    }

    #[test]
    fn a_seal_that_cannot_be_opened_asks_for_registration_again() {
        let (directory, file, value) = secret_fixture();
        let writer = PcClientState::new(file.clone(), Arc::new(TestCipher));
        writer
            .save(&serde_json::from_value(value).unwrap())
            .expect("save");
        let state = PcClientState::new(file, Arc::new(BrokenCipher));
        let error = state.load().expect_err("cannot open");
        assert_eq!(error.code, "secret_lost");
        state.forget().expect("forget");
        assert!(state.load().expect("load").is_none());
        fs::remove_dir_all(&directory).expect("cleanup");
    }

    #[test]
    fn saved_connection_keeps_loading_the_old_pinned_form() {
        let certificate = b"certificate";
        let old = saved(serde_json::json!({
            "url": "https://192.168.1.2:38777",
            "fingerprint": fingerprint(certificate),
            "certificateDer": URL_SAFE_NO_PAD.encode(certificate),
        }));
        let loaded = load_saved("old", old).expect("load").expect("saved");
        assert!(loaded.pinned());
    }

    #[test]
    fn saved_connection_without_a_pin_loads_and_half_a_pin_does_not() {
        let loaded = load_saved(
            "ca",
            saved(serde_json::json!({ "capabilities": ["read", "reading-sync"] })),
        )
        .expect("load")
        .expect("saved");
        assert!(!loaded.pinned());
        // 指紋だけ残っていて証明書が無い形を、CA 検証へ格下げしない。
        let half = saved(serde_json::json!({ "fingerprint": "a".repeat(64) }));
        assert_eq!(load_saved("half", half).unwrap_err().code, "storage");
        let mismatch = saved(serde_json::json!({
            "fingerprint": "a".repeat(64),
            "certificateDer": URL_SAFE_NO_PAD.encode(b"certificate"),
        }));
        assert_eq!(
            load_saved("mismatch", mismatch).unwrap_err().code,
            "storage"
        );
    }

    #[test]
    fn numbered_search_offsets_are_bounded_and_exclusive_with_cursors() {
        let criteria = search_criteria(serde_json::json!({}));
        let request = |cursor: Option<&str>, offset| {
            search_request(&criteria, cursor.map(str::to_string), offset, None)
        };
        assert!(request(None, None).is_ok());
        assert!(request(None, Some(0)).is_ok());
        assert!(request(None, Some(MAX_NUMBERED_OFFSET)).is_ok());
        assert!(request(None, Some(MAX_NUMBERED_OFFSET + 1)).is_err());
        assert!(request(None, Some(-1)).is_err());
        assert!(request(Some("cursor"), Some(20)).is_err());
    }

    #[test]
    fn search_request_forwards_terms_and_non_ai_filters() {
        let criteria = search_criteria(serde_json::json!({
            "author": ["作者A", " 作者B ", "", "作者C"], "tag": ["猫", "海"],
            "tagMode": "or", "minAssets": "2", "maxAssets": "12",
            "revision": "revised", "edit": "edited", "cover": "has_cover",
            "dateField": "published", "dateFrom": "2024-02-29", "dateTo": "2026-10-14",
            "sort": "published:asc"
        }));
        let payload =
            serde_json::to_value(search_request(&criteria, None, None, None).expect("request"))
                .unwrap();
        assert_eq!(
            payload["authorsInclude"],
            serde_json::json!(["作者A", "作者B", "作者C"])
        );
        assert_eq!(payload["tagsInclude"], serde_json::json!(["猫", "海"]));
        assert_eq!(payload["tagFilterMode"], "or");
        assert_eq!(payload["minAssetCount"], 2);
        assert_eq!(payload["maxAssetCount"], 12);
        assert_eq!(payload["revisionFilter"], "revised");
        assert_eq!(payload["editFilter"], "edited");
        assert_eq!(payload["coverFilter"], "has_cover");
        assert_eq!(payload["dateField"], "published");
        assert_eq!(payload["dateFrom"], "2024-02-29");
        assert_eq!(payload["dateTo"], "2026-10-14");
        assert_eq!(payload["sortBy"], "published");
        assert_eq!(payload["sortOrder"], "asc");
    }

    #[test]
    fn empty_filters_are_left_out_of_the_request() {
        let criteria = search_criteria(serde_json::json!({
            "tagMode": "and", "minAssets": "", "maxAssets": "",
            "revision": "", "edit": "", "cover": "",
            "dateField": "saved", "dateFrom": "", "dateTo": ""
        }));
        let payload =
            serde_json::to_value(search_request(&criteria, None, None, None).expect("request"))
                .unwrap();
        for key in [
            "tagFilterMode",
            "minAssetCount",
            "maxAssetCount",
            "revisionFilter",
            "editFilter",
            "coverFilter",
            "dateField",
            "dateFrom",
            "dateTo",
        ] {
            assert!(payload.get(key).is_none(), "unexpected {key}");
        }
    }

    /// 区切り文字を含むタグ・作者は、分けずに一件として送る。
    #[test]
    fn a_tag_with_a_delimiter_is_sent_as_one_tag() {
        let criteria = search_criteria(serde_json::json!({
            "author": ["山田、太郎"], "tag": ["猫、海", "A,B"], "tagExclude": ["春，夏"]
        }));
        let payload =
            serde_json::to_value(search_request(&criteria, None, None, None).expect("request"))
                .unwrap();
        assert_eq!(payload["authorsInclude"], serde_json::json!(["山田、太郎"]));
        assert_eq!(payload["tagsInclude"], serde_json::json!(["猫、海", "A,B"]));
        assert_eq!(payload["tagsExclude"], serde_json::json!(["春，夏"]));
    }

    #[test]
    fn search_request_rejects_invalid_ranges_dates_and_choices() {
        for changes in [
            serde_json::json!({ "author": ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k"] }),
            serde_json::json!({ "tag": ["猫"], "tagMode": "xor" }),
            serde_json::json!({ "minAssets": "3", "maxAssets": "2" }),
            serde_json::json!({ "minAssets": "-1" }),
            serde_json::json!({ "revision": "both" }),
            serde_json::json!({ "dateFrom": "2026-02-30" }),
            serde_json::json!({ "dateFrom": "2026-10-15", "dateTo": "2026-10-14" }),
            serde_json::json!({ "sort": "privatePath" }),
            serde_json::json!({ "sort": "saved:sideways" }),
            serde_json::json!({ "dateField": "downloaded_at", "dateFrom": "2026-10-14" }),
        ] {
            let criteria = search_criteria(changes.clone());
            assert!(
                search_request(&criteria, None, None, None).is_err(),
                "accepted {changes}"
            );
        }
    }
}
