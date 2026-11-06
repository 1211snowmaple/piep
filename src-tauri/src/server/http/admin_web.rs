//! 管理画面。ログイン、別のサイトからの要求の守り、画面のファイル、管理の設定の API。
//!
//! 出すのは `standalone` が `AdminWeb` を渡したときだけで、読み手の口と内蔵サーバーには載らない。
//! 設計の理由は `docs/policy/06-backend.md` に書いてある。

use super::super::admin_store::{TicketKind, RECENT_AUTH};
use super::super::standalone::ServerPaths;
use super::super::store::Role;
use super::super::update::{self, UpdateController};
use super::{require_admin, ApiError, ApiJson, Ctx, ServerContext};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use axum::body::Body;
use axum::extract::{ConnectInfo, Path as UrlPath, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

const COOKIE: &str = "__Host-piep_admin";
const LOGIN_TICKET: Duration = Duration::from_secs(60);
pub const SETUP_TICKET: Duration = Duration::from_secs(30 * 60);
const AUDIT_PAGE: i64 = 200;
const MAX_NAME_CHARS: usize = 64;
const MIN_PASSWORD_CHARS: usize = 15;
const MAX_PASSWORD_CHARS: usize = 256;
/// 誤りの文は原因によらず同じにする。
const LOGIN_FAILED: &str = "ログインできませんでした";
const STORAGE_CACHE: Duration = Duration::from_secs(10 * 60);

/// 画面のファイル。`build.rs` が作る一覧の 1 行。
pub struct AdminAsset {
    pub path: &'static str,
    pub mime: &'static str,
    pub bytes: &'static [u8],
}

/// 管理の口の設定。読み取りで見せる。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub listen: String,
    pub admin_listen: Option<String>,
    pub public_url: Option<String>,
    pub tls: &'static str,
}

/// 管理画面に要るもの。これがある口にだけ画面を出す。
pub struct AdminWeb {
    assets: &'static [AdminAsset],
    paths: ServerPaths,
    connection: ConnectionInfo,
    direct_tls: bool,
    throttle: Mutex<LoginThrottle>,
    /// ハッシュの計算でメモリを使い切らせない。
    hash_slots: Semaphore,
    storage: Mutex<Option<(Instant, serde_json::Value)>>,
}

impl AdminWeb {
    pub fn new(
        assets: &'static [AdminAsset],
        paths: ServerPaths,
        connection: ConnectionInfo,
    ) -> Self {
        let direct_tls = connection.tls != "none";
        Self {
            assets,
            paths,
            connection,
            direct_tls,
            throttle: Mutex::new(LoginThrottle::default()),
            hash_slots: Semaphore::new(2),
            storage: Mutex::new(None),
        }
    }
}

/// ログインの失敗の数え方。
///
/// アカウントは 1 つなので、失敗は名前によらず 1 か所で数える。IP では数えない。
/// トンネルの裏では全部が同じ IP に見え、転送のヘッダーも信じないためである。
/// 券は数えないので、他人が失敗を重ねても持ち主は入れる。
#[derive(Debug, Default)]
pub struct LoginThrottle {
    failures: u32,
    until: Option<Instant>,
}

impl LoginThrottle {
    const FREE_FAILURES: u32 = 5;
    const FIRST_WAIT: Duration = Duration::from_secs(60);
    const LONGEST_WAIT: Duration = Duration::from_secs(15 * 60);

    /// 待ちの残り。
    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        self.until
            .and_then(|until| until.checked_duration_since(now))
            .filter(|left| !left.is_zero())
    }

    pub fn fail(&mut self, now: Instant) {
        self.failures += 1;
        if self.failures >= Self::FREE_FAILURES {
            let doubled = self.failures - Self::FREE_FAILURES;
            let wait = Self::FIRST_WAIT
                .saturating_mul(2_u32.saturating_pow(doubled.min(8)))
                .min(Self::LONGEST_WAIT);
            self.until = Some(now + wait);
        }
    }

    pub fn succeed(&mut self) {
        self.failures = 0;
        self.until = None;
    }
}

/// よく使われるパスワードのうち、15 文字以上のもの。短いものは長さの決まりで落ちる。
const COMMON_PASSWORDS: &[&str] = &[
    "passwordpassword",
    "password1234567",
    "password12345678",
    "1234567890123456",
    "qwertyuiopasdfgh",
    "qwertyuiopasdfghjkl",
    "qwertyuiop123456",
    "1qaz2wsx3edc4rfv",
    "1q2w3e4r5t6y7u8i",
    "administrator123",
    "adminadminadmin",
    "letmeinletmein1",
    "iloveyouiloveyou",
    "welcome123456789",
    "changemechangeme",
    "correcthorsebatterystaple",
];

const KEYBOARD_ROWS: &[&str] = &[
    "1234567890",
    "0987654321",
    "qwertyuiop",
    "poiuytrewq",
    "asdfghjkl",
    "lkjhgfdsa",
    "zxcvbnm",
    "mnbvcxz",
    "abcdefghijklmnopqrstuvwxyz",
    "zyxwvutsrqponmlkjihgfedcba",
];

/// パスワードの決まり。文字の種類は問わない。
pub fn check_password(password: &str) -> Result<(), &'static str> {
    let count = password.chars().count();
    if count < MIN_PASSWORD_CHARS {
        return Err("パスワードは 15 文字以上にしてください");
    }
    if count > MAX_PASSWORD_CHARS {
        return Err("パスワードは 256 文字までにしてください");
    }
    let lowered = password.to_lowercase();
    if COMMON_PASSWORDS.contains(&lowered.as_str()) {
        return Err("よく使われるパスワードです。別のものにしてください");
    }
    // 短い並びの繰り返し。aaaa… や abcabc… を断る。
    let chars: Vec<char> = lowered.chars().collect();
    let repeats = (1..=6).any(|period| {
        chars.len() >= period * 2
            && chars
                .iter()
                .enumerate()
                .all(|(index, character)| *character == chars[index % period])
    });
    if repeats {
        return Err("同じ並びの繰り返しです。別のものにしてください");
    }
    // キーボードや数字の順に沿うだけのもの。
    let along_a_row = KEYBOARD_ROWS.iter().any(|row| {
        let doubled = row.repeat(2 + chars.len() / row.len().max(1));
        doubled.contains(&lowered)
    });
    if along_a_row {
        return Err("順番に並んだ文字だけです。別のものにしてください");
    }
    Ok(())
}

