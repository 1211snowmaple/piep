//! piep サーバーへの通信を Android の Rust 側に閉じ込める。
//! 認証鍵は WebView の JavaScript に返さない。

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
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

pub mod cover_protocol;
pub(crate) mod device_db;
mod image_validation;
pub(crate) mod local_reading;
pub(crate) mod offline;
pub(crate) mod reading_sync;

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
const MAX_NUMBERED_OFFSET: i64 = 5_000;
const API_VERSION: u32 = 2;
const PAIRING_VERSION: u32 = 2;
const CAPABILITY_READING_SYNC: &str = "reading-sync";
const CAPABILITY_FAVORITE_WRITE: &str = "favorite-write";

pub struct PcClientState {
    file: PathBuf,
    cipher: Arc<dyn SecretCipher>,
    saved: Mutex<Option<SavedConnection>>,
    // 保存直前の確認から履歴の永続化まで、登録変更と排他にする。
    connection_generation: Mutex<u64>,
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
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // 封をする前の版が置いたファイルは、読んだら封じて消す。
                let bytes = match fs::read(&self.file) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(_) => return Err(ClientError::storage()),
                };
                let saved = parse_saved(&bytes)?;
                self.write_sealed(&saved)?;
                saved
            }
            Err(_) => return Err(ClientError::storage()),
        };
        remove_if_present(&self.file)?;
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
        // sealed の公開後は、旧平文の削除に失敗しても接続世代を巻き戻さない。
        remove_if_present(&self.file)
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
            remove_if_present(&self.file)?;
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
        remove_if_present(&self.file)?;
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
    fn favorite_unsupported() -> Self {
        Self {
            code: "favorite_unsupported",
            message: "このサーバーではお気に入りを切り替えられません。サーバーを新しい版にしてください",
        }
    }
    fn favorite_missing() -> Self {
        Self {
            code: "favorite_missing",
            message: "サーバーの本棚にこの作品が見つかりません",
        }
    }
    fn favorite_rejected() -> Self {
        Self {
            code: "favorite_rejected",
            message: "お気に入りを切り替えられませんでした",
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
    fn can_sync_reading(&self) -> bool {
        self.capabilities
            .iter()
            .any(|capability| capability == CAPABILITY_READING_SYNC)
    }
    fn can_write_favorite(&self) -> bool {
        self.capabilities
            .iter()
            .any(|capability| capability == CAPABILITY_FAVORITE_WRITE)
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
    // 端点は `{url}/api/v1/...` で組み立てるので、末尾の / を落として持つ。
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
        .get(format!("{base}/api/v1/hello"))
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
        .post(format!("{}/api/v1/pair", pairing.url))
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
                let authorized = client
                    .get(format!("{}/api/v1/collections", saved.url))
                    .bearer_auth(&saved.token)
                    .timeout(PROBE_TIMEOUT)
                    .send()
                    .await;
                let issue = match authorized {
                    Ok(response) if response.status().is_success() => None,
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
                Ok(ConnectionStatus::from_saved(&saved, issue.is_none(), issue))
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

async fn authenticated_get(
    state: &PcClientState,
    endpoint: &str,
) -> Result<reqwest::Response, ClientError> {
    authenticated_get_with_timeout(state, endpoint, REQUEST_TIMEOUT).await
}

async fn authenticated_get_with_timeout(
    state: &PcClientState,
    endpoint: &str,
    timeout: Duration,
) -> Result<reqwest::Response, ClientError> {
    let saved = state.load()?.ok_or_else(ClientError::invalid_pairing)?;
    let client = state.client(&saved)?;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileWork {
    id: i64,
    source: String,
    source_id: String,
    title: String,
    author_name: String,
    #[serde(default)]
    author_id: String,
    content_type: String,
    tags: Vec<String>,
    excerpt: Option<String>,
    text_length: i64,
    asset_count: i64,
    has_cover: bool,
    current_version: i64,
    content_hash: Option<String>,
    favorite: bool,
    series_title: Option<String>,
    series_id: Option<String>,
    #[serde(default)]
    source_created_at: Option<String>,
    #[serde(default)]
    downloaded_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    items: Vec<MobileWork>,
    next_cursor: Option<String>,
    total_estimate: Option<i64>,
    index_complete: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PcSearchCriteria {
    text: String,
    source: String,
    content_type: String,
    author: String,
    author_exclude: String,
    favorite: bool,
    tag: String,
    tag_exclude: String,
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

fn split_search_terms(value: &str) -> Result<Option<Vec<String>>, ClientError> {
    if value.chars().count() > 1_100 {
        return Err(ClientError::remote());
    }
    let terms: Vec<String> = value
        .split(['\n', '\r', ',', '、', '，'])
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_string)
        .collect();
    if terms.len() > 10 || terms.iter().any(|term| term.chars().count() > 100) {
        return Err(ClientError::remote());
    }
    Ok((!terms.is_empty()).then_some(terms))
}

fn parse_search_count(value: &str) -> Result<Option<i64>, ClientError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > 10 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ClientError::remote());
    }
    let count = value.parse::<i64>().map_err(|_| ClientError::remote())?;
    if count > 1_000_000_000 {
        return Err(ClientError::remote());
    }
    Ok(Some(count))
}

fn optional_search_choice<'a>(
    value: Option<&'a str>,
    choices: &[&str],
) -> Result<Option<&'a str>, ClientError> {
    match value.unwrap_or("") {
        "" => Ok(None),
        value if choices.contains(&value) => Ok(Some(value)),
        _ => Err(ClientError::remote()),
    }
}

fn valid_search_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
    {
        return false;
    }
    let year = value[0..4].parse::<u32>().unwrap_or(0);
    let month = value[5..7].parse::<u32>().unwrap_or(0);
    let day = value[8..10].parse::<u32>().unwrap_or(0);
    if year == 0 {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=max_day).contains(&day)
}

