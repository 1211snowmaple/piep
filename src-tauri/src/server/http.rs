//! サーバーの HTTP 層。登録と接続確認（`/api/hello`・`pair`・`about`）と管理の API
//! （`/api/admin/...`）。棚を読む口（`/api/identity`・`/api/libraries/...`）は `reader`。
//!
//! URL が知られていても読めないことを前提にする。無認証で答えるのは `hello` と
//! `pair` だけで、応答にパスや内部の誤り文を出さない。

pub mod admin_web;
mod shelf_admin;

pub use admin_web::{AdminAsset, AdminWeb, ConnectionInfo};

use super::store::{AuthorizedDevice, PairError, Role, ServerStore};
use super::update::{UpdateController, UpdateStatus};
use super::{pairing_payload, API_VERSION};
use admin_web::AdminHeaders;
use axum::extract::rejection::JsonRejection;
use axum::extract::{
    DefaultBodyLimit, FromRequest, MatchedPath, Path as UrlPath, Query, Request, State,
};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tower_http::compression::CompressionLayer;
use tower_http::CompressionLevel;

const MAX_JSON_BYTES: usize = 64 * 1024;
/// 管理の口の本文の上限。棚の写しの問い合わせ（持っていない部分の一覧）が収まる大きさにする。
const MAX_ADMIN_BYTES: usize = 8 * 1024 * 1024 + 64 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const ADMIN_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const READER_PAIRING_LIFETIME: Duration = Duration::from_secs(10 * 60);
const PAIR_FAILURE_LIMIT: usize = 10;
const PAIR_FAILURE_WINDOW: Duration = Duration::from_secs(60);

/// `hello` と `about` で知らせる機能の一覧。棚を読む口の機能は `/api/identity` が知らせる。
const CAPABILITIES: &[&str] = &["read"];

pub struct ServerContext {
    pub store: Arc<ServerStore>,
    /// 単独サーバーの置き場の根。
    pub root: PathBuf,
    /// 携帯の登録文字列に載せる URL。空なら頼んだ管理者の URL を使う。
    pub public_url: Option<String>,
    /// 自己署名で待ち受けているときの証明書の指紋。
    pub fingerprint: Option<String>,
    reader_shelf: OnceLock<Arc<super::reader::Context>>,
    /// 自己更新の係。`install` で置いた単独サーバーにだけある。
    update: OnceLock<Arc<UpdateController>>,
    /// 管理画面。管理者の口を持つ単独サーバーにだけある。
    web: OnceLock<AdminWeb>,
    started_at: String,
    pair_failures: Mutex<VecDeque<Instant>>,
    /// 棚の写しの受け取り。単独サーバーにだけある。
    shipments: OnceLock<Arc<super::standalone::shipment::ShelfShipments>>,
    /// 単独サーバーの置き場（設定を書くため）。
    paths: OnceLock<super::standalone::ServerPaths>,
}

impl ServerContext {
    pub fn new(
        store: Arc<ServerStore>,
        root: PathBuf,
        public_url: Option<String>,
        fingerprint: Option<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            root,
            public_url,
            fingerprint,
            reader_shelf: OnceLock::new(),
            update: OnceLock::new(),
            web: OnceLock::new(),
            started_at: chrono::Utc::now().to_rfc3339(),
            pair_failures: Mutex::new(VecDeque::new()),
            shipments: OnceLock::new(),
            paths: OnceLock::new(),
        })
    }

    /// 単独サーバーの置き場と、棚の写しの受け取りを置く。
    pub(super) fn set_shipments(
        &self,
        paths: super::standalone::ServerPaths,
        shipments: Arc<super::standalone::shipment::ShelfShipments>,
    ) {
        let _ = self.paths.set(paths);
        let _ = self.shipments.set(shipments);
    }

    pub fn set_update(&self, controller: Arc<UpdateController>) {
        let _ = self.update.set(controller);
    }

    /// 配信する棚を決め、本文の検索の索引を棚に合わせ始める。索引を作るあいだは棚を
    /// 開いたままにするので、開いただけの棚（確かめて閉じるもの）では始めない。
    pub(super) fn set_reader_shelf(
        &self,
        shelf: Arc<super::reader::Context>,
    ) -> Result<(), String> {
        self.reader_shelf
            .set(Arc::clone(&shelf))
            .map_err(|_| "棚は既に設定されています".to_owned())?;
        shelf.prepare_search();
        Ok(())
    }

    /// 配っている棚。
    pub(super) fn reader_shelf(&self) -> Option<&Arc<super::reader::Context>> {
        self.reader_shelf.get()
    }

    pub fn set_admin_web(&self, web: AdminWeb) {
        let _ = self.web.set(web);
    }
}

