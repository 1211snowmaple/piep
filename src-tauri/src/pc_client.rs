//! PC 版 piep への通信を Android の Rust 側に閉じ込める。
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
use std::time::Duration;
use tauri::State;
use url::{Host, Url};

pub(crate) mod offline;
mod image_validation;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
const ASSET_TIMEOUT: Duration = Duration::from_secs(60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ASSET_BYTES: usize = 25 * 1024 * 1024;
const MAX_JSON_BYTES: usize = 16 * 1024 * 1024;
const MAX_NUMBERED_OFFSET: i64 = 5_000;
const API_VERSION: u32 = 2;

pub struct PcClientState {
    file: PathBuf,
}

impl PcClientState {
    pub fn new(file: PathBuf) -> Self {
        Self { file }
    }

    fn load(&self) -> Result<Option<SavedConnection>, ClientError> {
        let bytes = match fs::read(&self.file) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ClientError::storage()),
        };
        let saved: SavedConnection =
            serde_json::from_slice(&bytes).map_err(|_| ClientError::storage())?;
        validate_url(&saved.url)?;
        validate_fingerprint(&saved.fingerprint)?;
        if fingerprint(
            &URL_SAFE_NO_PAD
                .decode(&saved.certificate_der)
                .map_err(|_| ClientError::storage())?,
        ) != saved.fingerprint
        {
            return Err(ClientError::storage());
        }
        if URL_SAFE_NO_PAD
            .decode(&saved.token)
            .map_err(|_| ClientError::storage())?
            .len()
            != 32
        {
            return Err(ClientError::storage());
        }
        Ok(Some(saved))
    }

    fn save(&self, saved: &SavedConnection) -> Result<(), ClientError> {
        let bytes = serde_json::to_vec(saved).map_err(|_| ClientError::storage())?;
        let temporary = self.file.with_extension("json.tmp");
        fs::write(&temporary, bytes).map_err(|_| ClientError::storage())?;
        fs::rename(&temporary, &self.file).map_err(|_| ClientError::storage())?;
        Ok(())
    }
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
    fn storage() -> Self {
        Self {
            code: "storage",
            message: "端末内の接続設定を読み書きできません",
        }
    }
    fn offline_storage() -> Self {
        Self {
            code: "offline_storage",
            message: "保存した作品を読み書きできません。空き容量と保存状態を確認してください",
        }
    }
    fn offline() -> Self {
        Self {
            code: "offline",
            message: "PC に接続できません。同じ Wi-Fi と PC の共有状態を確認してください",
        }
    }
    fn certificate_changed() -> Self {
        Self {
            code: "certificate_changed",
            message: "PC の証明書が登録時と異なります",
        }
    }
    fn protocol() -> Self {
        Self {
            code: "protocol_mismatch",
            message: "PC 版の接続 API と互換性がありません",
        }
    }
    fn revoked() -> Self {
        Self {
            code: "revoked",
            message: "この端末の登録が PC 側で解除されています",
        }
    }
    fn remote() -> Self {
        Self {
            code: "remote_error",
            message: "PC の作品を読み込めません",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PairingPayload {
    version: u32,
    url: String,
    fingerprint: String,
    secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedConnection {
    url: String,
    fingerprint: String,
    certificate_der: String,
    token: String,
    device_id: String,
    device_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionStatus {
    paired: bool,
    reachable: bool,
    url: Option<String>,
    device_name: Option<String>,
    issue: Option<&'static str>,
}

impl ConnectionStatus {
    fn empty() -> Self {
        Self {
            paired: false,
            reachable: false,
            url: None,
            device_name: None,
            issue: None,
        }
    }
    fn from_saved(saved: &SavedConnection, reachable: bool, issue: Option<&'static str>) -> Self {
        Self {
            paired: true,
            reachable,
            url: Some(saved.url.clone()),
            device_name: Some(saved.device_name.clone()),
            issue,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Hello {
    api_version: u32,
    server_name: String,
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
    let allowed_ip = match url.host() {
        Some(Host::Ipv4(ip)) => ip.is_private() || ip.is_loopback(),
        _ => false,
    };
    if url.scheme() != "https"
        || !allowed_ip
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ClientError::invalid_pairing());
    }
    Ok(url)
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
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| ClientError::certificate_changed())
}

async fn hello(client: &reqwest::Client, base: &str) -> Result<reqwest::Response, ClientError> {
    let response = client
        .get(format!("{base}/api/v1/hello"))
        .send()
        .await
        .map_err(|_| ClientError::offline())?;
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
    let pairing: PairingPayload =
        serde_json::from_str(&payload).map_err(|_| ClientError::invalid_pairing())?;
    if pairing.version != 1 || device_name.trim().is_empty() || device_name.chars().count() > 64 {
        return Err(ClientError::invalid_pairing());
    }
    validate_url(&pairing.url)?;
    validate_fingerprint(&pairing.fingerprint)?;
    if URL_SAFE_NO_PAD
        .decode(&pairing.secret)
        .map_err(|_| ClientError::invalid_pairing())?
        .len()
        != 32
    {
        return Err(ClientError::invalid_pairing());
    }
    let probe = probe_client()?;
    // 無認証の接続確認だけを先に行い、登録鍵を送る前に証明書を照合する。
    let response = hello(&probe, &pairing.url).await?;
    let certificate = certificate_from_response(&response)?;
    if fingerprint(&certificate) != pairing.fingerprint.to_ascii_lowercase() {
        return Err(ClientError::certificate_changed());
    }
    let hello: Hello = response.bounded_json().await?;
    if hello.api_version != API_VERSION || hello.server_name != "piep" {
        return Err(ClientError::protocol());
    }
    let pinned = pinned_client(&certificate)?;
    let response = pinned
        .post(format!("{}/api/v1/pair", pairing.url))
        .json(&serde_json::json!({"secret": pairing.secret, "deviceName": device_name.trim()}))
        .send()
        .await
        .map_err(|_| ClientError::offline())?;
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
        fingerprint: pairing.fingerprint.to_ascii_lowercase(),
        certificate_der: URL_SAFE_NO_PAD.encode(certificate),
        token: result.token,
        device_id: result.device_id,
        device_name: device_name.trim().to_string(),
    };
    state.save(&saved)?;
    Ok(ConnectionStatus::from_saved(&saved, true, None))
}

#[tauri::command]
pub async fn pc_connection_status(
    state: State<'_, PcClientState>,
) -> Result<ConnectionStatus, ClientError> {
    let Some(saved) = state.load()? else {
        return Ok(ConnectionStatus::empty());
    };
    let certificate = URL_SAFE_NO_PAD
        .decode(&saved.certificate_der)
        .map_err(|_| ClientError::storage())?;
    let pinned = pinned_client(&certificate)?;
    match hello(&pinned, &saved.url).await {
        Ok(response) => {
            let hello: Hello = response.bounded_json().await?;
            if hello.api_version != API_VERSION || hello.server_name != "piep" {
                Ok(ConnectionStatus::from_saved(
                    &saved,
                    false,
                    Some("protocol_mismatch"),
                ))
            } else {
                // /hello は無認証。登録解除された端末を「接続中」と表示しない。
                let authorized = pinned
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
                    Ok(_) => Some("remote_error"),
                    Err(_) => Some("offline"),
                };
                Ok(ConnectionStatus::from_saved(&saved, issue.is_none(), issue))
            }
        }
        Err(_) => {
            // 証明書が変わった場合と、PC が停止している場合を区別する。
            let issue = match probe_client() {
                Ok(probe) => match hello(&probe, &saved.url).await {
                    Ok(response) => match certificate_from_response(&response) {
                        Ok(current) if fingerprint(&current) != saved.fingerprint => {
                            "certificate_changed"
                        }
                        _ => "offline",
                    },
                    Err(_) => "offline",
                },
                Err(_) => "offline",
            };
            Ok(ConnectionStatus::from_saved(&saved, false, Some(issue)))
        }
    }
}

#[tauri::command]
pub fn pc_disconnect(state: State<'_, PcClientState>) -> Result<ConnectionStatus, ClientError> {
    match fs::remove_file(&state.file) {
        Ok(()) => Ok(ConnectionStatus::empty()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ConnectionStatus::empty()),
        Err(_) => Err(ClientError::storage()),
    }
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
    let certificate = URL_SAFE_NO_PAD
        .decode(&saved.certificate_der)
        .map_err(|_| ClientError::storage())?;
    let client = pinned_client(&certificate)?;
    let response = client
        .get(format!("{}{}", saved.url, endpoint))
        .bearer_auth(saved.token)
        .timeout(timeout)
        .send()
        .await
        .map_err(|_| ClientError::offline())?;
    check_response(response)
}

fn check_response(response: reqwest::Response) -> Result<reqwest::Response, ClientError> {
    match response.status() {
        reqwest::StatusCode::UNAUTHORIZED => Err(ClientError::revoked()),
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
    if value.chars().count() > 1_100 { return Err(ClientError::remote()); }
    let terms: Vec<String> = value
        .split(|character| matches!(character, '\n' | '\r' | ',' | '、' | '，'))
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
    if value.is_empty() { return Ok(None); }
    if value.len() > 10 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ClientError::remote());
    }
    let count = value.parse::<i64>().map_err(|_| ClientError::remote())?;
    if count > 1_000_000_000 { return Err(ClientError::remote()); }
    Ok(Some(count))
}

fn optional_search_choice<'a>(value: Option<&'a str>, choices: &[&str]) -> Result<Option<&'a str>, ClientError> {
    match value.unwrap_or("") {
        "" => Ok(None),
        value if choices.contains(&value) => Ok(Some(value)),
        _ => Err(ClientError::remote()),
    }
}

