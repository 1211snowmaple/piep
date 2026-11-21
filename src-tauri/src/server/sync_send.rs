//! デスクトップ版から単独サーバーへの接続。管理者の口への問い合わせと、送信の進み具合。
//!
//! 棚の送信そのものは `shelf_send`。取得元の認証情報はこの経路を通らない。

use super::tls::certificate_fingerprint;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 管理者としてつないでいる単独サーバー。鍵は別の場所（資格情報マネージャー）に置く。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteServer {
    pub url: String,
    pub fingerprint: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendProgress {
    pub running: bool,
    /// snapshot / manifest / upload / database / commit / done / failed / canceled
    pub phase: String,
    pub files_total: u64,
    pub files_done: u64,
    pub bytes_total: u64,
    pub bytes_done: u64,
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct SendHandle {
    pub progress: Arc<Mutex<SendProgress>>,
    pub cancel: Arc<AtomicBool>,
}

impl SendHandle {
    pub fn new() -> Self {
        Self {
            progress: Arc::new(Mutex::new(SendProgress::default())),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(super) fn update(&self, change: impl FnOnce(&mut SendProgress)) {
        if let Ok(mut progress) = self.progress.lock() {
            change(&mut progress);
        }
    }

    pub(super) fn check_cancel(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(CANCELED.to_string())
        } else {
            Ok(())
        }
    }
}

impl Default for SendHandle {
    fn default() -> Self {
        Self::new()
    }
}

pub const CANCELED: &str = "送信を止めました";

pub(super) fn normalized_base(url: &str) -> String {
    url.trim_end_matches('/').to_string()
}

pub(super) fn net_error(error: reqwest::Error) -> String {
    // 誤り文に URL の問い合わせや鍵が混ざらないよう、種類だけを言う。
    if error.is_timeout() {
        "サーバーの応答が時間内にありませんでした".into()
    } else if error.is_connect() {
        "サーバーに接続できません。URL と通信状態を確認してください".into()
    } else {
        format!(
            "サーバーとの通信に失敗しました（{}）",
            error.status().map_or("通信".to_string(), |s| s.to_string())
        )
    }
}

/// 接続先を確かめて、HTTP クライアントを作る。
///
/// 指紋があれば、その証明書にだけつなぐ（自己署名）。無ければ OS の信頼する CA と
/// ホスト名で検証する。平文の http はこの機械の中だけ許す。
pub async fn build_client(remote: &RemoteServer) -> Result<reqwest::Client, String> {
    let parsed = url::Url::parse(&remote.url)
        .map_err(|_| "サーバーの URL が正しくありません".to_string())?;
    if !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err("サーバーの URL に利用者名や問い合わせは書けません".into());
    }
    let builder = reqwest::Client::builder()
        // 登録の鍵や本棚の本文を、307/308 の転送先へ再送しない。
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30 * 60))
        .no_proxy();
    match parsed.scheme() {
        "https" => {}
        "http" if matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")) => {
            return builder.build().map_err(net_error);
        }
        _ => return Err("サーバーの URL は https にしてください".into()),
    }
    let Some(fingerprint) = remote.fingerprint.as_deref() else {
        return builder.https_only(true).build().map_err(net_error);
    };
    // 鍵を送る前に証明書の指紋を照らし合わせる。ここでは hello しか呼ばない。
    let probe = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .tls_backend_rustls()
        .tls_danger_accept_invalid_certs(true)
        .tls_info(true)
        .no_proxy()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(net_error)?;
    let response = probe
        .get(format!("{}/api/v2/hello", normalized_base(&remote.url)))
        .send()
        .await
        .map_err(net_error)?;
    let certificate = response
        .extensions()
        .get::<reqwest::tls::TlsInfo>()
        .and_then(|info| info.peer_certificate())
        .ok_or("サーバーの証明書を確かめられません")?
        .to_vec();
    if certificate_fingerprint(&certificate) != fingerprint {
        return Err("サーバーの証明書が登録時と違います。接続先を確かめてください".into());
    }
    let pinned = reqwest::Certificate::from_der(&certificate).map_err(net_error)?;
    builder
        .tls_backend_rustls()
        .tls_certs_only([pinned])
        .tls_danger_accept_invalid_hostnames(true)
        .https_only(true)
        .build()
        .map_err(net_error)
}