fn search_payload(
    criteria: &PcSearchCriteria,
    cursor: Option<String>,
    offset: Option<i64>,
    scope: Option<PcWorkScope>,
) -> Result<serde_json::Value, ClientError> {
    if criteria.text.chars().count() > 200
        || cursor.as_ref().is_some_and(|cursor| cursor.len() > 4096)
        || validate_numbered_offset(offset, cursor.as_deref()).is_err()
    {
        return Err(ClientError::remote());
    }
    let source = optional_search_choice(Some(&criteria.source), &["pixiv", "fanbox"])?;
    let content_type = optional_search_choice(
        Some(&criteria.content_type),
        &["novel", "article", "text", "image", "file"],
    )?;
    let (sort_by, sort_order) = match criteria.sort.as_str() {
        "standard" => (None, None),
        "dateDesc" => (Some("date"), Some("desc")),
        "dateAsc" => (Some("date"), Some("asc")),
        "publishedDesc" => (Some("published"), Some("desc")),
        "publishedAsc" => (Some("published"), Some("asc")),
        "titleAsc" => (Some("title"), Some("asc")),
        "titleDesc" => (Some("title"), Some("desc")),
        "authorAsc" => (Some("author"), Some("asc")),
        "authorDesc" => (Some("author"), Some("desc")),
        "lengthDesc" => (Some("length"), Some("desc")),
        "lengthAsc" => (Some("length"), Some("asc")),
        "assetsDesc" => (Some("assets"), Some("desc")),
        "assetsAsc" => (Some("assets"), Some("asc")),
        "versionDesc" => (Some("version"), Some("desc")),
        "versionAsc" => (Some("version"), Some("asc")),
        "seriesAsc" => (Some("series_order"), Some("asc")),
        "seriesDesc" => (Some("series_order"), Some("desc")),
        _ => return Err(ClientError::remote()),
    };
    let min_chars = parse_search_count(&criteria.min_chars)?;
    let max_chars = parse_search_count(&criteria.max_chars)?;
    let min_assets = parse_search_count(criteria.min_assets.as_deref().unwrap_or(""))?;
    let max_assets = parse_search_count(criteria.max_assets.as_deref().unwrap_or(""))?;
    if min_chars.zip(max_chars).is_some_and(|(min, max)| min > max)
        || min_assets
            .zip(max_assets)
            .is_some_and(|(min, max)| min > max)
    {
        return Err(ClientError::remote());
    }
    let tag_mode = optional_search_choice(criteria.tag_mode.as_deref(), &["and", "or"])?;
    let revision =
        optional_search_choice(criteria.revision.as_deref(), &["revised", "first_version"])?;
    let edit = optional_search_choice(criteria.edit.as_deref(), &["edited", "unedited"])?;
    let cover = optional_search_choice(criteria.cover.as_deref(), &["has_cover", "no_cover"])?;
    let date_field = optional_search_choice(
        criteria.date_field.as_deref(),
        &["downloaded_at", "source_created_at", "source_updated_at"],
    )?;
    let date_from = criteria.date_from.as_deref().unwrap_or("").trim();
    let date_to = criteria.date_to.as_deref().unwrap_or("").trim();
    if (!date_from.is_empty() && !valid_search_date(date_from))
        || (!date_to.is_empty() && !valid_search_date(date_to))
        || (!date_from.is_empty() && !date_to.is_empty() && date_from > date_to)
    {
        return Err(ClientError::remote());
    }
    let authors_include = split_search_terms(&criteria.author)?;
    let authors_exclude = split_search_terms(&criteria.author_exclude)?;
    let tags_include = split_search_terms(&criteria.tag)?;
    let tags_exclude = split_search_terms(&criteria.tag_exclude)?;
    let (person_source, person_key, series_source, series_key) = match scope {
        None => (None, None, None, None),
        Some(scope)
            if matches!(scope.source.as_str(), "pixiv" | "fanbox")
                && !scope.key.is_empty()
                && scope.key.chars().count() <= 200 =>
        {
            match scope.kind.as_str() {
                "person" => (Some(scope.source), Some(scope.key), None, None),
                "series" => (None, None, Some(scope.source), Some(scope.key)),
                _ => return Err(ClientError::remote()),
            }
        }
        _ => return Err(ClientError::remote()),
    };
    let mut payload = serde_json::json!({
        "text": criteria.text.trim(), "cursor": cursor, "limit": 20,
        "source": source, "favorite": criteria.favorite.then_some(true),
        "contentType": content_type,
        "authorsInclude": authors_include, "authorsExclude": authors_exclude,
        "tagsInclude": tags_include, "tagsExclude": tags_exclude,
        "minCharCount": min_chars, "maxCharCount": max_chars,
        "sortBy": sort_by, "sortOrder": sort_order,
        "personSource": person_source, "personKey": person_key,
        "seriesSource": series_source, "seriesKey": series_key,
    });
    if let Some(offset) = offset {
        payload["offset"] = serde_json::json!(offset);
    }
    if tag_mode == Some("or") && payload["tagsInclude"].is_array() {
        payload["tagFilterMode"] = serde_json::json!("or");
    }
    if let Some(count) = min_assets {
        payload["minAssetCount"] = serde_json::json!(count);
    }
    if let Some(count) = max_assets {
        payload["maxAssetCount"] = serde_json::json!(count);
    }
    if let Some(value) = revision {
        payload["revisionFilter"] = serde_json::json!(value);
    }
    if let Some(value) = edit {
        payload["editFilter"] = serde_json::json!(value);
    }
    if let Some(value) = cover {
        payload["coverFilter"] = serde_json::json!(value);
    }
    if !date_from.is_empty() || !date_to.is_empty() {
        payload["dateField"] = serde_json::json!(date_field.unwrap_or("downloaded_at"));
        if !date_from.is_empty() {
            payload["dateFrom"] = serde_json::json!(date_from);
        }
        if !date_to.is_empty() {
            payload["dateTo"] = serde_json::json!(date_to);
        }
    }
    // Keep the original request shape for PC builds that predate these filters.
    for key in [
        "authorsExclude",
        "tagsExclude",
        "minCharCount",
        "maxCharCount",
    ] {
        if payload[key].is_null() {
            payload.as_object_mut().unwrap().remove(key);
        }
    }
    Ok(payload)
}