type Ctx = Arc<ServerContext>;
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    retry_after: Option<u64>,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
            retry_after: None,
        }
    }

    fn retry_after(mut self, wait: Duration) -> Self {
        let seconds = wait.as_secs() + u64::from(wait.subsec_nanos() != 0);
        self.retry_after = Some(seconds.max(1));
        self
    }

    fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "端末が登録されていません",
        )
    }
    fn forbidden() -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", "この端末では行えません")
    }
    fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "処理できませんでした",
        )
    }
    fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "見つかりません")
    }
    fn invalid(message: &'static str) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = serde_json::json!({ "code": self.code, "message": self.message });
        if let Some(seconds) = self.retry_after {
            body["retryAfterSeconds"] = seconds.into();
            return (
                self.status,
                [(header::RETRY_AFTER, seconds.to_string())],
                Json(body),
            )
                .into_response();
        }
        (self.status, Json(body)).into_response()
    }
}

/// 読み取れない JSON にも `{code, message}` で答える。axum の既定の誤り文は使わない。
struct ApiJson<T>(T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    Json<T>: FromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        match Json::<T>::from_request(request, state).await {
            Ok(Json(value)) => Ok(Self(value)),
            Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => {
                Err(ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "too_large",
                    "要求が大きすぎます",
                ))
            }
            Err(_) => Err(ApiError::invalid("要求を読み取れません")),
        }
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let (scheme, credentials) = headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .split_once(' ')?;
    let token = credentials.trim_start_matches(' ');
    let unpadded = token.trim_end_matches('=');
    (scheme.eq_ignore_ascii_case("Bearer")
        && !unpadded.is_empty()
        && unpadded
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._~+/".contains(&byte)))
    .then_some(token)
}

fn require_device(ctx: &ServerContext, headers: &HeaderMap) -> Result<AuthorizedDevice, ApiError> {
    let device = bearer(headers)
        .and_then(|token| ctx.store.authorize(token))
        .ok_or_else(ApiError::unauthorized)?;
    Ok(device)
}

fn require_admin(
    ctx: &ServerContext,
    headers: &AdminHeaders,
) -> Result<AuthorizedDevice, ApiError> {
    let Some(token) = bearer(headers) else {
        return admin_web::session_from(ctx, headers)
            .map(|session| admin_web::session_as_device(&session))
            .ok_or_else(ApiError::unauthorized);
    };
    let device = ctx
        .store
        .authorize(token)
        .ok_or_else(ApiError::unauthorized)?;
    if device.role == Role::Admin {
        Ok(device)
    } else {
        Err(ApiError::forbidden())
    }
}

fn device_routes(ctx: Ctx) -> Router {
    Router::new()
        .route("/api/about", get(about))
        .route_layer(middleware::from_fn_with_state(
            Arc::clone(&ctx),
            authorize_device_before_body,
        ))
        // 登録と接続確認だけは公開する。
        .route("/api/hello", get(hello))
        .route("/api/pair", post(pair))
        .layer(DefaultBodyLimit::max(MAX_JSON_BYTES))
        .layer(middleware::from_fn(read_timeout))
        .with_state(ctx)
}