fn valid_search_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-'
        || !bytes.iter().enumerate().all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
    { return false; }
    let year = value[0..4].parse::<u32>().unwrap_or(0);
    let month = value[5..7].parse::<u32>().unwrap_or(0);
    let day = value[8..10].parse::<u32>().unwrap_or(0);
    if year == 0 { return false; }
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
    let content_type = optional_search_choice(Some(&criteria.content_type), &["novel", "article", "text", "image", "file"])?;
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
        _ => return Err(ClientError::remote()),
    };
    let min_chars = parse_search_count(&criteria.min_chars)?;
    let max_chars = parse_search_count(&criteria.max_chars)?;
    let min_assets = parse_search_count(criteria.min_assets.as_deref().unwrap_or(""))?;
    let max_assets = parse_search_count(criteria.max_assets.as_deref().unwrap_or(""))?;
    if min_chars.zip(max_chars).is_some_and(|(min, max)| min > max)
        || min_assets.zip(max_assets).is_some_and(|(min, max)| min > max)
    {
        return Err(ClientError::remote());
    }
    let tag_mode = optional_search_choice(criteria.tag_mode.as_deref(), &["and", "or"])?;
    let revision = optional_search_choice(criteria.revision.as_deref(), &["revised", "first_version"])?;
    let edit = optional_search_choice(criteria.edit.as_deref(), &["edited", "unedited"])?;
    let cover = optional_search_choice(criteria.cover.as_deref(), &["has_cover", "no_cover"])?;
    let date_field = optional_search_choice(criteria.date_field.as_deref(), &["downloaded_at", "source_created_at", "source_updated_at"])?;
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
    if let Some(offset) = offset { payload["offset"] = serde_json::json!(offset); }
    if tag_mode == Some("or") && payload["tagsInclude"].is_array() { payload["tagFilterMode"] = serde_json::json!("or"); }
    if let Some(count) = min_assets { payload["minAssetCount"] = serde_json::json!(count); }
    if let Some(count) = max_assets { payload["maxAssetCount"] = serde_json::json!(count); }
    if let Some(value) = revision { payload["revisionFilter"] = serde_json::json!(value); }
    if let Some(value) = edit { payload["editFilter"] = serde_json::json!(value); }
    if let Some(value) = cover { payload["coverFilter"] = serde_json::json!(value); }
    if !date_from.is_empty() || !date_to.is_empty() {
        payload["dateField"] = serde_json::json!(date_field.unwrap_or("downloaded_at"));
        if !date_from.is_empty() { payload["dateFrom"] = serde_json::json!(date_from); }
        if !date_to.is_empty() { payload["dateTo"] = serde_json::json!(date_to); }
    }
    // Keep the original request shape for PC builds that predate these filters.
    for key in ["authorsExclude", "tagsExclude", "minCharCount", "maxCharCount"] {
        if payload[key].is_null() { payload.as_object_mut().unwrap().remove(key); }
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
) -> Result<SearchPage, ClientError> {
    let payload = search_payload(&criteria, cursor, offset, scope)?;
    let saved = state.load()?.ok_or_else(ClientError::invalid_pairing)?;
    let certificate = URL_SAFE_NO_PAD
        .decode(&saved.certificate_der)
        .map_err(|_| ClientError::storage())?;
    let client = pinned_client(&certificate)?;
    let response = client
        .post(format!("{}/api/v1/works/search", saved.url))
        .bearer_auth(saved.token)
        .json(&payload)
        .send()
        .await
        .map_err(|_| ClientError::offline())?;
    check_response(response)?
        .bounded_json().await
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

#[tauri::command]
pub async fn pc_list_entities(
    state: State<'_, PcClientState>,
    kind: String,
    text: String,
    offset: i64,
) -> Result<MobileEntityPage, ClientError> {
    if !matches!(kind.as_str(), "people" | "series")
        || text.chars().count() > 200
        || !(0..=1_000_000).contains(&offset)
    {
        return Err(ClientError::remote());
    }
    let query_string = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("text", text.trim());
        query.append_pair("offset", &offset.to_string());
        query.append_pair("limit", "30");
        query.finish()
    };
    let endpoint = format!("/api/v1/entities/{kind}?{query_string}");
    authenticated_get(&state, &endpoint)
        .await?
        .bounded_json().await
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
        .bounded_json().await
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
        .bounded_json().await
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
        .bounded_json().await
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
        .bounded_json().await
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobilePage {
    page: usize,
    page_count: usize,
    html: String,
    total_plain_text_chars: usize,
    source_page_starts: Vec<usize>,
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
        .bounded_json().await
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
    .bounded_json().await
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
    if response.content_length().is_some_and(|length| length > maximum as u64) {
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

#[tauri::command]
pub async fn pc_get_cover(
    state: State<'_, PcClientState>,
    work_id: i64,
) -> Result<MobileAssetData, ClientError> {
    if work_id <= 0 {
        return Err(ClientError::remote());
    }
    let response = authenticated_get_with_timeout(
        &state,
        &format!("/api/v1/works/{work_id}/cover"),
        ASSET_TIMEOUT,
    )
    .await?;
    image_response(response).await
}

#[tauri::command]
pub async fn pc_get_collection_cover(
    state: State<'_, PcClientState>,
    collection_id: String,
    slot: String,
) -> Result<MobileAssetData, ClientError> {
    if collection_id.is_empty()
        || collection_id.len() > 128
        || !collection_id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        || !(slot == "file" || matches!(slot.as_str(), "tile-0" | "tile-1" | "tile-2" | "tile-3"))
    {
        return Err(ClientError::remote());
    }
    let response = authenticated_get_with_timeout(
        &state,
        &format!("/api/v1/collections/{collection_id}/cover/{slot}"),
        ASSET_TIMEOUT,
    ).await?;
    image_response(response).await
}

#[tauri::command]
pub async fn pc_get_entity_cover(
    state: State<'_, PcClientState>,
    kind: String,
    source: String,
    key: String,
) -> Result<MobileAssetData, ClientError> {
    if !matches!(kind.as_str(), "people" | "series")
        || !matches!(source.as_str(), "pixiv" | "fanbox")
        || key.is_empty()
        || key.chars().count() > 200
    {
        return Err(ClientError::remote());
    }
    let query_string = {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("source", &source);
        query.append_pair("key", &key);
        query.finish()
    };
    let endpoint = format!("/api/v1/entities/{kind}/cover?{query_string}");
    let response = authenticated_get_with_timeout(&state, &endpoint, ASSET_TIMEOUT).await?;
    image_response(response).await
}

#[tauri::command]
pub async fn pc_get_asset(
    state: State<'_, PcClientState>,
    work_id: i64,
    asset_id: i64,
) -> Result<MobileAssetData, ClientError> {
    if work_id <= 0 || asset_id <= 0 {
        return Err(ClientError::remote());
    }
    let response = authenticated_get_with_timeout(
        &state,
        &format!("/api/v1/works/{work_id}/assets/{asset_id}"),
        ASSET_TIMEOUT,
    )
    .await?;
    image_response(response).await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn local_response(wire_response: &'static str) -> reqwest::Response {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(wire_response.as_bytes());
        });
        reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(3))
            .build().unwrap().get(format!("http://{address}/"))
            .send().await.unwrap()
    }

    #[test]
    fn bounds_response_body_with_and_without_content_length() {
        tauri::async_runtime::block_on(async {
            let response = local_response("HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\n\r\n").await;
            assert_eq!(bounded_response_bytes(response, 4).await.unwrap_err().code, "remote_error");
            let response = local_response("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n3\r\ndef\r\n0\r\n\r\n").await;
            assert_eq!(bounded_response_bytes(response, 4).await.unwrap_err().code, "remote_error");
            let response = local_response("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nab\r\n2\r\ncd\r\n0\r\n\r\n").await;
            assert_eq!(bounded_response_bytes(response, 4).await.unwrap(), b"abcd");
        });
    }

    #[test]
    fn json_decoder_bounds_http_bodies_and_reports_invalid_json() {
        tauri::async_runtime::block_on(async {
            let response = local_response("HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\n\r\n").await;
            assert_eq!(response.bounded_json::<serde_json::Value>().await.unwrap_err().code, "remote_error");
            let response = local_response("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n[1,2\r\n5\r\n,3,4]\r\n0\r\n\r\n").await;
            assert_eq!(bounded_json_response::<serde_json::Value>(response, 8).await.unwrap_err().code, "remote_error");
            let response = local_response("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"v\"\r\n3\r\n:1}\r\n0\r\n\r\n").await;
            assert_eq!(bounded_json_response::<serde_json::Value>(response, 7).await.unwrap(), serde_json::json!({"v": 1}));
            let response = local_response("HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nbad").await;
            assert_eq!(response.bounded_json::<serde_json::Value>().await.unwrap_err().code, "protocol_mismatch");
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
    fn rejects_non_private_or_non_https_pairing_targets() {
        for address in [
            "http://192.168.1.2:38777",
            "https://example.com:38777",
            "https://8.8.8.8:38777",
            "https://127.0.0.1:38777/path",
            "https://127.0.0.1:38777/?x=1",
        ] {
            assert!(validate_url(address).is_err(), "{address}");
        }
        assert!(validate_url("https://192.168.1.2:38777").is_ok());
        assert!(validate_url("https://127.0.0.1:38777").is_ok());
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
        assert_eq!(payload["authorsInclude"], serde_json::json!(["作者A", "作者B", "作者C"]));
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
        for key in ["tagFilterMode", "minAssetCount", "maxAssetCount", "revisionFilter", "editFilter", "coverFilter", "dateField", "dateFrom", "dateTo"] {
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
            assert!(search_payload(&criteria, None, None, None).is_err(), "accepted {changes}");
        }
    }
}