#[tauri::command]
pub async fn pc_search_works(
    state: State<'_, PcClientState>,
    criteria: PcSearchCriteria,
    cursor: Option<String>,
    offset: Option<i64>,
    scope: Option<PcWorkScope>,
    limit: Option<i64>,
) -> Result<SearchPage, ClientError> {
    if limit.is_some_and(|value| !(1..=50).contains(&value)) {
        return Err(ClientError::remote());
    }
    let mut payload = search_payload(&criteria, cursor, offset, scope)?;
    if let Some(limit) = limit {
        payload["limit"] = serde_json::json!(limit);
    }
    let saved = state.load()?.ok_or_else(ClientError::invalid_pairing)?;
    let client = state.client(&saved)?;
    let response = client
        .post(format!("{}/api/v1/works/search", saved.url))
        .bearer_auth(&saved.token)
        .json(&payload)
        .send()
        .await
        .map_err(|error| ClientError::from_send(&error, saved.pinned()))?;
    check_response(response)?.bounded_json().await
}

fn validate_numbered_offset(offset: Option<i64>, cursor: Option<&str>) -> Result<(), ClientError> {
    if cursor.is_some() && offset.is_some()
        || offset.is_some_and(|value| !(0..=MAX_NUMBERED_OFFSET).contains(&value))
    {
        return Err(ClientError::remote());
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileEntity {
    source: String,
    source_key: String,
    display_name: String,
    count: i64,
    description: Option<String>,
    sample_title: Option<String>,
    has_cover: bool,
    is_concluded: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileEntityPage {
    items: Vec<MobileEntity>,
    next_offset: Option<i64>,
    total: i64,
}

fn valid_entity_identity(source: &str, key: &str) -> bool {
    matches!(source, "pixiv" | "fanbox") && !key.is_empty() && key.chars().count() <= 200
}

fn valid_entity_sort(sort: Option<&str>, order: Option<&str>) -> bool {
    sort.is_none_or(|value| {
        matches!(
            value,
            "work_count"
                | "downloaded_at"
                | "source_created_at"
                | "source_updated_at"
                | "name"
                | "text_length"
                | "file_size_bytes"
                | "asset_count"
                | "current_version"
        )
    }) && order.is_none_or(|value| matches!(value, "asc" | "desc"))
}

#[tauri::command]
pub async fn pc_list_entities(
    state: State<'_, PcClientState>,
    kind: String,
    text: String,
    offset: i64,
    sort: Option<String>,
    order: Option<String>,
) -> Result<MobileEntityPage, ClientError> {
    if !matches!(kind.as_str(), "people" | "series")
        || text.chars().count() > 200
        || !(0..=1_000_000).contains(&offset)
        || !valid_entity_sort(sort.as_deref(), order.as_deref())
    {
        return Err(ClientError::remote());
    }
    let query_string = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("text", text.trim());
        query.append_pair("offset", &offset.to_string());
        query.append_pair("limit", "30");
        if let Some(sort) = sort.as_deref() {
            query.append_pair("sort", sort);
        }
        if let Some(order) = order.as_deref() {
            query.append_pair("order", order);
        }
        query.finish()
    };
    let endpoint = format!("/api/v1/entities/{kind}?{query_string}");
    authenticated_get(&state, &endpoint)
        .await?
        .bounded_json()
        .await
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileFacet {
    name: String,
    count: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileEntityDetail {
    source: String,
    source_key: String,
    display_name: String,
    description: Option<String>,
    work_count: i64,
    has_cover: bool,
    has_banner: bool,
    is_concluded: Option<bool>,
    published_count: Option<i64>,
    links: Vec<String>,
    tags: Vec<MobileFacet>,
    series_count: Option<i64>,
    collections: Vec<MobileCollection>,
}

#[tauri::command]
pub async fn pc_get_entity_detail(
    state: State<'_, PcClientState>,
    kind: String,
    source: String,
    key: String,
) -> Result<MobileEntityDetail, ClientError> {
    if !matches!(kind.as_str(), "people" | "series") || !valid_entity_identity(&source, &key) {
        return Err(ClientError::remote());
    }
    let query_string = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("source", &source);
        query.append_pair("key", &key);
        query.finish()
    };
    authenticated_get(
        &state,
        &format!("/api/v1/entities/{kind}/detail?{query_string}"),
    )
    .await?
    .bounded_json()
    .await
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileEntitySeriesPage {
    items: Vec<MobileEntity>,
    next_cursor: Option<String>,
    total: i64,
}

#[tauri::command]
pub async fn pc_list_entity_series(
    state: State<'_, PcClientState>,
    source: String,
    key: String,
    text: String,
    cursor: Option<String>,
    sort: Option<String>,
    order: Option<String>,
) -> Result<MobileEntitySeriesPage, ClientError> {
    if !valid_entity_identity(&source, &key)
        || text.chars().count() > 200
        || cursor.as_ref().is_some_and(|cursor| cursor.len() > 4096)
        || !valid_entity_sort(sort.as_deref(), order.as_deref())
    {
        return Err(ClientError::remote());
    }
    let query_string = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("source", &source);
        query.append_pair("key", &key);
        query.append_pair("text", text.trim());
        query.append_pair("limit", "30");
        if let Some(cursor) = cursor.as_deref() {
            query.append_pair("cursor", cursor);
        }
        if let Some(sort) = sort.as_deref() {
            query.append_pair("sort", sort);
        }
        if let Some(order) = order.as_deref() {
            query.append_pair("order", order);
        }
        query.finish()
    };
    authenticated_get(
        &state,
        &format!("/api/v1/entities/people/series?{query_string}"),
    )
    .await?
    .bounded_json()
    .await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileCollection {
    id: String,
    name: String,
    description: Option<String>,
    collection_kind: String,
    member_count: i64,
    available_count: i64,
    cover_work_id: Option<i64>,
    #[serde(default)]
    cover_mode: String,
    #[serde(default)]
    cover_revision: i64,
    #[serde(default)]
    has_single_cover: bool,
    #[serde(default)]
    has_file_cover: bool,
    #[serde(default)]
    cover_tiles: Vec<MobileCollectionCoverTile>,
    #[serde(default)]
    position: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileCollectionCoverTile {
    source: String,
    source_id: String,
    title: String,
    author_name: String,
    has_cover: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileCollectionMember {
    title: String,
    author_name: String,
    position: i64,
    work: Option<MobileWork>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileCollectionDetail {
    #[serde(flatten)]
    summary: MobileCollection,
    members: Vec<MobileCollectionMember>,
}

#[tauri::command]
pub async fn pc_list_collections(
    state: State<'_, PcClientState>,
) -> Result<Vec<MobileCollection>, ClientError> {
    authenticated_get(&state, "/api/v1/collections")
        .await?
        .bounded_json()
        .await
}

#[tauri::command]
pub async fn pc_get_work_collections(
    state: State<'_, PcClientState>,
    work_id: i64,
) -> Result<Vec<MobileCollection>, ClientError> {
    if work_id <= 0 {
        return Err(ClientError::remote());
    }
    authenticated_get(&state, &format!("/api/v1/works/{work_id}/collections"))
        .await?
        .bounded_json()
        .await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteState {
    source: String,
    source_id: String,
    favorite: bool,
}

fn favorite_endpoint(base: &str, source: &str, source_id: &str) -> Result<Url, ClientError> {
    let mut url = Url::parse(base).map_err(|_| ClientError::storage())?;
    url.path_segments_mut()
        .map_err(|_| ClientError::storage())?
        .pop_if_empty()
        .extend(["api", "v1", "favorites", source, source_id]);
    Ok(url)
}

fn valid_favorite_target(source: &str, source_id: &str) -> bool {
    matches!(source, "pixiv" | "fanbox")
        && !source_id.is_empty()
        && source_id.chars().count() <= 200
}

/// お気に入りを付け外しする。サーバーが `favorite-write` を持つときだけ送る。
///
/// 圏外では溜めずに失敗を返す。利用者は「つながる時だけ押せる」を選んでいる。
/// 返る値はサーバーが残したほうで、別の端末が新しい時刻で書いていればそちらになる。
#[tauri::command]
pub async fn pc_set_favorite(
    state: State<'_, PcClientState>,
    source: String,
    source_id: String,
    favorite: bool,
) -> Result<FavoriteState, ClientError> {
    if !valid_favorite_target(&source, &source_id) {
        return Err(ClientError::favorite_rejected());
    }
    let saved = state.load()?.ok_or_else(ClientError::invalid_pairing)?;
    if !saved.can_write_favorite() {
        return Err(ClientError::favorite_unsupported());
    }
    let device_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(1)
        .max(1);
    let client = state.client(&saved)?;
    let response = client
        .put(favorite_endpoint(&saved.url, &source, &source_id)?)
        .bearer_auth(&saved.token)
        .json(&serde_json::json!({ "favorite": favorite, "deviceTime": device_time }))
        .send()
        .await
        .map_err(|error| ClientError::from_send(&error, saved.pinned()))?;
    match response.status() {
        reqwest::StatusCode::NOT_FOUND => Err(ClientError::favorite_missing()),
        status if status.is_client_error() && status != reqwest::StatusCode::UNAUTHORIZED => {
            Err(ClientError::favorite_rejected())
        }
        _ => check_response(response)?.bounded_json().await,
    }
}

#[tauri::command]
pub async fn pc_get_collection(
    state: State<'_, PcClientState>,
    collection_id: String,
) -> Result<MobileCollectionDetail, ClientError> {
    if collection_id.is_empty()
        || collection_id.len() > 128
        || !collection_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ClientError::remote());
    }
    authenticated_get(&state, &format!("/api/v1/collections/{collection_id}"))
        .await?
        .bounded_json()
        .await
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileMetadata {
    work: MobileWork,
    #[serde(default)]
    people: Vec<MobilePerson>,
    #[serde(default)]
    series: Vec<MobileSeriesMembership>,
    versions: Vec<MobileVersion>,
    is_edited: bool,
    active_edit_id: Option<i64>,
    active_edit_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobilePerson {
    pub source: String,
    pub key: String,
    pub role: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileSeriesMembership {
    pub source: String,
    pub key: String,
    pub title: String,
    pub content_order: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileVersion {
    version: i64,
    content_hash: Option<String>,
    text_length: i64,
    asset_count: i64,
    created_at: String,
}

#[tauri::command]
pub async fn pc_get_metadata(
    state: State<'_, PcClientState>,
    work_id: i64,
) -> Result<MobileMetadata, ClientError> {
    if work_id <= 0 {
        return Err(ClientError::remote());
    }
    authenticated_get(&state, &format!("/api/v1/works/{work_id}"))
        .await?
        .bounded_json()
        .await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobilePage {
    page: usize,
    page_count: usize,
    html: String,
    total_plain_text_chars: usize,
    source_page_starts: Vec<usize>,
    /// この本文の版。古いサーバーは返さない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    revision: Option<MobileRevision>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileRevision {
    current_version: i64,
    content_hash: Option<String>,
    active_edit_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileOutlineEntry {
    page: usize,
    index: usize,
    title: String,
}

#[tauri::command]
pub async fn pc_get_outline(
    state: State<'_, PcClientState>,
    work_id: i64,
) -> Result<Vec<MobileOutlineEntry>, ClientError> {
    if work_id <= 0 {
        return Err(ClientError::remote());
    }
    authenticated_get(&state, &format!("/api/v1/works/{work_id}/outline"))
        .await?
        .bounded_json()
        .await
}

#[tauri::command]
pub async fn pc_get_page(
    state: State<'_, PcClientState>,
    work_id: i64,
    page: usize,
    version: Option<i64>,
) -> Result<MobilePage, ClientError> {
    if work_id <= 0 || page > 1_000_000 {
        return Err(ClientError::remote());
    }
    let version_query = version
        .map(|version| format!("?version={version}"))
        .unwrap_or_default();
    authenticated_get(
        &state,
        &format!("/api/v1/works/{work_id}/pages/{page}{version_query}"),
    )
    .await?
    .bounded_json()
    .await
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileAssetData {
    mime_type: String,
    base64: String,
}

// 上限は受信後ではなく受信中に適用する。Content-Length がない応答にも適用する。
async fn bounded_response_bytes(
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
    #[test]
    fn favorite_endpoint_keeps_the_path_prefix_and_escapes_the_id() {
        assert_eq!(
            super::favorite_endpoint("https://example.com/piep", "fanbox", "a/b c")
                .unwrap()
                .as_str(),
            "https://example.com/piep/api/v1/favorites/fanbox/a%2Fb%20c",
        );
        assert_eq!(
            super::favorite_endpoint("https://192.168.1.2:38777", "pixiv", "123")
                .unwrap()
                .as_str(),
            "https://192.168.1.2:38777/api/v1/favorites/pixiv/123",
        );
        assert!(!super::valid_favorite_target("booth", "1"));
        assert!(!super::valid_favorite_target("pixiv", ""));
    }

    use super::*;

    async fn local_response(wire_response: &'static str) -> reqwest::Response {
        use std::io::{Read, Write};
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
            "text": "", "source": "", "contentType": "", "author": "",
            "authorExclude": "", "favorite": false, "tag": "",
            "tagExclude": "", "minChars": "", "maxChars": "", "sort": "standard"
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
        let file = directory.join("connection.json");
        fs::write(&file, value.to_string()).expect("write");
        let loaded = PcClientState::new(file, Arc::new(TestCipher)).load();
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
    fn the_old_plain_file_is_sealed_and_removed() {
        let (directory, file, value) = secret_fixture();
        fs::write(&file, value.to_string()).expect("write");
        let state = PcClientState::new(file.clone(), Arc::new(TestCipher));
        assert_eq!(
            state.load().expect("load").expect("saved").device_id,
            "device"
        );
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
        fs::write(&file, value.to_string()).expect("write");
        let writer = PcClientState::new(file.clone(), Arc::new(TestCipher));
        writer.load().expect("load");
        let state = PcClientState::new(file, Arc::new(BrokenCipher));
        let error = state.load().expect_err("cannot open");
        assert_eq!(error.code, "secret_lost");
        state.forget().expect("forget");
        assert!(state.load().expect("load").is_none());
        fs::remove_dir_all(&directory).expect("cleanup");
    }

    #[test]
    fn published_connection_advances_generation_even_if_plaintext_cleanup_fails() {
        let (directory, file, value) = secret_fixture();
        let state = PcClientState::new(file.clone(), Arc::new(TestCipher));
        let old: SavedConnection = serde_json::from_value(value).unwrap();
        state.save(&old).unwrap();
        let (old_generation, _) = state.connection_snapshot().unwrap();
        fs::create_dir(&file).unwrap();
        let mut new = old;
        new.url = "https://new.example".into();
        assert_eq!(state.save(&new).unwrap_err().code, "storage");
        let (new_generation, current) = state.connection_snapshot().unwrap();
        assert_ne!(new_generation, old_generation);
        assert_eq!(current.unwrap().url, new.url);
        assert_eq!(
            state
                .with_connection(old_generation, || Ok(()))
                .unwrap_err()
                .code,
            "connection_changed"
        );
        fs::remove_dir(&file).unwrap();
        let reopened = PcClientState::new(file, Arc::new(TestCipher));
        assert_eq!(reopened.load().unwrap().unwrap().url, new.url);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_plain_file_stays_until_it_can_be_sealed() {
        let (directory, file, value) = secret_fixture();
        fs::write(&file, value.to_string()).expect("write");
        let state = PcClientState::new(file.clone(), Arc::new(BrokenCipher));
        assert_eq!(state.load().expect_err("cannot seal").code, "storage");
        assert!(file.exists());
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
        assert!(!loaded.can_sync_reading());
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
        assert!(loaded.can_sync_reading());
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
        assert!(validate_numbered_offset(None, None).is_ok());
        assert!(validate_numbered_offset(Some(0), None).is_ok());
        assert!(validate_numbered_offset(Some(MAX_NUMBERED_OFFSET), None).is_ok());
        assert!(validate_numbered_offset(Some(MAX_NUMBERED_OFFSET + 1), None).is_err());
        assert!(validate_numbered_offset(Some(-1), None).is_err());
        assert!(validate_numbered_offset(Some(20), Some("cursor")).is_err());
    }

    #[test]
    fn search_payload_splits_multiple_terms_and_forwards_non_ai_filters() {
        let criteria = search_criteria(serde_json::json!({
            "author": "作者A、作者B\n作者C", "tag": "猫, 海",
            "tagMode": "or", "minAssets": "2", "maxAssets": "12",
            "revision": "revised", "edit": "edited", "cover": "has_cover",
            "dateField": "source_created_at", "dateFrom": "2024-02-29", "dateTo": "2026-10-14",
            "sort": "publishedAsc"
        }));
        let payload = search_payload(&criteria, None, None, None).expect("payload");
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
        assert_eq!(payload["dateField"], "source_created_at");
        assert_eq!(payload["dateFrom"], "2024-02-29");
        assert_eq!(payload["dateTo"], "2026-10-14");
        assert_eq!(payload["sortBy"], "published");
        assert_eq!(payload["sortOrder"], "asc");
    }

    #[test]
    fn empty_new_filters_preserve_legacy_wire_shape() {
        let criteria = search_criteria(serde_json::json!({
            "tagMode": "and", "minAssets": "", "maxAssets": "",
            "revision": "", "edit": "", "cover": "",
            "dateField": "downloaded_at", "dateFrom": "", "dateTo": ""
        }));
        let payload = search_payload(&criteria, None, None, None).expect("payload");
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

    #[test]
    fn search_payload_rejects_invalid_ranges_dates_and_choices() {
        for changes in [
            serde_json::json!({ "author": "a,b,c,d,e,f,g,h,i,j,k" }),
            serde_json::json!({ "tag": "猫", "tagMode": "xor" }),
            serde_json::json!({ "minAssets": "3", "maxAssets": "2" }),
            serde_json::json!({ "minAssets": "-1" }),
            serde_json::json!({ "revision": "both" }),
            serde_json::json!({ "dateFrom": "2026-02-30" }),
            serde_json::json!({ "dateFrom": "2026-10-15", "dateTo": "2026-10-14" }),
            serde_json::json!({ "sort": "privatePath" }),
        ] {
            let criteria = search_criteria(changes.clone());
            assert!(
                search_payload(&criteria, None, None, None).is_err(),
                "accepted {changes}"
            );
        }
    }
}