fn admin_routes(ctx: Ctx) -> Router {
    let routes = Router::new()
        .merge(admin_web::api_routes())
        .route("/api/admin/status", get(admin_status))
        .route("/api/admin/update", post(admin_update))
        .route("/api/admin/pairings", post(admin_pairing))
        .route("/api/admin/devices", get(admin_devices))
        .route("/api/admin/devices/{id}", delete(admin_revoke))
        .route("/api/admin/shelf/reading", get(admin_shelf_reading))
        .route("/api/admin/shelf/favorites", get(admin_shelf_favorites))
        .route(
            "/api/admin/shelf/shipments",
            get(shelf_admin::status).post(shelf_admin::open),
        )
        .route(
            "/api/admin/shelf/shipments/{id}",
            get(shelf_admin::result).delete(shelf_admin::abort),
        )
        .route(
            "/api/admin/shelf/shipments/{id}/needed",
            post(shelf_admin::needed),
        )
        .route(
            "/api/admin/shelf/shipments/{id}/chunks/{chunk}",
            put(shelf_admin::put_chunk).layer(DefaultBodyLimit::max(
                piep_storage::MAX_ENCODED_CHUNK_BYTES as usize + 1024,
            )),
        )
        .route(
            "/api/admin/shelf/shipments/{id}/commit",
            post(shelf_admin::commit),
        )
        .route(
            "/api/admin/devices/{id}/shelf",
            put(shelf_admin::set_reader),
        )
        .layer(DefaultBodyLimit::max(MAX_ADMIN_BYTES))
        .layer(middleware::from_fn(admin_timeout))
        // 認証のない要求のために、本文を受信しない。
        // Cookie の Host/Origin 検査は、この外側の with_guard が先に行う。
        .layer(middleware::from_fn_with_state(
            Arc::clone(&ctx),
            authorize_admin_before_body,
        ))
        .with_state(Arc::clone(&ctx));
    admin_web::with_guard(routes, ctx)
}

async fn authorize_admin_before_body(
    State(ctx): State<Ctx>,
    request: Request,
    next: Next,
) -> Response {
    authorize_before_body(ctx, request, next, true).await
}

async fn authorize_device_before_body(
    State(ctx): State<Ctx>,
    request: Request,
    next: Next,
) -> Response {
    authorize_before_body(ctx, request, next, false).await
}

async fn authorize_before_body(ctx: Ctx, request: Request, next: Next, admin: bool) -> Response {
    let headers = AdminHeaders::new(request.headers(), request.extensions());
    // 本文を待たずに拒否し、状態DBを待つ間もHTTP実行スレッドを止めない。
    let authorized = tokio::task::spawn_blocking(move || {
        if admin {
            require_admin(&ctx, &headers)
        } else {
            require_device(&ctx, &headers)
        }
    })
    .await
    .unwrap_or_else(|_| Err(ApiError::internal()));
    if let Err(error) = authorized {
        return error.into_response();
    }
    next.run(request).await
}

/// 読み手の口。管理の API は載せない。
pub fn reader_router(ctx: Ctx) -> Router {
    finish(device_routes(Arc::clone(&ctx)), &ctx)
}

/// 読み手と管理の両方を受ける口。管理の口を分けるときは、そちらにもこれを使う。
/// 管理者の登録と証明書の確認に `pair` と `hello` が要るからである。
pub fn combined_router(ctx: Ctx) -> Router {
    finish(
        device_routes(Arc::clone(&ctx)).merge(admin_routes(Arc::clone(&ctx))),
        &ctx,
    )
}

/// 管理者の口に、管理画面も載せる。`AdminWeb` を渡した単独サーバーだけが使う。
pub fn admin_router(ctx: Ctx) -> Router {
    finish(
        device_routes(Arc::clone(&ctx))
            .merge(admin_routes(Arc::clone(&ctx)))
            .merge(admin_web::web_routes(Arc::clone(&ctx))),
        &ctx,
    )
    .layer(Extension(admin_web::PageHere))
}

fn finish(router: Router, ctx: &Ctx) -> Router {
    let routes = router
        .fallback(|| async { ApiError::not_found() })
        // 本文と一覧の JSON は大きく縮む。既定の判定は画像を素通りさせる。
        // 頼まれたときだけ圧縮するので、古い端末の受け取り方は変わらない。
        .layer(CompressionLayer::new().quality(CompressionLevel::Fastest))
        .layer(middleware::from_fn(no_store));
    // 棚の資産は形式とキャッシュの条件を自分で決める。配信時に圧縮し直さない。
    let routes = match ctx.reader_shelf.get() {
        Some(shelf) => routes.merge(super::reader::router(Arc::clone(shelf))),
        None => routes,
    };
    routes.layer(middleware::from_fn(observe))
}

async fn no_store(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    response
}

/// 要求の記録を残す。
///
/// 記録に載せるのは端点の型と状態コードと所要時間だけで、作品 ID も問い合わせも載せない。
/// 検索語や登録の鍵が残らないようにするためである。成功は読書のたびに起きるので debug に落とす。
async fn observe(request: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = request.method().clone();
    let endpoint = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("(経路なし)".to_string(), |path| {
            path.as_str().to_string()
        });
    let response = next.run(request).await;
    let status = response.status();
    let elapsed = started.elapsed().as_millis();
    if status.is_success() {
        log::debug!("{method} {endpoint} {} {elapsed}ms", status.as_u16());
    } else {
        log::info!("{method} {endpoint} {} {elapsed}ms", status.as_u16());
    }
    response
}