/// 応答の状態コードを誤りに直す。サーバーの `message` は決まった文だけなのでそのまま出す。
pub(super) async fn check_status(response: reqwest::Response) -> Result<reqwest::Response, String> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("サーバーに登録が残っていません。登録し直してください".into());
    }
    let message = response
        .json::<serde_json::Value>()
        .await
        .ok()
        .and_then(|body| {
            body.get("message")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        });
    Err(message.unwrap_or_else(|| format!("サーバーが要求を断りました（{status}）")))
}

/// 管理者として登録する。一回限りの鍵を、端末の鍵に換える。
pub async fn pair_admin(
    remote: &RemoteServer,
    secret: &str,
    device_name: &str,
) -> Result<String, String> {
    let client = build_client(remote).await?;
    let response = client
        .post(format!("{}/api/v2/pair", normalized_base(&remote.url)))
        .json(&serde_json::json!({ "secret": secret, "deviceName": device_name }))
        .send()
        .await
        .map_err(net_error)?;
    let response = check_status(response).await?;
    let body: serde_json::Value = response.json().await.map_err(net_error)?;
    if body.get("role").and_then(|role| role.as_str()) != Some("admin") {
        return Err("この登録文字列は携帯用です".into());
    }
    body.get("token")
        .and_then(|token| token.as_str())
        .map(str::to_string)
        .ok_or_else(|| "サーバーの応答を読み取れません".to_string())
}

pub async fn admin_get(
    remote: &RemoteServer,
    token: &str,
    path: &str,
) -> Result<serde_json::Value, String> {
    let client = build_client(remote).await?;
    let response = client
        .get(format!(
            "{}/api/v2/admin/{path}",
            normalized_base(&remote.url)
        ))
        .bearer_auth(token)
        .send()
        .await
        .map_err(net_error)?;
    check_status(response)
        .await?
        .json()
        .await
        .map_err(net_error)
}

/// `admin_get` と同じだが、その口が無い（古いサーバーの）ときは `None` を返す。
pub async fn admin_get_if_present(
    remote: &RemoteServer,
    token: &str,
    path: &str,
) -> Result<Option<serde_json::Value>, String> {
    let client = build_client(remote).await?;
    let response = client
        .get(format!(
            "{}/api/v2/admin/{path}",
            normalized_base(&remote.url)
        ))
        .bearer_auth(token)
        .send()
        .await
        .map_err(net_error)?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    check_status(response)
        .await?
        .json()
        .await
        .map(Some)
        .map_err(net_error)
}

pub async fn admin_post(
    remote: &RemoteServer,
    token: &str,
    path: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let client = build_client(remote).await?;
    let response = client
        .post(format!(
            "{}/api/v2/admin/{path}",
            normalized_base(&remote.url)
        ))
        .bearer_auth(token)
        .json(body)
        .send()
        .await
        .map_err(net_error)?;
    check_status(response)
        .await?
        .json()
        .await
        .map_err(net_error)
}

/// 本文の要らない応答（204）を返す口へ JSON を置く。
pub async fn admin_put(
    remote: &RemoteServer,
    token: &str,
    path: &str,
    body: &serde_json::Value,
) -> Result<(), String> {
    let client = build_client(remote).await?;
    let response = client
        .put(format!(
            "{}/api/v2/admin/{path}",
            normalized_base(&remote.url)
        ))
        .bearer_auth(token)
        .json(body)
        .send()
        .await
        .map_err(net_error)?;
    check_status(response).await?;
    Ok(())
}

pub async fn admin_delete(remote: &RemoteServer, token: &str, path: &str) -> Result<(), String> {
    let client = build_client(remote).await?;
    let response = client
        .delete(format!(
            "{}/api/v2/admin/{path}",
            normalized_base(&remote.url)
        ))
        .bearer_auth(token)
        .send()
        .await
        .map_err(net_error)?;
    check_status(response).await?;
    Ok(())
}