fn argon2() -> Result<Argon2<'static>, String> {
    // OWASP が示す Argon2id の下限。確かめるときは、ハッシュに書かれた値を使う。
    let params = Params::new(19_456, 2, 1, Some(32)).map_err(|error| error.to_string())?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

pub fn hash_password(password: &str) -> Result<String, String> {
    let mut salt = [0_u8; 16];
    rand::fill(&mut salt);
    let salt = SaltString::encode_b64(&salt).map_err(|error| error.to_string())?;
    argon2()?
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| error.to_string())
}

pub fn verify_password(password: &str, stored: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored) else {
        return false;
    };
    let Ok(hasher) = argon2() else {
        return false;
    };
    hasher.verify_password(password.as_bytes(), &parsed).is_ok()
}

fn web(ctx: &ServerContext) -> Result<&AdminWeb, ApiError> {
    if ctx.store.admin_web_disabled() {
        return Err(ApiError::not_found());
    }
    ctx.web.get().ok_or_else(ApiError::not_found)
}

fn cookie_value(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == COOKIE).then_some(value)
        })
}

/// Cookie のセッションを引く。管理画面を出していない口では、Cookie を見ない。
pub(super) fn session_from(
    ctx: &ServerContext,
    headers: &HeaderMap,
) -> Option<super::super::admin_store::AdminSession> {
    web(ctx).ok()?;
    let touch = !headers.contains_key("x-piep-background");
    ctx.store
        .authorize_admin_session(cookie_value(headers)?, touch)
}

fn session_cookie(token: &str) -> HeaderValue {
    let max_age = super::super::admin_store::SESSION_LIFETIME.as_secs();
    HeaderValue::from_str(&format!(
        "{COOKIE}={token}; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age}"
    ))
    .expect("鍵は URL に使える文字だけである")
}

const CLEARED_COOKIE: &str =
    "__Host-piep_admin=; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age=0";

fn client_ip(request: &Request) -> ClientIp {
    ClientIp(
        request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0.ip().to_string()),
    )
}

/// 見えた IP。トンネルの裏では同じになるので、記録にだけ使う。
#[derive(Debug, Clone)]
pub(super) struct ClientIp(Option<String>);