async fn read_timeout(request: Request, next: Next) -> Response {
    time_limited(READ_TIMEOUT, request, next).await
}

async fn admin_timeout(request: Request, next: Next) -> Response {
    time_limited(ADMIN_TIMEOUT, request, next).await
}

async fn time_limited(limit: Duration, request: Request, next: Next) -> Response {
    match tokio::time::timeout(limit, next.run(request)).await {
        Ok(response) => response,
        Err(_) => ApiError::new(
            StatusCode::REQUEST_TIMEOUT,
            "timeout",
            "時間内に終わりませんでした",
        )
        .into_response(),
    }
}

async fn hello() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "apiVersion": API_VERSION,
        "serverName": "piep",
        "capabilities": CAPABILITIES,
        "beta": true,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PairRequest {
    secret: String,
    device_name: String,
}

async fn pair(
    State(ctx): State<Ctx>,
    ApiJson(request): ApiJson<PairRequest>,
) -> Result<Json<piep_library_protocol::PairReply>, ApiError> {
    if request.secret.len() > 128 || request.device_name.chars().count() > 64 {
        return Err(ApiError::invalid("登録情報が長すぎます"));
    }
    let result = {
        // 上限の検査から失敗の記録までをまとめ、同時に届いた要求にも同じ上限を適用する。
        let mut failures = ctx.pair_failures.lock().await;
        let now = Instant::now();
        while failures
            .front()
            .is_some_and(|at| now.duration_since(*at) >= PAIR_FAILURE_WINDOW)
        {
            failures.pop_front();
        }
        if failures.len() >= PAIR_FAILURE_LIMIT {
            let wait = PAIR_FAILURE_WINDOW - now.duration_since(failures[0]);
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_attempts",
                "登録の試行が多すぎます。しばらく待ってからもう一度試してください",
            )
            .retry_after(wait));
        }
        let result = ctx
            .store
            .consume_pairing(&request.secret, &request.device_name);
        if matches!(result, Err(PairError::Rejected | PairError::TooManyDevices)) {
            failures.push_back(Instant::now());
        }
        result
    };
    match result {
        Ok(paired) => {
            ctx.store.audit(
                "device",
                "端末の登録",
                "登録文字列",
                None,
                true,
                Some(paired.role.as_str()),
            );
            Ok(Json(piep_library_protocol::PairReply {
                device_id: paired.device_id,
                token: paired.token,
                role: paired.role,
                api_version: API_VERSION,
            }))
        }
        // 理由を区別しない。鍵が合っているかどうかを外から測らせない。
        Err(PairError::Rejected | PairError::TooManyDevices) => Err(ApiError::unauthorized()),
        Err(PairError::Storage(detail)) => {
            log::error!("端末を登録できません: {detail}");
            Err(ApiError::internal())
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShelfReadingQuery {
    cursor: Option<String>,
    limit: Option<i64>,
}

/// 携帯が棚に書いた読書位置を、デスクトップ版が自分の棚へ取り込むために読む。
/// 棚の持ち主の位置だけ。棚を配っているサーバーにだけある。
async fn admin_shelf_reading(
    State(ctx): State<Ctx>,
    headers: AdminHeaders,
    Query(query): Query<ShelfReadingQuery>,
) -> Result<Response, ApiError> {
    require_admin(&ctx, &headers)?;
    let shelf = ctx
        .reader_shelf
        .get()
        .cloned()
        .ok_or_else(ApiError::not_found)?;
    Ok(shelf.owner_reading(query.cursor, query.limit).await)
}

/// 棚の持ち主のお気に入り（携帯が付け外ししたものをデスクトップ版が取り込む）。
async fn admin_shelf_favorites(
    State(ctx): State<Ctx>,
    headers: AdminHeaders,
    Query(query): Query<ShelfReadingQuery>,
) -> Result<Response, ApiError> {
    require_admin(&ctx, &headers)?;
    let shelf = ctx
        .reader_shelf
        .get()
        .cloned()
        .ok_or_else(ApiError::not_found)?;
    Ok(shelf.owner_favorites(query.cursor, query.limit).await)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminStatus {
    version: &'static str,
    api_version: u32,
    /// 配っている棚。まだ受け取っていない単独サーバーでは無い。
    shelf: Option<super::reader::ShelfStatus>,
    devices: Vec<serde_json::Value>,
    public_url: Option<String>,
    update: Option<UpdateStatus>,
    server_name: String,
    started_at: String,
    /// `available`、`public-only`（公開の口しかない）、`disabled`（止めてある）、`none`。
    admin_web: &'static str,
    /// 送られた棚を組み終えて、次の起動で配るのを待っているか（単独サーバーだけ）。
    #[serde(skip_serializing_if = "Option::is_none")]
    shelf_pending: Option<bool>,
    /// 棚の受け取りの進み具合（単独サーバーだけ）。
    #[serde(skip_serializing_if = "Option::is_none")]
    shipment: Option<super::standalone::shipment::ShipmentStatus>,
}

async fn admin_status(
    State(ctx): State<Ctx>,
    headers: AdminHeaders,
) -> Result<Json<AdminStatus>, ApiError> {
    let status_ctx = Arc::clone(&ctx);
    let (devices, server_name, admin_web) = tokio::task::spawn_blocking(move || {
        require_admin(&status_ctx, &headers)?;
        let devices = device_rows(&status_ctx)?;
        Ok::<_, ApiError>((
            devices,
            status_ctx.store.server_name(),
            admin_web_state(&status_ctx),
        ))
    })
    .await
    .map_err(|_| ApiError::internal())??;
    let shelf = if let Some(shelf) = ctx.reader_shelf.get().cloned() {
        Some(
            tokio::task::spawn_blocking(move || shelf.shelf_status())
                .await
                .map_err(|_| ApiError::internal())?
                .map_err(|error| {
                    log::error!("棚の配信状態を確認できません: {error}");
                    ApiError::internal()
                })?,
        )
    } else {
        None
    };
    Ok(Json(AdminStatus {
        version: super::update::VERSION,
        api_version: API_VERSION,
        shelf,
        devices,
        public_url: ctx.public_url.clone(),
        update: ctx.update.get().map(|controller| controller.status()),
        server_name,
        started_at: ctx.started_at.clone(),
        admin_web,
        shelf_pending: ctx.shipments.get().map(|shipments| shipments.pending()),
        shipment: ctx.shipments.get().map(|shipments| shipments.status()),
    }))
}

fn admin_web_state(ctx: &ServerContext) -> &'static str {
    if ctx.web.get().is_none() {
        if ctx.public_url.is_some() {
            "public-only"
        } else {
            "none"
        }
    } else if ctx.store.admin_web_disabled() {
        "disabled"
    } else {
        "available"
    }
}

/// 登録した端末に、版と機能を知らせる。無認証の `hello` には版を出さない。
async fn about(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_device(&ctx, &headers)?;
    let mut capabilities = CAPABILITIES.to_vec();
    if ctx.update.get().is_some() {
        capabilities.push("update-settings");
    }
    if admin_web_state(&ctx) == "available" {
        capabilities.push("admin-web");
    }
    Ok(Json(serde_json::json!({
        "version": super::update::VERSION,
        "apiVersion": API_VERSION,
        "serverName": ctx.store.server_name(),
        "capabilities": capabilities,
    })))
}

/// 新しい版を確かめて当てるよう頼む。頼めるのはこれだけで、ファイルや URL は渡せない。
async fn admin_update(
    State(ctx): State<Ctx>,
    headers: AdminHeaders,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&ctx, &headers)?;
    let controller = ctx.update.get().ok_or(ApiError::new(
        StatusCode::CONFLICT,
        "not_managed",
        "このサーバーは自分では更新できません。piep-server install で置いてください",
    ))?;
    controller.request_now();
    Ok(Json(serde_json::json!({ "accepted": true })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PairingRequest {
    /// 管理者が自分を登録した URL。公開 URL の設定が無いときに使う。
    url: Option<String>,
    /// `admin` なら管理者の入れ替え。もう一度確かめる。
    role: Option<String>,
}

async fn admin_pairing(
    State(ctx): State<Ctx>,
    Extension(ip): Extension<admin_web::ClientIp>,
    headers: AdminHeaders,
    ApiJson(request): ApiJson<PairingRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&ctx, &headers)?;
    if request.role.as_deref() == Some("admin") {
        admin_web::require_for_admin_pairing(&ctx, &headers, &ip)?;
        let url = pairing_url(&ctx, request.url, "http")
            .ok_or_else(|| ApiError::invalid("このサーバーへ届く URL が決まっていません"))??;
        let lifetime = Duration::from_secs(30 * 60);
        let secret = ctx
            .store
            .issue_pairing(Role::Admin, lifetime)
            .map_err(|_| ApiError::internal())?;
        return Ok(Json(serde_json::json!({
            "payload": pairing_payload(Role::Admin, &url, ctx.fingerprint.as_deref(), &secret),
            "expiresInSeconds": lifetime.as_secs(),
        })));
    }
    admin_web::note_device(&ctx, &headers, &ip, "携帯の登録文字列の発行");
    let url = pairing_url(&ctx, request.url, "https://")
        .ok_or_else(|| ApiError::invalid("携帯が接続する https の URL が決まっていません。server.toml の public_url に書いてください"))??;
    let secret = ctx
        .store
        .issue_pairing(Role::Reader, READER_PAIRING_LIFETIME)
        .map_err(|_| ApiError::internal())?;
    Ok(Json(serde_json::json!({
        "payload": pairing_payload(Role::Reader, &url, ctx.fingerprint.as_deref(), &secret),
        "expiresInSeconds": READER_PAIRING_LIFETIME.as_secs(),
    })))
}

/// 登録文字列に載せる URL。公開の URL があればそれを、無ければ画面から渡された URL を使う。
/// 登録する端末は別の機械なので、この機械の中だけのアドレスは載せない。
fn pairing_url(
    ctx: &ServerContext,
    page: Option<String>,
    scheme: &str,
) -> Option<Result<String, ApiError>> {
    let url = ctx
        .public_url
        .clone()
        .or(page)
        .filter(|url| url.len() <= 512 && url.starts_with(scheme))?;
    if only_this_machine(&url) {
        return Some(Err(ApiError::invalid(
            "このアドレスはこの PC でしか使えません。登録する端末から届く URL を使ってください",
        )));
    }
    Some(Ok(url))
}

/// この機械の中からしか届かないアドレスか（ループバックと localhost）。
fn only_this_machine(url: &str) -> bool {
    match url::Url::parse(url)
        .ok()
        .and_then(|url| url.host().map(|host| host.to_owned()))
    {
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => {
            address.is_loopback()
                || address
                    .to_ipv4_mapped()
                    .is_some_and(|address| address.is_loopback())
        }
        None => true,
    }
}

async fn admin_devices(
    State(ctx): State<Ctx>,
    headers: AdminHeaders,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    tokio::task::spawn_blocking(move || {
        require_admin(&ctx, &headers)?;
        device_rows(&ctx).map(Json)
    })
    .await
    .map_err(|_| ApiError::internal())?
}

/// 端末の一覧。棚を配っているサーバーだけ、携帯ごとに棚を読めるかを添える。
fn device_rows(ctx: &ServerContext) -> Result<Vec<serde_json::Value>, ApiError> {
    let devices = ctx.store.list_devices().map_err(|_| ApiError::internal())?;
    let shelf = ctx.reader_shelf.get().cloned();
    let mut rows = Vec::with_capacity(devices.len());
    for device in devices {
        let reads = shelf.as_ref().map(|shelf| shelf.reads(&device.id));
        let mut row = serde_json::to_value(&device).map_err(|_| ApiError::internal())?;
        // 棚を配っているサーバーだけ、携帯ごとに棚を読めるかを添える。
        if let (Some(reads), Some(object)) = (reads, row.as_object_mut()) {
            object.insert("readsShelf".into(), serde_json::Value::Bool(reads));
        }
        rows.push(row);
    }
    Ok(rows)
}

async fn admin_revoke(
    State(ctx): State<Ctx>,
    Extension(ip): Extension<admin_web::ClientIp>,
    headers: AdminHeaders,
    UrlPath(id): UrlPath<String>,
) -> Result<StatusCode, ApiError> {
    require_admin(&ctx, &headers)?;
    if ctx
        .store
        .revoke_device(&id)
        .map_err(|_| ApiError::internal())?
    {
        admin_web::note_device(&ctx, &headers, &ip, "端末の解除");
        shelf_admin::forget_reader(&ctx, id).await;
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}