impl ClientIp {
    fn get(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

fn request_host(request: &Request) -> Option<String> {
    request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .or_else(|| request.uri().authority().map(|value| value.to_string()))
}

/// `Host` からポートを除く。
pub fn host_name(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    host.rsplit_once(':').map_or(host, |(name, _)| name)
}

/// IP と localhost はいつでも通す。名前は、覚えたものか `public_url` のものだけを通す。
/// 攻撃者の名前を 127.0.0.1 へ向けて口を読ませる手口（DNS rebinding）を止めるためである。
/// 名前を覚えるのは、管理者の鍵でリンクを発行したときと、券が通ったときである。
fn host_allowed(ctx: &ServerContext, host: &str) -> bool {
    let name = host_name(host).to_ascii_lowercase();
    if name == "localhost" || name.parse::<IpAddr>().is_ok() {
        return true;
    }
    if ctx.store.admin_hosts().contains(&name) {
        return true;
    }
    ctx.public_url
        .as_deref()
        .and_then(|url| url.split("://").nth(1))
        .and_then(|rest| rest.split('/').next())
        .is_some_and(|public| host_name(public).eq_ignore_ascii_case(&name))
}

/// 別のサイトからの要求か。`Sec-Fetch-Site` を先に見て、無ければ `Origin` を完全一致で見る。
fn same_origin(headers: &HeaderMap, host: &str) -> bool {
    if let Some(site) = headers.get("sec-fetch-site") {
        return site.as_bytes() == b"same-origin";
    }
    headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .and_then(|origin| origin.split_once("://"))
        .is_some_and(|(_, rest)| rest.eq_ignore_ascii_case(host))
}

fn rejected(code: &'static str, message: &'static str) -> Response {
    ApiError(StatusCode::FORBIDDEN, code, message).into_response()
}

/// 管理の要求すべての前に通す。鍵（Bearer）の要求は Cookie を使わないので見ない。
async fn guard(State(ctx): State<Ctx>, mut request: Request, next: Next) -> Response {
    let ip = client_ip(&request);
    request.extensions_mut().insert(ip);
    if request.headers().contains_key(header::AUTHORIZATION) {
        return next.run(request).await;
    }
    if ctx.web.get().is_some() {
        let Some(host) = request_host(&request) else {
            return rejected("host_rejected", "このアドレスでは開けません");
        };
        if !host_allowed(&ctx, &host) {
            if wants_page(&request) {
                return host_rejected_page(&host);
            }
            return rejected(
                "host_rejected",
                "このアドレスでは開けません。デスクトップ版の「管理画面を開く」か、サーバーの admin-setup --url から開いてください",
            );
        }
        let safe = matches!(request.method(), &Method::GET | &Method::HEAD);
        if !safe && !same_origin(request.headers(), &host) {
            return rejected("cross_site", "別のサイトからの操作は受けません");
        }
    }
    next.run(request).await
}

/// ブラウザでページとして開いたか。読み込みの部品や API には JSON で答える。
fn wants_page(request: &Request) -> bool {
    if request.method() != Method::GET {
        return false;
    }
    let headers = request.headers();
    if let Some(dest) = headers.get("sec-fetch-dest") {
        return dest.as_bytes() == b"document";
    }
    headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("text/html"))
}

/// 知らない名前で開かれたときのページ。アプリのアイコンを丸ごと出し、理由と開き方を書く。
/// 外のファイルを読まない。読み込みの部品も、同じ理由で断られるからである。
fn host_rejected_page(host: &str) -> Response {
    use base64::Engine as _;
    static PARTS: std::sync::OnceLock<(String, String)> = std::sync::OnceLock::new();
    let (favicon, icon) = PARTS.get_or_init(|| {
        let encode = |svg: &str| {
            format!(
                "data:image/svg+xml;base64,{}",
                base64::engine::general_purpose::STANDARD.encode(svg)
            )
        };
        (
            encode(include_str!("../../../../public/piep.svg")),
            encode(include_str!("../../../../src/assets/icon.svg")),
        )
    });
    // Host は相手が自由に書けるので、そのまま埋め込まない。
    let page = HOST_REJECTED_PAGE
        .replace("{favicon}", favicon)
        .replace("{icon}", icon)
        .replace("{host}", &crate::database::parser::escape_html(host));
    (
        StatusCode::FORBIDDEN,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        page,
    )
        .into_response()
}

/// 色と書体は、アプリの背景（`app.css`）と入口のカード（`admin.css`）に合わせる。
const HOST_REJECTED_PAGE: &str = r#"<!doctype html>
<html lang="ja">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex, nofollow">
<link rel="icon" type="image/svg+xml" href="{favicon}">
<title>piep 管理画面</title>
<style>
:root { color-scheme: light dark; --bg: #f5f7fa; --card: #fff; --line: #e4e8ee; --text: #1b1f24; --dim: #6b7380; --code: #f1f3f5; --accent: #0d86f4; }
@media (prefers-color-scheme: dark) { :root { --bg: #0d1117; --card: #161b22; --line: #252b35; --text: #e6e9ee; --dim: #9aa3af; --code: #1f2630; --accent: #59b8ff; } }
* { box-sizing: border-box; }
html, body { margin: 0; }
body { min-height: 100vh; min-height: 100dvh; display: grid; place-items: center; padding: 16px; background: var(--bg); color: var(--text);
  font-family: "Segoe UI", "Yu Gothic UI", Meiryo, "Hiragino Sans", system-ui, -apple-system, sans-serif; font-size: 14px; line-height: 1.7;
  word-break: auto-phrase; overflow-wrap: anywhere; }
main { width: min(420px, 100%); padding: 28px; border: 1px solid var(--line); border-radius: 16px; background: var(--card); }
.brand { display: flex; justify-content: center; margin-bottom: 24px; }
.brand img { width: 52px; height: 52px; }
h1 { margin: 0 0 6px; font-size: 20px; font-weight: 700; line-height: 1.35; }
.lead { margin: 0 0 20px; color: var(--dim); }
h2 { margin: 0 0 6px; font-size: 14px; font-weight: 650; }
ol { margin: 0 0 20px; padding-left: 1.4em; line-height: 1.9; }
.note { margin: 0; padding-top: 16px; border-top: 1px solid var(--line); color: var(--dim); font-size: 12.5px; }
.note p { margin: 0 0 8px; }
.note p:last-child { margin-bottom: 0; }
.host { color: var(--text); font-weight: 600; }
code { display: block; margin-top: 4px; padding: 8px 10px; border-radius: 8px; background: var(--code); color: var(--text);
  font-family: "Cascadia Code", "SFMono-Regular", Consolas, monospace; font-size: 12px; line-height: 1.6; overflow-wrap: anywhere; }
</style>
</head>
<body>
<main>
<div class="brand"><img src="{icon}" alt="piep" width="52" height="52"></div>
<h1>このアドレスでは開けません</h1>
<p class="lead">安全のため、管理画面は、デスクトップ版から一度開いたアドレスでだけ開けます。</p>
<h2>開き方</h2>
<ol>
<li>デスクトップ版で「設定」→「サーバー」を開く</li>
<li>「管理画面を開く」を押す</li>
</ol>
<div class="note">
<p>一度開くと、このアドレスを覚えます。そのあとは、ほかの PC や携帯のブラウザからもこのアドレスで開けます。</p>
<p>開こうとしたアドレス: <span class="host">{host}</span></p>
<p>デスクトップ版を使っていない場合は、サーバーの PC で次を実行し、表示されたリンクを開いてください。<code>piep-server admin-setup --data &lt;置き場所&gt; --url &lt;このアドレス&gt;</code></p>
</div>
</main>
</body>
</html>
"#;

/// `admin_routes` に重ねる守り。
pub(super) fn with_guard(router: Router, ctx: Ctx) -> Router {
    router.layer(middleware::from_fn_with_state(ctx, guard))
}

/// 画面と API の応答に付ける印。
async fn secure_headers(State(ctx): State<Ctx>, request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    // Mantine は実行時にスタイルを差し込むので、style だけ inline を許す。
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'",
        ),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if ctx.web.get().is_some_and(|web| web.direct_tls) {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        );
    }
    response
}

fn storage_error<T>(error: T) -> ApiError
where
    T: std::fmt::Display,
{
    log::error!("管理画面の状態を読み書きできません: {error}");
    ApiError::internal()
}

fn actor(headers: &HeaderMap) -> &'static str {
    if headers.contains_key(header::AUTHORIZATION) {
        "デスクトップ版"
    } else {
        "ログイン中のブラウザ"
    }
}

/// 大事な操作の前に通す。鍵で来たデスクトップ版には聞かない。
fn require_recent(ctx: &ServerContext, headers: &HeaderMap) -> Result<(), ApiError> {
    require_admin(ctx, headers)?;
    if headers.contains_key(header::AUTHORIZATION) {
        return Ok(());
    }
    let recent = session_from(ctx, headers)
        .and_then(|session| session.recent_auth_at)
        .is_some_and(|at| {
            chrono::Utc::now().timestamp_millis() - at < RECENT_AUTH.as_millis() as i64
        });
    if recent {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::FORBIDDEN,
            "reauth_required",
            "もう一度パスワードを入力してください",
        ))
    }
}

async fn hashed(web: &AdminWeb, password: String) -> Result<String, ApiError> {
    let _slot = web
        .hash_slots
        .acquire()
        .await
        .map_err(|_| ApiError::internal())?;
    tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(storage_error)
}

async fn verified(web: &AdminWeb, password: String, stored: String) -> Result<bool, ApiError> {
    let _slot = web
        .hash_slots
        .acquire()
        .await
        .map_err(|_| ApiError::internal())?;
    tokio::task::spawn_blocking(move || verify_password(&password, &stored))
        .await
        .map_err(|_| ApiError::internal())
}

fn signed_in(token: &str, body: serde_json::Value) -> Response {
    let mut response = Json(body).into_response();
    response
        .headers_mut()
        .insert(header::SET_COOKIE, session_cookie(token));
    response
}

fn user_agent(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
}

async fn web_state(State(ctx): State<Ctx>, headers: HeaderMap) -> Result<Response, ApiError> {
    web(&ctx)?;
    let account = ctx.store.admin_account().map_err(storage_error)?;
    let session = session_from(&ctx, &headers);
    Ok(Json(serde_json::json!({
        "account": account.is_some(),
        "authenticated": session.is_some(),
        "serverName": session.as_ref().map(|_| ctx.store.server_name()),
        "username": session.as_ref().and_then(|_| account.map(|account| account.username)),
        "sessionId": session.as_ref().map(|session| session.id.clone()),
    }))
    .into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LoginRequest {
    username: String,
    password: String,
}

async fn web_login(
    State(ctx): State<Ctx>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
    ApiJson(login): ApiJson<LoginRequest>,
) -> Result<Response, ApiError> {
    let site = web(&ctx)?;
    let wait = site
        .throttle
        .lock()
        .map_err(|_| ApiError::internal())?
        .remaining(Instant::now());
    if let Some(wait) = wait {
        ctx.store.audit(
            "login",
            "ログイン",
            "ログイン画面",
            ip.get(),
            false,
            Some("待ち時間の間の試行"),
        );
        let mut response = (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "code": "too_many_attempts",
                "message": LOGIN_FAILED,
                "retryAfterSeconds": wait.as_secs() + 1,
            })),
        )
            .into_response();
        if let Ok(value) = HeaderValue::from_str(&(wait.as_secs() + 1).to_string()) {
            response.headers_mut().insert(header::RETRY_AFTER, value);
        }
        return Ok(response);
    }
    let account = ctx.store.admin_account().map_err(storage_error)?;
    // 名前が違っても同じだけ計算して、応答の時間で見分けられないようにする。
    let ok = match &account {
        Some(account) => {
            let right = verified(site, login.password, account.password_hash.clone()).await?;
            right && account.username == login.username.trim()
        }
        None => {
            hashed(site, login.password).await?;
            false
        }
    };
    let mut throttle = site.throttle.lock().map_err(|_| ApiError::internal())?;
    if !ok {
        throttle.fail(Instant::now());
        drop(throttle);
        ctx.store
            .audit("login", "ログイン", "ログイン画面", ip.get(), false, None);
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "login_failed",
            LOGIN_FAILED,
        ));
    }
    throttle.succeed();
    drop(throttle);
    let (token, session) = ctx
        .store
        .create_admin_session(true, ip.get(), user_agent(&headers))
        .map_err(storage_error)?;
    ctx.store
        .audit("login", "ログイン", "ログイン画面", ip.get(), true, None);
    Ok(signed_in(
        &token,
        serde_json::json!({ "sessionId": session.id }),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TicketRequest {
    ticket: String,
}

/// 券の種類を見る。作成の画面を出すかを決める。使わない。
async fn web_ticket_info(
    State(ctx): State<Ctx>,
    ApiJson(request): ApiJson<TicketRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    web(&ctx)?;
    let kind = ctx
        .store
        .peek_admin_ticket(&request.ticket)
        .map_err(storage_error)?
        .ok_or_else(ticket_rejected)?;
    let account = ctx.store.admin_account().map_err(storage_error)?;
    Ok(Json(serde_json::json!({
        "kind": kind,
        "username": account.map(|account| account.username),
    })))
}

fn ticket_rejected() -> ApiError {
    ApiError(
        StatusCode::UNAUTHORIZED,
        "ticket_rejected",
        "このリンクは使えません。期限が切れたか、すでに使われています",
    )
}

/// 券でログインする。名前の `Host` は、ここで覚える。
async fn web_ticket(
    State(ctx): State<Ctx>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<TicketRequest>,
) -> Result<Response, ApiError> {
    web(&ctx)?;
    let peeked = ctx
        .store
        .peek_admin_ticket(&request.ticket)
        .map_err(storage_error)?;
    if peeked == Some(TicketKind::Setup) {
        return Err(ApiError::invalid("先にパスワードを決めてください"));
    }
    let kind = ctx
        .store
        .consume_admin_ticket(&request.ticket)
        .map_err(storage_error)?;
    if kind != Some(TicketKind::Login) {
        ctx.store.audit(
            "login",
            "リンクでログイン",
            "ログイン画面",
            ip.get(),
            false,
            None,
        );
        return Err(ticket_rejected());
    }
    learn_host(&ctx, &headers);
    let (token, session) = ctx
        .store
        .create_admin_session(false, ip.get(), user_agent(&headers))
        .map_err(storage_error)?;
    ctx.store.audit(
        "login",
        "リンクでログイン",
        "ログイン画面",
        ip.get(),
        true,
        None,
    );
    Ok(signed_in(
        &token,
        serde_json::json!({ "sessionId": session.id }),
    ))
}

fn learn_host(ctx: &ServerContext, headers: &HeaderMap) {
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(host_name);
    if let Some(name) = host {
        if name != "localhost" && name.parse::<IpAddr>().is_err() {
            if let Err(error) = ctx.store.allow_admin_host(name) {
                log::warn!("管理画面の名前を覚えられません: {error}");
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SetupRequest {
    ticket: String,
    username: Option<String>,
    password: String,
}

/// 準備の券で、最初の管理者を作る。アカウントがあれば、パスワードの再設定になる。
async fn web_setup(
    State(ctx): State<Ctx>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<SetupRequest>,
) -> Result<Response, ApiError> {
    let site = web(&ctx)?;
    if ctx
        .store
        .peek_admin_ticket(&request.ticket)
        .map_err(storage_error)?
        != Some(TicketKind::Setup)
    {
        ctx.store.audit(
            "login",
            "管理者の作成",
            "ログイン画面",
            ip.get(),
            false,
            None,
        );
        return Err(ticket_rejected());
    }
    check_password(&request.password).map_err(ApiError::invalid)?;
    let existing = ctx.store.admin_account().map_err(storage_error)?;
    let username = match &existing {
        Some(account) => account.username.clone(),
        None => {
            let name = request.username.as_deref().unwrap_or("").trim();
            if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
                return Err(ApiError::invalid("名前は 1〜64 文字にしてください"));
            }
            name.to_string()
        }
    };
    let hash = hashed(site, request.password).await?;
    // 計算の間に同じ券が使われていないことを、ここで確かめる。
    if ctx
        .store
        .consume_admin_ticket(&request.ticket)
        .map_err(storage_error)?
        != Some(TicketKind::Setup)
    {
        return Err(ticket_rejected());
    }
    ctx.store
        .put_admin_account(&username, &hash)
        .map_err(storage_error)?;
    ctx.store
        .delete_admin_sessions_except(None)
        .map_err(storage_error)?;
    learn_host(&ctx, &headers);
    if let Ok(mut throttle) = site.throttle.lock() {
        throttle.succeed();
    }
    let (token, session) = ctx
        .store
        .create_admin_session(true, ip.get(), user_agent(&headers))
        .map_err(storage_error)?;
    let action = if existing.is_some() {
        "パスワードの再設定"
    } else {
        "管理者の作成"
    };
    ctx.store
        .audit("login", action, "ログイン画面", ip.get(), true, None);
    Ok(signed_in(
        &token,
        serde_json::json!({ "sessionId": session.id }),
    ))
}

async fn web_logout(
    State(ctx): State<Ctx>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    web(&ctx)?;
    if let Some(session) = session_from(&ctx, &headers) {
        ctx.store
            .delete_admin_session(&session.id)
            .map_err(storage_error)?;
        ctx.store.audit(
            "login",
            "ログアウト",
            "ログイン中のブラウザ",
            ip.get(),
            true,
            None,
        );
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    let headers = response.headers_mut();
    headers.insert(header::SET_COOKIE, HeaderValue::from_static(CLEARED_COOKIE));
    headers.insert("clear-site-data", HeaderValue::from_static("\"cookies\""));
    Ok(response)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReauthRequest {
    password: String,
}

async fn web_reauth(
    State(ctx): State<Ctx>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<ReauthRequest>,
) -> Result<StatusCode, ApiError> {
    let site = web(&ctx)?;
    let session = session_from(&ctx, &headers).ok_or_else(ApiError::unauthorized)?;
    let wait = site
        .throttle
        .lock()
        .map_err(|_| ApiError::internal())?
        .remaining(Instant::now());
    if wait.is_some() {
        return Err(ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_attempts",
            LOGIN_FAILED,
        ));
    }
    let account = ctx
        .store
        .admin_account()
        .map_err(storage_error)?
        .ok_or_else(ApiError::unauthorized)?;
    if !verified(site, request.password, account.password_hash).await? {
        if let Ok(mut throttle) = site.throttle.lock() {
            throttle.fail(Instant::now());
        }
        ctx.store.audit(
            "login",
            "パスワードの再確認",
            "ログイン中のブラウザ",
            ip.get(),
            false,
            None,
        );
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "login_failed",
            "パスワードが違います",
        ));
    }
    if let Ok(mut throttle) = site.throttle.lock() {
        throttle.succeed();
    }
    ctx.store
        .mark_admin_recent_auth(&session.id)
        .map_err(storage_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// 管理画面を止める。戻すのはサーバーのコマンドだけである。
async fn web_disable(
    State(ctx): State<Ctx>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    web(&ctx)?;
    require_recent(&ctx, &headers)?;
    ctx.store.audit(
        "login",
        "管理画面の停止",
        actor(&headers),
        ip.get(),
        true,
        None,
    );
    ctx.store
        .delete_admin_sessions_except(None)
        .map_err(storage_error)?;
    ctx.store
        .set_admin_web_disabled(true)
        .map_err(storage_error)?;
    Ok(StatusCode::NO_CONTENT)
}

/// デスクトップ版の「管理画面を開く」。鍵でしか発行しない。
async fn issue_ticket(
    State(ctx): State<Ctx>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !headers.contains_key(header::AUTHORIZATION) {
        return Err(ApiError::forbidden());
    }
    require_admin(&ctx, &headers)?;
    web(&ctx).map_err(|_| {
        ApiError(
            StatusCode::CONFLICT,
            "admin_web_unavailable",
            "このサーバーでは管理画面を使えません",
        )
    })?;
    // デスクトップは、いま話している接続先の URL でリンクを開く。その名前を先に覚えておかないと、
    // ページの読み込みが断られ、券を使う所まで進めない。
    learn_host(&ctx, &headers);
    let account = ctx.store.admin_account().map_err(storage_error)?;
    let (kind, lifetime) = if account.is_some() {
        (TicketKind::Login, LOGIN_TICKET)
    } else {
        (TicketKind::Setup, SETUP_TICKET)
    };
    let ticket = ctx
        .store
        .issue_admin_ticket(kind, lifetime)
        .map_err(storage_error)?;
    ctx.store.audit(
        "login",
        "ログイン用リンクの発行",
        actor(&headers),
        ip.get(),
        true,
        None,
    );
    Ok(Json(serde_json::json!({
        "path": format!("/admin/#ticket={ticket}"),
        "kind": kind,
        "expiresInSeconds": lifetime.as_secs(),
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PasswordRequest {
    password: String,
}

async fn change_password(
    State(ctx): State<Ctx>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<PasswordRequest>,
) -> Result<StatusCode, ApiError> {
    let site = web(&ctx)?;
    require_recent(&ctx, &headers)?;
    check_password(&request.password).map_err(ApiError::invalid)?;
    let account = ctx
        .store
        .admin_account()
        .map_err(storage_error)?
        .ok_or_else(ApiError::not_found)?;
    let hash = hashed(site, request.password).await?;
    ctx.store
        .put_admin_account(&account.username, &hash)
        .map_err(storage_error)?;
    let keep = session_from(&ctx, &headers).map(|session| session.id);
    ctx.store
        .delete_admin_sessions_except(keep.as_deref())
        .map_err(storage_error)?;
    ctx.store.audit(
        "login",
        "パスワードの変更",
        actor(&headers),
        ip.get(),
        true,
        None,
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn list_sessions(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&ctx, &headers)?;
    let sessions = ctx.store.list_admin_sessions().map_err(storage_error)?;
    let current = session_from(&ctx, &headers).map(|session| session.id);
    Ok(Json(serde_json::json!({
        "sessions": sessions,
        "currentId": current,
    })))
}

async fn end_session(
    State(ctx): State<Ctx>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Result<StatusCode, ApiError> {
    require_admin(&ctx, &headers)?;
    if !ctx.store.delete_admin_session(&id).map_err(storage_error)? {
        return Err(ApiError::not_found());
    }
    ctx.store.audit(
        "login",
        "ログインの切断",
        actor(&headers),
        ip.get(),
        true,
        None,
    );
    Ok(StatusCode::NO_CONTENT)
}

fn controller(ctx: &ServerContext) -> Result<&Arc<UpdateController>, ApiError> {
    ctx.update.get().ok_or(ApiError(
        StatusCode::CONFLICT,
        "not_managed",
        "このサーバーは自分では更新できません。piep-server install で置いてください",
    ))
}

async fn get_update_settings(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&ctx, &headers)?;
    let status = controller(&ctx)?.status();
    Ok(Json(serde_json::json!({
        "auto": status.auto,
        "window": status.window,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateSettingsRequest {
    auto: bool,
    window: String,
}

async fn put_update_settings(
    State(ctx): State<Ctx>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<UpdateSettingsRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&ctx, &headers)?;
    let updates = controller(&ctx)?;
    if update::UpdateWindow::parse(&request.window).is_err() {
        return Err(ApiError::invalid(
            "時間帯は 03:00-06:00 の形で書いてください",
        ));
    }
    updates
        .set_settings(request.auto, &request.window)
        .map_err(storage_error)?;
    let detail = format!(
        "自動 {}・{}",
        if request.auto { "入" } else { "切" },
        request.window
    );
    ctx.store.audit(
        "update",
        "自動更新の設定",
        actor(&headers),
        ip.get(),
        true,
        Some(&detail),
    );
    Ok(Json(serde_json::json!({
        "auto": request.auto,
        "window": request.window,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ServerNameRequest {
    name: String,
}

async fn put_server_name(
    State(ctx): State<Ctx>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<ServerNameRequest>,
) -> Result<StatusCode, ApiError> {
    require_admin(&ctx, &headers)?;
    let name = request.name.trim();
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(ApiError::invalid("名前は 64 文字までにしてください"));
    }
    ctx.store.set_server_name(name).map_err(storage_error)?;
    ctx.store.audit(
        "system",
        "サーバーの名前の変更",
        actor(&headers),
        ip.get(),
        true,
        None,
    );
    Ok(StatusCode::NO_CONTENT)
}

/// 確かめるだけ。当てない。結果はすぐ返す。
async fn update_check(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
) -> Result<Json<update::UpdateStatus>, ApiError> {
    require_admin(&ctx, &headers)?;
    let updates = controller(&ctx)?;
    if let Err(error) = updates.check_now().await {
        log::warn!("更新を確かめられません: {error}");
        return Err(ApiError(
            StatusCode::BAD_GATEWAY,
            "update_check_failed",
            "更新を確かめられませんでした。ネットワークを確認してください",
        ));
    }
    Ok(Json(updates.status()))
}

async fn update_history(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
) -> Result<Json<Vec<update::UpdateHistoryEntry>>, ApiError> {
    require_admin(&ctx, &headers)?;
    Ok(Json(controller(&ctx)?.history()))
}

async fn update_retry(
    State(ctx): State<Ctx>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    require_admin(&ctx, &headers)?;
    controller(&ctx)?.retry_skipped();
    ctx.store.audit(
        "update",
        "飛ばしたバージョンをもう一度試す",
        actor(&headers),
        ip.get(),
        true,
        None,
    );
    Ok(StatusCode::ACCEPTED)
}

async fn update_rollback(
    State(ctx): State<Ctx>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_recent(&ctx, &headers)?;
    let version = controller(&ctx)?.roll_back().map_err(|error| {
        log::warn!("前の版へ戻せません: {error}");
        ApiError(
            StatusCode::CONFLICT,
            "rollback_unavailable",
            "今は前のバージョンへ戻せません",
        )
    })?;
    ctx.store.audit(
        "update",
        "前のバージョンへ戻す",
        actor(&headers),
        ip.get(),
        true,
        Some(&version),
    );
    Ok(Json(serde_json::json!({ "version": version })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AuditQuery {
    kind: Option<String>,
    ok: Option<bool>,
    before: Option<i64>,
}

async fn audit_list(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Vec<super::super::admin_store::AuditEntry>>, ApiError> {
    require_admin(&ctx, &headers)?;
    Ok(Json(
        ctx.store
            .list_audit(query.kind.as_deref(), query.ok, query.before, AUDIT_PAGE)
            .map_err(storage_error)?,
    ))
}

fn directory_bytes(path: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(entry.path());
            } else {
                total += metadata.len();
            }
        }
    }
    total
}

/// 容量の内訳。全部のファイルを歩くので、10 分は同じ値を返す。
async fn storage(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&ctx, &headers)?;
    let site = ctx.web.get().ok_or_else(ApiError::not_found)?;
    if let Ok(cached) = site.storage.lock() {
        if let Some((at, value)) = cached.as_ref() {
            if at.elapsed() < STORAGE_CACHE {
                return Ok(Json(value.clone()));
            }
        }
    }
    let paths = site.paths.clone();
    let value = tokio::task::spawn_blocking(move || {
        let database = std::fs::metadata(paths.database()).map_or(0, |meta| meta.len());
        let content = directory_bytes(&paths.storage());
        let library = directory_bytes(&paths.library());
        serde_json::json!({
            "contentBytes": content,
            "databaseBytes": database,
            "indexBytes": library.saturating_sub(content).saturating_sub(database),
            "trashBytes": directory_bytes(&paths.trash()),
            "freeBytes": crate::database::queries::available_space_bytes(&paths.root),
        })
    })
    .await
    .map_err(|_| ApiError::internal())?;
    if let Ok(mut cached) = site.storage.lock() {
        *cached = Some((Instant::now(), value.clone()));
    }
    Ok(Json(value))
}

async fn connection(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
) -> Result<Json<ConnectionInfo>, ApiError> {
    require_admin(&ctx, &headers)?;
    let site = ctx.web.get().ok_or_else(ApiError::not_found)?;
    Ok(Json(site.connection.clone()))
}

/// 索引の続きを作り直す。
async fn reindex(
    State(ctx): State<Ctx>,
    Extension(ip): Extension<ClientIp>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    require_admin(&ctx, &headers)?;
    let receiver = ctx.receiver.clone().ok_or_else(ApiError::not_found)?;
    tokio::task::spawn_blocking(move || receiver.start_index_catch_up())
        .await
        .map_err(|_| ApiError::internal())?;
    ctx.store.audit(
        "library",
        "索引の確認と作成",
        actor(&headers),
        ip.get(),
        true,
        None,
    );
    Ok(StatusCode::ACCEPTED)
}

/// 新しい管理者の登録文字列。今の管理者は、これが使われた時点で切れる。
pub(super) fn require_for_admin_pairing(
    ctx: &ServerContext,
    headers: &HeaderMap,
    ip: &ClientIp,
) -> Result<(), ApiError> {
    require_recent(ctx, headers)?;
    ctx.store.audit(
        "device",
        "管理者の入れ替えの発行",
        actor(headers),
        ip.get(),
        true,
        None,
    );
    Ok(())
}

pub(super) fn note_device(ctx: &ServerContext, headers: &HeaderMap, ip: &ClientIp, action: &str) {
    ctx.store
        .audit("device", action, actor(headers), ip.get(), true, None);
}

pub(super) fn note_library(ctx: &ServerContext, action: &str, detail: Option<&str>) {
    ctx.store
        .audit("library", action, "デスクトップ版", None, true, detail);
}

/// 画面のファイルを返す。知らない道は画面そのものを返す。
fn asset_response(site: &AdminWeb, path: &str) -> Response {
    let path = path.trim_start_matches('/');
    let found = site
        .assets
        .iter()
        .find(|asset| asset.path == path)
        .or_else(|| {
            // 拡張子のある道はファイルの要求なので、画面でごまかさない。
            (!path.rsplit('/').next().unwrap_or("").contains('.'))
                .then(|| site.assets.iter().find(|asset| asset.path == "index.html"))
                .flatten()
        });
    match found {
        Some(asset) => Response::builder()
            .header(header::CONTENT_TYPE, asset.mime)
            .body(Body::from(asset.bytes))
            .unwrap_or_else(|_| ApiError::internal().into_response()),
        None if site.assets.is_empty() => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "この実行ファイルには管理画面が組み込まれていません",
        )
            .into_response(),
        None => ApiError::not_found().into_response(),
    }
}

async fn page_root(State(ctx): State<Ctx>) -> Result<Response, ApiError> {
    Ok(asset_response(web(&ctx)?, "index.html"))
}

async fn page_asset(
    State(ctx): State<Ctx>,
    UrlPath(path): UrlPath<String>,
) -> Result<Response, ApiError> {
    Ok(asset_response(web(&ctx)?, &path))
}

/// `admin_router` に足す API。Cookie と鍵のどちらでも通るもの。
pub(super) fn api_routes() -> Router<Ctx> {
    Router::new()
        .route("/api/v1/admin/tickets", post(issue_ticket))
        .route("/api/v1/admin/password", put(change_password))
        .route("/api/v1/admin/sessions", get(list_sessions))
        .route("/api/v1/admin/sessions/{id}", delete(end_session))
        .route(
            "/api/v1/admin/settings/update",
            get(get_update_settings).put(put_update_settings),
        )
        .route("/api/v1/admin/settings/server-name", put(put_server_name))
        .route("/api/v1/admin/update/check", post(update_check))
        .route("/api/v1/admin/update/history", get(update_history))
        .route("/api/v1/admin/update/retry", post(update_retry))
        .route("/api/v1/admin/update/rollback", post(update_rollback))
        .route("/api/v1/admin/audit", get(audit_list))
        .route("/api/v1/admin/storage", get(storage))
        .route("/api/v1/admin/connection", get(connection))
        .route("/api/v1/admin/library/reindex", post(reindex))
}

/// 画面とログイン。管理画面を出す口にだけ足す。
pub(super) fn web_routes(ctx: Ctx) -> Router {
    let routes = Router::new()
        .route("/admin", get(|| async { Redirect::to("/admin/") }))
        .route("/admin/", get(page_root))
        .route("/admin/{*path}", get(page_asset))
        .route("/api/v1/admin/web/state", get(web_state))
        .route("/api/v1/admin/web/login", post(web_login))
        .route("/api/v1/admin/web/ticket-info", post(web_ticket_info))
        .route("/api/v1/admin/web/ticket", post(web_ticket))
        .route("/api/v1/admin/web/setup", post(web_setup))
        .route("/api/v1/admin/web/logout", post(web_logout))
        .route("/api/v1/admin/web/reauth", post(web_reauth))
        .route("/api/v1/admin/web/disable", post(web_disable))
        .layer(axum::extract::DefaultBodyLimit::max(super::MAX_JSON_BYTES))
        .with_state(Arc::clone(&ctx));
    with_guard(routes, Arc::clone(&ctx)).layer(middleware::from_fn_with_state(ctx, secure_headers))
}

/// サーバーの中で一度きりのリンクを作る。`admin-setup` と `admin-password` が使う。
pub fn command_ticket(
    store: &super::super::store::ServerStore,
    url: Option<&str>,
    kind: TicketKind,
) -> Result<String, String> {
    if let Some(url) = url {
        let host = url
            .split("://")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .ok_or("--url は https://… の形で指定してください")?;
        let name = host_name(host);
        if name != "localhost" && name.parse::<IpAddr>().is_err() {
            store.allow_admin_host(name)?;
        }
    }
    let ticket = store.issue_admin_ticket(kind, SETUP_TICKET)?;
    store.audit(
        "login",
        "ログイン用リンクの発行",
        "サーバーのコマンド",
        None,
        true,
        None,
    );
    Ok(ticket)
}

/// 管理者の鍵か Cookie か。役割はどちらも管理者である。
pub(super) fn session_as_device(
    session: &super::super::admin_store::AdminSession,
) -> super::super::store::AuthorizedDevice {
    super::super::store::AuthorizedDevice {
        id: format!("session:{}", session.id),
        role: Role::Admin,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_rules_refuse_short_common_and_patterned() {
        assert!(check_password("short").is_err());
        assert!(check_password(&"a".repeat(257)).is_err());
        assert!(check_password("PasswordPassword").is_err());
        assert!(check_password("aaaaaaaaaaaaaaaaaaaa").is_err());
        assert!(check_password("abcabcabcabcabcabc").is_err());
        assert!(check_password("1234567890123456").is_err());
        assert!(check_password("qwertyuiopqwerty").is_err());
        assert!(check_password("abcdefghijklmnopq").is_err());
        assert!(check_password("夏の終わりに海辺で読む本を探す").is_ok());
        assert!(check_password("correct horse battery").is_ok());
    }

    #[test]
    fn hashes_use_argon2id_and_verify() {
        let hash = hash_password("夏の終わりに海辺で読む本を探す").unwrap();
        assert!(hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert!(verify_password("夏の終わりに海辺で読む本を探す", &hash));
        assert!(!verify_password(
            "夏の終わりに海辺で読む本を探さない",
            &hash
        ));
        assert!(!verify_password("x", "壊れた値"));
    }

    #[test]
    fn the_wait_grows_from_one_minute_to_fifteen() {
        let mut throttle = LoginThrottle::default();
        let now = Instant::now();
        for _ in 0..4 {
            throttle.fail(now);
            assert!(throttle.remaining(now).is_none());
        }
        let minutes = |throttle: &LoginThrottle| throttle.remaining(now).unwrap().as_secs() / 60;
        for expected in [1, 2, 4, 8, 15, 15, 15, 15, 15, 15] {
            throttle.fail(now);
            let seen = minutes(&throttle);
            assert!(
                seen == expected || seen + 1 == expected,
                "{seen} {expected}"
            );
        }
        throttle.succeed();
        assert!(throttle.remaining(now).is_none());
    }

    #[test]
    fn host_names_drop_the_port() {
        assert_eq!(host_name("127.0.0.1:38778"), "127.0.0.1");
        assert_eq!(host_name("[::1]:38778"), "::1");
        assert_eq!(host_name("piep.example.net"), "piep.example.net");
    }

    #[test]
    fn a_missing_or_foreign_origin_is_not_same_origin() {
        let host = "piep.example.net";
        let headers = |pairs: &[(&'static str, &'static str)]| {
            let mut map = HeaderMap::new();
            for (name, value) in pairs {
                map.insert(*name, HeaderValue::from_static(value));
            }
            map
        };
        assert!(same_origin(
            &headers(&[("sec-fetch-site", "same-origin")]),
            host
        ));
        assert!(!same_origin(
            &headers(&[("sec-fetch-site", "cross-site")]),
            host
        ));
        assert!(!same_origin(
            &headers(&[("sec-fetch-site", "same-site")]),
            host
        ));
        assert!(!same_origin(&headers(&[]), host));
        assert!(!same_origin(&headers(&[("origin", "null")]), host));
        assert!(!same_origin(
            &headers(&[("origin", "https://evil.example")]),
            host
        ));
        assert!(same_origin(
            &headers(&[("origin", "https://piep.example.net")]),
            host
        ));
    }
}
