//! サーバーの HTTP 層。読み手の API（`/api/v1/...`）と管理の API（`/api/v1/admin/...`）。
//!
//! URL が知られていても読めないことを前提にする。無認証で答えるのは `hello` と
//! `pair` だけで、応答にパスや内部の誤り文を出さない。

pub mod admin_web;

pub use admin_web::{AdminAsset, AdminWeb, ConnectionInfo};

use super::dto::{
    mobile_page, MobileCollection, MobileCollectionDetail, MobileEntity, MobileEntityDetail,
    MobileEntityPage, MobileEntitySeriesPage, MobileFacet, MobileMetadata, MobilePage,
    MobilePerson, MobileRevision, MobileSearchPage, MobileSearchRequest, MobileSeriesMembership,
};
use super::library::Library;
use super::store::{AuthorizedDevice, FavoriteUpdate, PairError, ReadingUpdate, Role, ServerStore};
use super::sync_receive::{BindEntry, CommitRequest, ManifestEntry, SyncError, SyncReceiver};
use super::update::{UpdateController, UpdateStatus};
use super::{pairing_payload, API_VERSION};
use crate::database::{AssetEntry, Database};
use axum::body::{Body, Bytes};
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
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tower_http::compression::CompressionLayer;
use tower_http::CompressionLevel;

const MAX_ASSET_BYTES: u64 = 25 * 1024 * 1024;
const MAX_TRANSCODE_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TRANSCODE_PIXELS: u64 = 64_000_000;
/// 挿絵はそのまま渡さず縮小する境目。端末の保存と同じ値にする。
const MAX_RAW_ASSET_EDGE: u32 = 8192;
const MAX_RAW_ASSET_PIXELS: u64 = 16_000_000;
/// 取り込みが許す上限。これを超えるものは展開しない。
const MAX_ASSET_SOURCE_PIXELS: u64 = 100_000_000;
const MAX_JSON_BYTES: usize = 64 * 1024;
const MAX_ADMIN_BYTES: usize = super::CHUNK_BYTES + 64 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const ADMIN_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const READER_PAIRING_LIFETIME: Duration = Duration::from_secs(10 * 60);
const PAIR_ATTEMPTS_PER_MINUTE: usize = 10;

/// `hello` で知らせる機能の一覧。
const CAPABILITIES_EMBEDDED: &[&str] = &["read", "reading-sync", "entity-detail", "favorite-write"];
const CAPABILITIES_STANDALONE: &[&str] = &[
    "read",
    "reading-sync",
    "entity-detail",
    "favorite-write",
    "admin-sync",
];
/// デスクトップ版の作者・シリーズの並び順と同じ語彙。
const ENTITY_SORTS: &[&str] = &[
    "work_count",
    "downloaded_at",
    "source_created_at",
    "source_updated_at",
    "name",
    "text_length",
    "file_size_bytes",
    "asset_count",
    "current_version",
];
/// 縮小した表紙を覚えておく上限。一覧の数百枚ぶんが収まる。
const COVER_CACHE_BYTES: usize = 64 * 1024 * 1024;
/// 端末が指定できる挿絵の幅。細かい幅ごとに変換すると結果を使い回せないので段に丸める。
const ILLUSTRATION_WIDTHS: [u32; 4] = [720, 1080, 1600, 2160];
/// これより小さく、幅も収まっている挿絵は変換せずそのまま渡す。
const ILLUSTRATION_PASS_BYTES: u64 = 512 * 1024;
/// 同時に変換する枚数。本文の要求に計算機を残す。
const ILLUSTRATION_WORKERS: usize = 3;
/// 覚えておく挿絵の縦横の数。本文のページを返すたびにファイルを開かないため。
const DIMENSION_CACHE_ENTRIES: usize = 8192;

pub struct ServerContext {
    pub library: Library,
    pub store: Arc<ServerStore>,
    /// アプリデータに当たる置き場（`downloads`・`profiles` などの親）。
    pub root: PathBuf,
    pub receiver: Option<Arc<SyncReceiver>>,
    /// 携帯の登録文字列に載せる URL。空なら頼んだ管理者の URL を使う。
    pub public_url: Option<String>,
    /// 自己署名で待ち受けているときの証明書の指紋。
    pub fingerprint: Option<String>,
    /// 自己更新の係。`install` で置いた単独サーバーにだけある。
    update: OnceLock<Arc<UpdateController>>,
    /// 管理画面。管理者の口を持つ単独サーバーにだけある。
    web: OnceLock<AdminWeb>,
    started_at: String,
    pair_attempts: Mutex<VecDeque<Instant>>,
    covers: Mutex<CoverCache>,
    illustrations: Mutex<CoverCache>,
    illustration_workers: tokio::sync::Semaphore,
    dimensions: Mutex<HashMap<IllustrationKey, (u32, u32)>>,
    /// 携帯がお気に入りを変えたと、デスクトップ版の画面へ知らせる係。内蔵サーバーにだけある。
    favorites_listener: OnceLock<FavoritesListener>,
}

pub type FavoritesListener = Arc<dyn Fn() + Send + Sync>;

/// 挿絵の寸法を覚える鍵。ファイルが置き換わったら別のものとして測り直す。
type IllustrationKey = (PathBuf, std::time::SystemTime, u64);

/// 表紙の用途ごとの大きさ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CoverSize {
    Card,
    Large,
    Banner,
    /// 本文の挿絵。幅だけを揃え、縦長の絵は縦長のまま残す。
    Illustration(u32),
}

impl CoverSize {
    fn bounds(self) -> (u32, u32) {
        match self {
            Self::Card => (320, 480),
            Self::Large => (640, 960),
            Self::Banner => (1280, 480),
            // 縦長の絵も、端末がそのまま受け取れる一辺と画素数に収める。
            Self::Illustration(width) => {
                let height = (MAX_RAW_ASSET_PIXELS / u64::from(width.max(1)))
                    .min(u64::from(MAX_RAW_ASSET_EDGE));
                (width, u32::try_from(height).unwrap_or(MAX_RAW_ASSET_EDGE))
            }
        }
    }
}

type CoverKey = (PathBuf, std::time::SystemTime, u64, CoverSize);

/// 縮小した表紙を置いておく。変換は重いので、同じ表紙を要求のたびに作り直さない。
/// 元のファイルが変われば更新時刻が変わるので、古いものは返らない。
#[derive(Default)]
struct CoverCache {
    entries: HashMap<CoverKey, (Bytes, String)>,
    order: VecDeque<CoverKey>,
    bytes: usize,
}

impl CoverCache {
    fn get(&mut self, key: &CoverKey) -> Option<(Bytes, String)> {
        let found = self.entries.get(key).cloned()?;
        if let Some(position) = self.order.iter().position(|item| item == key) {
            self.order.remove(position);
        }
        self.order.push_back(key.clone());
        Some(found)
    }

    fn put(&mut self, key: CoverKey, value: (Bytes, String)) {
        if value.0.len() > COVER_CACHE_BYTES || self.entries.contains_key(&key) {
            return;
        }
        self.bytes += value.0.len();
        self.order.push_back(key.clone());
        self.entries.insert(key, value);
        while self.bytes > COVER_CACHE_BYTES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some((bytes, _)) = self.entries.remove(&oldest) {
                self.bytes -= bytes.len();
            }
        }
    }
}

impl ServerContext {
    pub fn new(
        library: Library,
        store: Arc<ServerStore>,
        root: PathBuf,
        receiver: Option<Arc<SyncReceiver>>,
        public_url: Option<String>,
        fingerprint: Option<String>,
    ) -> Arc<Self> {
        Arc::new(Self {
            library,
            store,
            root,
            receiver,
            public_url,
            fingerprint,
            update: OnceLock::new(),
            web: OnceLock::new(),
            started_at: chrono::Utc::now().to_rfc3339(),
            pair_attempts: Mutex::new(VecDeque::new()),
            covers: Mutex::new(CoverCache::default()),
            illustrations: Mutex::new(CoverCache::default()),
            illustration_workers: tokio::sync::Semaphore::new(ILLUSTRATION_WORKERS),
            dimensions: Mutex::new(HashMap::new()),
            favorites_listener: OnceLock::new(),
        })
    }

    pub fn set_favorites_listener(&self, listener: FavoritesListener) {
        let _ = self.favorites_listener.set(listener);
    }

    pub fn set_update(&self, controller: Arc<UpdateController>) {
        let _ = self.update.set(controller);
    }

    pub fn set_admin_web(&self, web: AdminWeb) {
        let _ = self.web.set(web);
    }

    fn pair_attempt_allowed(&self) -> bool {
        let Ok(mut attempts) = self.pair_attempts.lock() else {
            return false;
        };
        let now = Instant::now();
        while attempts
            .front()
            .is_some_and(|at| now.duration_since(*at) >= Duration::from_secs(60))
        {
            attempts.pop_front();
        }
        if attempts.len() >= PAIR_ATTEMPTS_PER_MINUTE {
            return false;
        }
        attempts.push_back(now);
        true
    }
}

type Ctx = Arc<ServerContext>;

#[derive(Debug)]
pub struct ApiError(StatusCode, &'static str, &'static str);

impl ApiError {
    fn unauthorized() -> Self {
        Self(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "端末が登録されていません",
        )
    }
    fn forbidden() -> Self {
        Self(StatusCode::FORBIDDEN, "forbidden", "この端末では行えません")
    }
    fn internal() -> Self {
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "処理できませんでした",
        )
    }
    fn not_found() -> Self {
        Self(StatusCode::NOT_FOUND, "not_found", "見つかりません")
    }
    fn not_ready() -> Self {
        Self(
            StatusCode::SERVICE_UNAVAILABLE,
            "library_not_ready",
            "ライブラリを準備しています",
        )
    }
    fn invalid(message: &'static str) -> Self {
        Self(StatusCode::BAD_REQUEST, "invalid_request", message)
    }
    fn asset_too_large() -> Self {
        Self(
            StatusCode::PAYLOAD_TOO_LARGE,
            "asset_too_large",
            "画像が大きすぎます",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.0,
            Json(serde_json::json!({ "code": self.1, "message": self.2 })),
        )
            .into_response()
    }
}

impl From<SyncError> for ApiError {
    fn from(error: SyncError) -> Self {
        match error {
            SyncError::Invalid(message) => Self::invalid(message),
            SyncError::NotFound => Self::not_found(),
            SyncError::Conflict(message) => Self(StatusCode::CONFLICT, "conflict", message),
            SyncError::Internal(detail) => {
                log::error!("送受信に失敗しました: {detail}");
                Self::internal()
            }
        }
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
            Err(rejection) if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE => Err(ApiError(
                StatusCode::PAYLOAD_TOO_LARGE,
                "too_large",
                "要求が大きすぎます",
            )),
            Err(_) => Err(ApiError::invalid("要求を読み取れません")),
        }
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

fn require_device(ctx: &ServerContext, headers: &HeaderMap) -> Result<AuthorizedDevice, ApiError> {
    let device = bearer(headers)
        .and_then(|token| ctx.store.authorize(token))
        .ok_or_else(ApiError::unauthorized)?;
    Ok(device)
}

fn require_admin(ctx: &ServerContext, headers: &HeaderMap) -> Result<AuthorizedDevice, ApiError> {
    if bearer(headers).is_none() {
        // Cookie を見るのは管理画面を出している口だけで、別のサイトの守りは `guard` が先に通す。
        return admin_web::session_from(ctx, headers)
            .map(|session| admin_web::session_as_device(&session))
            .ok_or_else(ApiError::unauthorized);
    }
    let device = require_device(ctx, headers)?;
    if device.role == Role::Admin {
        Ok(device)
    } else {
        Err(ApiError::forbidden())
    }
}

async fn with_db<T, F>(ctx: &Ctx, work: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&Database) -> Result<T, ApiError> + Send + 'static,
{
    let lease = ctx.library.lease().ok_or_else(ApiError::not_ready)?;
    tokio::task::spawn_blocking(move || work(&lease))
        .await
        .map_err(|_| ApiError::internal())?
}

fn read_routes(ctx: Ctx) -> Router {
    Router::new()
        .route("/api/v1/hello", get(hello))
        .route("/api/v1/pair", post(pair))
        .route("/api/v1/about", get(about))
        .route("/api/v1/works/search", post(search))
        .route("/api/v1/works/{id}", get(metadata))
        .route("/api/v1/works/{id}/pages/{page}", get(page))
        .route("/api/v1/works/{id}/outline", get(outline))
        .route("/api/v1/works/{id}/assets/{asset_id}", get(asset))
        .route("/api/v1/works/{id}/cover", get(cover))
        .route("/api/v1/works/{id}/collections", get(work_collections))
        .route("/api/v1/entities/{kind}", get(entities))
        .route("/api/v1/entities/{kind}/cover", get(entity_cover))
        .route("/api/v1/entities/{kind}/detail", get(entity_detail))
        .route("/api/v1/entities/{kind}/series", get(entity_series))
        .route("/api/v1/collections", get(collections))
        .route("/api/v1/collections/{id}", get(collection))
        .route(
            "/api/v1/collections/{id}/cover/{slot}",
            get(collection_cover),
        )
        .route("/api/v1/reading", get(reading_since))
        .route("/api/v1/reading/{source}/{source_id}", put(reading_put))
        .route("/api/v1/favorites/{source}/{source_id}", put(favorite_put))
        .layer(DefaultBodyLimit::max(MAX_JSON_BYTES))
        .layer(middleware::from_fn(read_timeout))
        .with_state(ctx)
}

fn admin_routes(ctx: Ctx) -> Router {
    let routes = Router::new()
        .merge(admin_web::api_routes())
        .route("/api/v1/admin/status", get(admin_status))
        .route("/api/v1/admin/update", post(admin_update))
        .route("/api/v1/admin/pairings", post(admin_pairing))
        .route("/api/v1/admin/devices", get(admin_devices))
        .route("/api/v1/admin/devices/{id}", delete(admin_revoke))
        .route("/api/v1/admin/favorites", get(admin_favorites))
        .route("/api/v1/admin/favorites/ack", post(admin_favorites_ack))
        .route("/api/v1/admin/sync/sessions", post(sync_open))
        .route("/api/v1/admin/sync/sessions/{id}", delete(sync_abort))
        .route(
            "/api/v1/admin/sync/sessions/{id}/manifest",
            post(sync_manifest),
        )
        .route(
            "/api/v1/admin/sync/sessions/{id}/blobs/{sha}",
            get(sync_blob_status).put(sync_blob_put),
        )
        .route("/api/v1/admin/sync/sessions/{id}/bind", post(sync_bind))
        .route("/api/v1/admin/sync/sessions/{id}/commit", post(sync_commit))
        .layer(DefaultBodyLimit::max(MAX_ADMIN_BYTES))
        .layer(middleware::from_fn(admin_timeout))
        .with_state(Arc::clone(&ctx));
    admin_web::with_guard(routes, ctx)
}

/// 読み手の口。管理の API は載せない。
pub fn reader_router(ctx: Ctx) -> Router {
    finish(read_routes(ctx))
}

/// 読み手と管理の両方を受ける口。管理の口を分けるときは、そちらにもこれを使う。
/// 管理者の登録と証明書の確認に `pair` と `hello` が要るからである。
pub fn combined_router(ctx: Ctx) -> Router {
    finish(read_routes(Arc::clone(&ctx)).merge(admin_routes(ctx)))
}

/// 管理者の口に、管理画面も載せる。`AdminWeb` を渡した単独サーバーだけが使う。
pub fn admin_router(ctx: Ctx) -> Router {
    finish(
        read_routes(Arc::clone(&ctx))
            .merge(admin_routes(Arc::clone(&ctx)))
            .merge(admin_web::web_routes(ctx)),
    )
}

fn finish(router: Router) -> Router {
    router
        .fallback(|| async { ApiError::not_found() })
        // 本文と一覧の JSON は大きく縮む。既定の判定は画像を素通りさせる。
        // 頼まれたときだけ圧縮するので、古い端末の受け取り方は変わらない。
        .layer(CompressionLayer::new().quality(CompressionLevel::Fastest))
        .layer(middleware::from_fn(observe))
}

/// 応答に印を付け、記録を残す。
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
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
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
        Err(_) => ApiError(
            StatusCode::REQUEST_TIMEOUT,
            "timeout",
            "時間内に終わりませんでした",
        )
        .into_response(),
    }
}

async fn hello(State(ctx): State<Ctx>) -> Json<serde_json::Value> {
    let capabilities = if ctx.receiver.is_some() {
        CAPABILITIES_STANDALONE
    } else {
        CAPABILITIES_EMBEDDED
    };
    Json(serde_json::json!({
        "apiVersion": API_VERSION,
        "serverName": "piep",
        "capabilities": capabilities,
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
) -> Result<Json<serde_json::Value>, ApiError> {
    if request.secret.len() > 128 || request.device_name.chars().count() > 64 {
        return Err(ApiError::invalid("登録情報が長すぎます"));
    }
    if !ctx.pair_attempt_allowed() {
        return Err(ApiError::unauthorized());
    }
    match ctx
        .store
        .consume_pairing(&request.secret, &request.device_name)
    {
        Ok(paired) => {
            ctx.store.audit(
                "device",
                "端末の登録",
                "登録文字列",
                None,
                true,
                Some(paired.role.as_str()),
            );
            Ok(Json(serde_json::json!({
            "deviceId": paired.device_id,
            "token": paired.token,
            "role": paired.role,
            "apiVersion": API_VERSION,
            })))
        }
        // 理由を区別しない。鍵が合っているかどうかを外から測らせない。
        Err(PairError::Rejected | PairError::TooManyDevices) => Err(ApiError::unauthorized()),
        Err(PairError::Storage(detail)) => {
            log::error!("端末を登録できません: {detail}");
            Err(ApiError::internal())
        }
    }
}

async fn search(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<MobileSearchRequest>,
) -> Result<Json<MobileSearchPage>, ApiError> {
    require_device(&ctx, &headers)?;
    let params = request
        .into_db_params()
        .map_err(|_| ApiError::invalid("検索条件が正しくありません"))?;
    let result = with_db(&ctx, move |db| {
        db.search_downloads_v2(&params).map_err(|error| {
            log::warn!("検索に失敗しました: {error}");
            ApiError::internal()
        })
    })
    .await?;
    Ok(Json(MobileSearchPage::from(result)))
}

#[derive(Debug, Deserialize)]
struct EntityListQuery {
    text: Option<String>,
    offset: Option<i64>,
    limit: Option<i64>,
    sort: Option<String>,
    order: Option<String>,
}

fn valid_entity_sort(sort: Option<&str>, order: Option<&str>) -> bool {
    sort.is_none_or(|value| ENTITY_SORTS.contains(&value))
        && order.is_none_or(|value| matches!(value, "asc" | "desc"))
}

async fn entities(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath(kind): UrlPath<String>,
    Query(query): Query<EntityListQuery>,
) -> Result<Json<MobileEntityPage>, ApiError> {
    require_device(&ctx, &headers)?;
    let kind = match kind.as_str() {
        "people" => "person",
        "series" => "series",
        _ => return Err(ApiError::not_found()),
    };
    if query
        .text
        .as_ref()
        .is_some_and(|text| text.chars().count() > 200)
        || !(0..=1_000_000).contains(&query.offset.unwrap_or(0))
        || !valid_entity_sort(query.sort.as_deref(), query.order.as_deref())
    {
        return Err(ApiError::invalid("一覧条件が正しくありません"));
    }
    let limit = query.limit.unwrap_or(30).clamp(1, 50);
    let offset = query.offset.unwrap_or(0);
    let page = with_db(&ctx, move |db| {
        let total = db
            .count_entity_facets(kind, query.text.as_deref(), None, None)
            .map_err(|_| ApiError::internal())?;
        let items = db
            .search_entity_facets(
                kind,
                query.text.as_deref(),
                limit,
                offset,
                None,
                Some(query.sort.as_deref().unwrap_or("work_count")),
                Some(query.order.as_deref().unwrap_or("desc")),
                None,
            )
            .map_err(|_| ApiError::internal())?;
        let next_offset =
            (offset + (items.len() as i64) < total).then_some(offset + items.len() as i64);
        Ok(MobileEntityPage {
            items: items.into_iter().map(MobileEntity::from).collect(),
            next_offset,
            total,
        })
    })
    .await?;
    Ok(Json(page))
}

#[derive(Debug, Deserialize)]
struct EntityCoverQuery {
    source: String,
    key: String,
    #[serde(default)]
    variant: Option<String>,
}

fn valid_entity_key(source: &str, key: &str) -> bool {
    matches!(source, "pixiv" | "fanbox") && !key.is_empty() && key.chars().count() <= 200
}

/// プロフィールのリンク。外へ開くものだけを渡す。
fn profile_links(links_json: Option<&str>) -> Vec<String> {
    links_json
        .and_then(|json| serde_json::from_str::<Vec<String>>(json).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|link| link.starts_with("https://") || link.starts_with("http://"))
        .take(20)
        .collect()
}

async fn entity_detail(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath(kind): UrlPath<String>,
    Query(query): Query<EntityCoverQuery>,
) -> Result<Json<MobileEntityDetail>, ApiError> {
    require_device(&ctx, &headers)?;
    if !valid_entity_key(&query.source, &query.key) {
        return Err(ApiError::not_found());
    }
    let detail = with_db(&ctx, move |db| {
        let (source, key) = (query.source, query.key);
        let internal = |_| ApiError::internal();
        match kind.as_str() {
            "people" => {
                let person = db
                    .get_person(&source, &key)
                    .map_err(|_| ApiError::not_found())?;
                let tags = db
                    .list_entity_tags("person", &source, &key, 60)
                    .map_err(internal)?;
                let series = db
                    .list_entity_series_paged(&source, &key, None, None, None, 1, None)
                    .map_err(internal)?;
                let collections = db
                    .list_collections_for_person(&source, &key)
                    .map_err(internal)?;
                Ok(MobileEntityDetail {
                    has_cover: person.icon_path.is_some() || person.cover_path.is_some(),
                    has_banner: person.icon_path.is_some() && person.cover_path.is_some(),
                    links: profile_links(person.links_json.as_deref()),
                    display_name: person.display_name,
                    description: person.description,
                    work_count: person.work_count.unwrap_or(0),
                    is_concluded: None,
                    published_count: None,
                    series_count: Some(series.total),
                    tags: tags
                        .into_iter()
                        .map(|tag| MobileFacet {
                            name: tag.name,
                            count: tag.count,
                        })
                        .collect(),
                    collections: collections
                        .into_iter()
                        .map(MobileCollection::from)
                        .collect(),
                    source,
                    source_key: key,
                })
            }
            "series" => {
                let series = db
                    .get_series(&source, &key)
                    .map_err(|_| ApiError::not_found())?;
                let tags = db
                    .list_entity_tags("series", &source, &key, 60)
                    .map_err(internal)?;
                let collections = db
                    .list_collections_for_series(&source, &key)
                    .map_err(internal)?;
                Ok(MobileEntityDetail {
                    has_cover: series.cover_path.is_some(),
                    has_banner: false,
                    links: Vec::new(),
                    display_name: series.title,
                    description: series.description,
                    work_count: series.work_count.unwrap_or(0),
                    is_concluded: series.is_concluded,
                    published_count: series.published_content_count,
                    series_count: None,
                    tags: tags
                        .into_iter()
                        .map(|tag| MobileFacet {
                            name: tag.name,
                            count: tag.count,
                        })
                        .collect(),
                    collections: collections
                        .into_iter()
                        .map(MobileCollection::from)
                        .collect(),
                    source,
                    source_key: key,
                })
            }
            _ => Err(ApiError::not_found()),
        }
    })
    .await?;
    Ok(Json(detail))
}

#[derive(Debug, Deserialize)]
struct EntitySeriesQuery {
    source: String,
    key: String,
    text: Option<String>,
    sort: Option<String>,
    order: Option<String>,
    cursor: Option<String>,
    limit: Option<i64>,
}

async fn entity_series(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath(kind): UrlPath<String>,
    Query(query): Query<EntitySeriesQuery>,
) -> Result<Json<MobileEntitySeriesPage>, ApiError> {
    require_device(&ctx, &headers)?;
    if kind != "people" || !valid_entity_key(&query.source, &query.key) {
        return Err(ApiError::not_found());
    }
    if query
        .text
        .as_ref()
        .is_some_and(|text| text.chars().count() > 200)
        || !valid_entity_sort(query.sort.as_deref(), query.order.as_deref())
    {
        return Err(ApiError::invalid("一覧条件が正しくありません"));
    }
    let limit = query.limit.unwrap_or(30).clamp(1, 50);
    let page = with_db(&ctx, move |db| {
        db.list_entity_series_paged(
            &query.source,
            &query.key,
            query.text.as_deref().filter(|text| !text.trim().is_empty()),
            query.sort.as_deref(),
            query.order.as_deref(),
            limit,
            query.cursor.as_deref(),
        )
        .map_err(|_| ApiError::invalid("一覧条件が正しくありません"))
    })
    .await?;
    Ok(Json(MobileEntitySeriesPage {
        items: page.items.into_iter().map(MobileEntity::from).collect(),
        next_cursor: page.next_cursor,
        total: page.total,
    }))
}

async fn entity_cover(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath(kind): UrlPath<String>,
    Query(query): Query<EntityCoverQuery>,
) -> Result<Response, ApiError> {
    require_device(&ctx, &headers)?;
    if !valid_entity_key(&query.source, &query.key) {
        return Err(ApiError::not_found());
    }
    let root = ctx.root.clone();
    let banner = query.variant.as_deref() == Some("banner");
    let path = with_db(&ctx, move |db| {
        let (path, area) = match kind.as_str() {
            "people" => {
                let person = db
                    .get_person(&query.source, &query.key)
                    .map_err(|_| ApiError::not_found())?;
                let path = if banner {
                    person.cover_path
                } else {
                    person.icon_path.or(person.cover_path)
                };
                (path, "profiles")
            }
            "series" => (
                db.get_series(&query.source, &query.key)
                    .map_err(|_| ApiError::not_found())?
                    .cover_path,
                "series",
            ),
            _ => return Err(ApiError::not_found()),
        };
        let path = path.ok_or_else(ApiError::not_found)?;
        inside_roots(&path, &[root.join(area), root.join("downloads")])
    })
    .await?;
    let size = if banner {
        CoverSize::Banner
    } else {
        CoverSize::Card
    };
    cover_response(&ctx, path, size).await
}

async fn collections(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
) -> Result<Json<Vec<MobileCollection>>, ApiError> {
    require_device(&ctx, &headers)?;
    let items = with_db(&ctx, |db| {
        db.list_work_collections().map_err(|_| ApiError::internal())
    })
    .await?;
    Ok(Json(
        items.into_iter().map(MobileCollection::from).collect(),
    ))
}

async fn work_collections(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<i64>,
) -> Result<Json<Vec<MobileCollection>>, ApiError> {
    require_device(&ctx, &headers)?;
    if id <= 0 {
        return Err(ApiError::not_found());
    }
    let items = with_db(&ctx, move |db| {
        let work = db.get_download(id).map_err(|_| ApiError::not_found())?;
        let collections = db
            .list_collections_for_work(&work.source, &work.source_id)
            .map_err(|_| ApiError::internal())?;
        let positions: HashMap<_, _> = db
            .list_collection_positions_for_work(&work.source, &work.source_id)
            .map_err(|_| ApiError::internal())?
            .into_iter()
            .collect();
        Ok(collections
            .into_iter()
            .map(|collection| {
                let mut mobile = MobileCollection::from(collection);
                mobile.position = positions.get(&mobile.id).copied();
                mobile
            })
            .collect::<Vec<_>>())
    })
    .await?;
    Ok(Json(items))
}

async fn collection(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<MobileCollectionDetail>, ApiError> {
    require_device(&ctx, &headers)?;
    if id.len() > 128 {
        return Err(ApiError::not_found());
    }
    let item = with_db(&ctx, move |db| {
        db.get_work_collection(&id)
            .map_err(|_| ApiError::not_found())
    })
    .await?;
    Ok(Json(MobileCollectionDetail::from(item)))
}

async fn collection_cover(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath((id, slot)): UrlPath<(String, String)>,
) -> Result<Response, ApiError> {
    require_device(&ctx, &headers)?;
    if id.is_empty() || id.len() > 128 {
        return Err(ApiError::not_found());
    }
    let tile_index = if slot == "file" {
        None
    } else {
        let index = slot
            .strip_prefix("tile-")
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(ApiError::not_found)?;
        if index >= 4 {
            return Err(ApiError::not_found());
        }
        Some(index)
    };
    let root = ctx.root.clone();
    let path = with_db(&ctx, move |db| {
        let collection = db
            .get_work_collection(&id)
            .map_err(|_| ApiError::not_found())?;
        let (path, area) = match tile_index {
            None => {
                if collection.summary.cover_mode != "file" {
                    return Err(ApiError::not_found());
                }
                (
                    collection
                        .summary
                        .cover_image_path
                        .ok_or_else(ApiError::not_found)?,
                    "collection-covers",
                )
            }
            Some(index) => (
                collection
                    .summary
                    .cover_tiles
                    .get(index)
                    .and_then(|tile| tile.cover_path.clone())
                    .ok_or_else(ApiError::not_found)?,
                "downloads",
            ),
        };
        inside_roots(&path, &[root.join(area)])
    })
    .await?;
    cover_response(&ctx, path, CoverSize::Card).await
}

async fn metadata(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<i64>,
) -> Result<Json<MobileMetadata>, ApiError> {
    require_device(&ctx, &headers)?;
    if id <= 0 {
        return Err(ApiError::not_found());
    }
    let public = with_db(&ctx, move |db| {
        let metadata = db
            .get_reader_metadata(id)
            .map_err(|_| ApiError::not_found())?;
        let people = db
            .get_download_people(id)
            .map_err(|_| ApiError::internal())?;
        let series = db
            .get_download_series_list(id)
            .map_err(|_| ApiError::internal())?;
        let mut public = MobileMetadata::from(metadata);
        public.people = people.into_iter().map(MobilePerson::from).collect();
        public.series = series
            .into_iter()
            .map(MobileSeriesMembership::from)
            .collect();
        Ok(public)
    })
    .await?;
    Ok(Json(public))
}

#[derive(Debug, Deserialize)]
struct VersionQuery {
    version: Option<i64>,
}

async fn page(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath((id, page)): UrlPath<(i64, usize)>,
    Query(query): Query<VersionQuery>,
) -> Result<Json<MobilePage>, ApiError> {
    require_device(&ctx, &headers)?;
    if id <= 0 || page > 1_000_000 {
        return Err(ApiError::not_found());
    }
    let sizes = ctx.clone();
    let result = with_db(&ctx, move |db| {
        let revision = || -> Result<MobileRevision, String> {
            Ok(MobileRevision::from(&MobileMetadata::from(
                db.get_reader_metadata(id)?,
            )))
        };
        // 本文を組むあいだに版が変わると、古い本文に新しい版の印を付けてしまう。
        // 前後で同じだったときだけ、その版の本文として返す。
        let attempt = || -> Result<Option<MobilePage>, String> {
            let before = revision()?;
            let assets = db.get_assets(id)?;
            let content = db.get_reader_content_page(id, query.version, page, false)?;
            let rendered = mobile_page(content, &assets, before.clone(), |asset| {
                illustration_size(&sizes, asset)
            })?;
            Ok((revision()? == before).then_some(rendered))
        };
        for _ in 0..2 {
            match attempt() {
                Ok(Some(rendered)) => return Ok(rendered),
                Ok(None) => {}
                Err(error) => {
                    log::warn!("本文を渡せません: {error}");
                    return Err(ApiError::internal());
                }
            }
        }
        // 書き込みが続いている。少し置いて読み直してもらう。
        Err(ApiError::not_ready())
    })
    .await?;
    Ok(Json(result))
}

/// 挿絵の縦横。ファイルの頭だけを読み、ファイルが変わるまで覚えておく。
fn illustration_size(ctx: &ServerContext, asset: &AssetEntry) -> Option<(u32, u32)> {
    if !asset.mime_type.as_deref()?.starts_with("image/") {
        return None;
    }
    let path = PathBuf::from(&asset.local_path);
    let metadata = fs::metadata(&path).ok()?;
    let key = (path, metadata.modified().ok()?, metadata.len());
    if let Some(found) = ctx.dimensions.lock().ok()?.get(&key) {
        return Some(*found);
    }
    let size = image::image_dimensions(&key.0).ok()?;
    let mut known = ctx.dimensions.lock().ok()?;
    if known.len() >= DIMENSION_CACHE_ENTRIES {
        known.clear();
    }
    known.insert(key, size);
    Some(size)
}

async fn outline(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<i64>,
    Query(query): Query<VersionQuery>,
) -> Result<Json<Vec<crate::database::ReaderOutlineEntry>>, ApiError> {
    require_device(&ctx, &headers)?;
    if id <= 0 {
        return Err(ApiError::not_found());
    }
    let result = with_db(&ctx, move |db| {
        db.get_reader_outline(id, query.version)
            .map_err(|_| ApiError::not_found())
    })
    .await?;
    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
struct AssetQuery {
    #[serde(default)]
    width: Option<u32>,
}

async fn asset(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath((id, asset_id)): UrlPath<(i64, i64)>,
    Query(query): Query<AssetQuery>,
) -> Result<Response, ApiError> {
    require_device(&ctx, &headers)?;
    if id <= 0 || asset_id <= 0 {
        return Err(ApiError::not_found());
    }
    let root = ctx.root.clone();
    let wanted = query.width.and_then(illustration_width);
    let (path, mime) = with_db(&ctx, move |db| {
        let asset = db
            .get_assets(id)
            .map_err(|_| ApiError::not_found())?
            .into_iter()
            .find(|asset| asset.id == asset_id && asset.download_id == id)
            .ok_or_else(ApiError::not_found)?;
        let mime = asset.mime_type.as_deref().ok_or_else(ApiError::not_found)?;
        if !matches!(
            mime,
            "image/jpeg" | "image/png" | "image/webp" | "image/gif" | "image/avif"
        ) {
            return Err(ApiError::not_found());
        }
        let path = inside_roots(&asset.local_path, &[root.join("downloads")])?;
        Ok((path, mime.to_string()))
    })
    .await?;
    let (bytes, mime) = match wanted {
        Some(width) => illustration_bytes_for(&ctx, path, mime, width).await?,
        // 接続を握ったまま大きなファイルを読まない。
        None => tokio::task::spawn_blocking(move || {
            let size = fs::metadata(&path)
                .map_err(|_| ApiError::not_found())?
                .len();
            asset_bytes(&path, &mime, size).map(|(bytes, mime)| (Bytes::from(bytes), mime))
        })
        .await
        .map_err(|_| ApiError::internal())??,
    };
    image_response(bytes, &mime)
}

/// 頼まれた幅を、収まる段へ切り上げる。大きすぎる指定は最大の段に収める。
fn illustration_width(width: u32) -> Option<u32> {
    (width > 0).then(|| {
        ILLUSTRATION_WIDTHS
            .iter()
            .copied()
            .find(|step| *step >= width)
            .unwrap_or(ILLUSTRATION_WIDTHS[ILLUSTRATION_WIDTHS.len() - 1])
    })
}

/// 動く画像は縮めると止まってしまう。読めないものも触らない。
fn is_animated(path: &Path, mime: &str) -> bool {
    use std::io::Read;
    let marker: &[u8] = match mime {
        "image/png" => b"acTL",
        "image/webp" => b"ANIM",
        "image/jpeg" => return false,
        _ => return true,
    };
    let mut head = Vec::with_capacity(64 * 1024);
    let read = fs::File::open(path).and_then(|file| file.take(64 * 1024).read_to_end(&mut head));
    read.is_err() || head.windows(marker.len()).any(|window| window == marker)
}

/// 挿絵を端末の幅に合わせて返す。縮められないときは、今までどおり原本を渡す。
///
/// 原本は 1 枚で数 MB あり、画面の幅に要るのはその一部である。縮めた結果は覚えておく。
async fn illustration_bytes_for(
    ctx: &Ctx,
    path: PathBuf,
    mime: String,
    width: u32,
) -> Result<(Bytes, String), ApiError> {
    let _slot = ctx
        .illustration_workers
        .acquire()
        .await
        .map_err(|_| ApiError::internal())?;
    let ctx = ctx.clone();
    tokio::task::spawn_blocking(move || {
        let metadata = fs::metadata(&path).map_err(|_| ApiError::not_found())?;
        let modified = metadata.modified().map_err(|_| ApiError::not_found())?;
        let size = metadata.len();
        let original =
            || asset_bytes(&path, &mime, size).map(|(bytes, mime)| (Bytes::from(bytes), mime));
        let fits = || {
            size <= ILLUSTRATION_PASS_BYTES
                && image::image_dimensions(&path).is_ok_and(|(actual, _)| actual <= width)
        };
        if is_animated(&path, &mime) || fits() {
            return original();
        }
        let key = (path.clone(), modified, size, CoverSize::Illustration(width));
        if let Some(found) = ctx
            .illustrations
            .lock()
            .ok()
            .and_then(|mut made| made.get(&key))
        {
            return Ok(found);
        }
        match illustration_bytes(&path, size, width) {
            // 縮めたほうが大きくなる絵もある。そのときは原本のままがよい。
            Ok((bytes, made_mime)) if (bytes.len() as u64) < size => {
                let made = (Bytes::from(bytes), made_mime);
                if let Ok(mut cache) = ctx.illustrations.lock() {
                    cache.put(key, made.clone());
                }
                Ok(made)
            }
            _ => original(),
        }
    })
    .await
    .map_err(|_| ApiError::internal())?
}

fn illustration_bytes(path: &Path, size: u64, width: u32) -> Result<(Vec<u8>, String), ApiError> {
    if size > MAX_TRANSCODE_SOURCE_BYTES {
        return Err(ApiError::asset_too_large());
    }
    let (actual_width, actual_height) =
        image::image_dimensions(path).map_err(|_| ApiError::not_found())?;
    if u64::from(actual_width) * u64::from(actual_height) > MAX_TRANSCODE_PIXELS {
        return Err(ApiError::asset_too_large());
    }
    let (max_width, max_height) = CoverSize::Illustration(width).bounds();
    let options = crate::epub::intermediate::ImageCompressOptions {
        enabled: true,
        max_width: Some(max_width),
        max_height: Some(max_height),
        output_format: Some("webp".into()),
        webp_quality: 82,
        webp_method: 0,
        ..Default::default()
    };
    let (bytes, mime) = crate::epub::image_processor::process_image(path, &options)
        .map_err(|_| ApiError::internal())?;
    if bytes.len() as u64 > MAX_ASSET_BYTES {
        return Err(ApiError::asset_too_large());
    }
    Ok((bytes, mime))
}

#[derive(Debug, Deserialize)]
struct WorkCoverQuery {
    #[serde(default)]
    size: Option<String>,
}

async fn cover(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<i64>,
    Query(query): Query<WorkCoverQuery>,
) -> Result<Response, ApiError> {
    require_device(&ctx, &headers)?;
    if id <= 0 {
        return Err(ApiError::not_found());
    }
    let root = ctx.root.clone();
    let path = with_db(&ctx, move |db| {
        let work = db.get_download(id).map_err(|_| ApiError::not_found())?;
        let path = work.cover_path.ok_or_else(ApiError::not_found)?;
        inside_roots(&path, &[root.join("downloads")])
    })
    .await?;
    let size = if query.size.as_deref() == Some("large") {
        CoverSize::Large
    } else {
        CoverSize::Card
    };
    cover_response(&ctx, path, size).await
}

#[derive(Debug, Deserialize)]
struct ReadingQuery {
    since: Option<i64>,
    limit: Option<i64>,
}

async fn reading_since(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    Query(query): Query<ReadingQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_device(&ctx, &headers)?;
    let since = query.since.unwrap_or(0).max(0);
    let limit = query.limit.unwrap_or(200).clamp(1, 500);
    let items = ctx.store.reading_since(since, limit).map_err(|error| {
        log::error!("読書位置を読めません: {error}");
        ApiError::internal()
    })?;
    let cursor = items.last().map_or(since, |item| item.cursor);
    Ok(Json(
        serde_json::json!({ "items": items, "cursor": cursor }),
    ))
}

/// 取得元と取得元の番号として受け付ける形か。
fn valid_work_key(source: &str, source_id: &str) -> bool {
    matches!(source, "pixiv" | "fanbox")
        && !source_id.is_empty()
        && source_id.chars().count() <= 200
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FavoriteState {
    source: String,
    source_id: String,
    favorite: bool,
}

/// 携帯からお気に入りを付け外しする。
///
/// 内蔵サーバーはデスクトップ版の本棚へそのまま書く。単独サーバーは自分の本棚へ書いたうえで
/// 記録を残し、デスクトップ版が取り込むまで、送られてきた本棚へも当て直す。
async fn favorite_put(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath((source, source_id)): UrlPath<(String, String)>,
    ApiJson(update): ApiJson<FavoriteUpdate>,
) -> Result<Json<FavoriteState>, ApiError> {
    let device = require_device(&ctx, &headers)?;
    if !valid_work_key(&source, &source_id) {
        return Err(ApiError::not_found());
    }
    if update.device_time <= 0 {
        return Err(ApiError::invalid("お気に入りの変更が正しくありません"));
    }
    let favorite = match &ctx.library {
        Library::App(state) => {
            // デスクトップ版の書き込みと同じ順番待ちに並ぶ。取り込みや作り直しの途中に割り込まない。
            let gate = state.library_gate.clone().write_owned().await;
            let state = Arc::clone(state);
            let (key, id, favorite) = (source.clone(), source_id.clone(), update.favorite);
            let found = tokio::task::spawn_blocking(move || {
                let _gate = gate;
                state.db.set_favorite_by_source(&key, &id, favorite)
            })
            .await
            .map_err(|_| ApiError::internal())?
            .map_err(|error| {
                log::error!("お気に入りを書けません: {error}");
                ApiError::internal()
            })?;
            found.ok_or_else(ApiError::not_found)?;
            if let Some(listener) = ctx.favorites_listener.get() {
                listener();
            }
            favorite
        }
        Library::Swappable(_) => {
            let store = Arc::clone(&ctx.store);
            let (key, id) = (source.clone(), source_id.clone());
            let stored = with_db(&ctx, move |db| {
                store
                    .apply_favorite(db, &key, &id, &device.id, &update)
                    .map_err(|error| {
                        log::error!("お気に入りを書けません: {error}");
                        ApiError::internal()
                    })
            })
            .await?;
            stored.ok_or_else(ApiError::not_found)?.favorite
        }
    };
    Ok(Json(FavoriteState {
        source,
        source_id,
        favorite,
    }))
}

/// 携帯で変えたお気に入りを、デスクトップ版が取り込むために読む。単独サーバーにだけある。
async fn admin_favorites(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    Query(query): Query<ReadingQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&ctx, &headers)?;
    if ctx.receiver.is_none() {
        return Err(ApiError::not_found());
    }
    let since = query.since.unwrap_or(0).max(0);
    let limit = query.limit.unwrap_or(200).clamp(1, 500);
    let items = ctx
        .store
        .favorite_changes_since(since, limit)
        .map_err(|error| {
            log::error!("お気に入りの変更を読めません: {error}");
            ApiError::internal()
        })?;
    let cursor = items.last().map_or(since, |item| item.cursor);
    Ok(Json(
        serde_json::json!({ "items": items, "cursor": cursor }),
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FavoritesAck {
    cursor: i64,
}

/// デスクトップ版が `cursor` までを自分の本棚へ取り込んだ。その分は送られてくる本棚に入っている。
async fn admin_favorites_ack(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    ApiJson(ack): ApiJson<FavoritesAck>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&ctx, &headers)?;
    if ctx.receiver.is_none() {
        return Err(ApiError::not_found());
    }
    if ack.cursor < 0 {
        return Err(ApiError::invalid("取り込んだ位置が正しくありません"));
    }
    ctx.store
        .forget_favorite_changes(ack.cursor)
        .map_err(|error| {
            log::error!("取り込んだお気に入りを消せません: {error}");
            ApiError::internal()
        })?;
    Ok(Json(serde_json::json!({ "cursor": ack.cursor })))
}

async fn reading_put(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath((source, source_id)): UrlPath<(String, String)>,
    ApiJson(update): ApiJson<ReadingUpdate>,
) -> Result<Json<super::store::ReadingPosition>, ApiError> {
    let device = require_device(&ctx, &headers)?;
    if !valid_work_key(&source, &source_id) {
        return Err(ApiError::not_found());
    }
    if !(0..=1_000_000).contains(&update.page)
        || !(0.0..=1.0).contains(&update.position)
        || update.device_time <= 0
        || update
            .edit_hash
            .as_ref()
            .is_some_and(|hash| hash.len() > 128)
    {
        return Err(ApiError::invalid("読書位置が正しくありません"));
    }
    let stored = ctx
        .store
        .put_reading(&source, &source_id, &device.id, &update)
        .map_err(|error| {
            log::error!("読書位置を書けません: {error}");
            ApiError::internal()
        })?;
    Ok(Json(stored))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminStatus {
    version: &'static str,
    api_version: u32,
    works: Option<i64>,
    devices: Vec<super::store::DeviceSummary>,
    sync: Option<super::sync_receive::ReceiverStatus>,
    public_url: Option<String>,
    update: Option<UpdateStatus>,
    server_name: String,
    started_at: String,
    /// `available`、`public-only`（公開の口しかない）、`disabled`（止めてある）、`none`。
    admin_web: &'static str,
}

async fn admin_status(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
) -> Result<Json<AdminStatus>, ApiError> {
    require_admin(&ctx, &headers)?;
    let works = match ctx.library.lease() {
        Some(lease) => tokio::task::spawn_blocking(move || {
            lease.get_stats().ok().map(|stats| stats.total_downloads)
        })
        .await
        .map_err(|_| ApiError::internal())?,
        None => None,
    };
    let devices = ctx.store.list_devices().map_err(|_| ApiError::internal())?;
    Ok(Json(AdminStatus {
        version: super::update::VERSION,
        api_version: API_VERSION,
        works,
        devices,
        sync: ctx.receiver.as_ref().map(|receiver| receiver.status()),
        public_url: ctx.public_url.clone(),
        update: ctx.update.get().map(|controller| controller.status()),
        server_name: ctx.store.server_name(),
        started_at: ctx.started_at.clone(),
        admin_web: admin_web_state(&ctx),
    }))
}

fn admin_web_state(ctx: &ServerContext) -> &'static str {
    if ctx.web.get().is_none() {
        if ctx.public_url.is_some() && ctx.receiver.is_some() {
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
    let mut capabilities: Vec<&str> = if ctx.receiver.is_some() {
        CAPABILITIES_STANDALONE.to_vec()
    } else {
        CAPABILITIES_EMBEDDED.to_vec()
    };
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
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&ctx, &headers)?;
    let controller = ctx.update.get().ok_or(ApiError(
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
    headers: HeaderMap,
    ApiJson(request): ApiJson<PairingRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&ctx, &headers)?;
    if request.role.as_deref() == Some("admin") {
        admin_web::require_for_admin_pairing(&ctx, &headers, &ip)?;
        let url = request
            .url
            .filter(|url| url.len() <= 512 && url.starts_with("http"))
            .ok_or_else(|| ApiError::invalid("このサーバーへ届く URL が決まっていません"))?;
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
    let url = ctx
        .public_url
        .clone()
        .or(request.url)
        .filter(|url| url.len() <= 512 && url.starts_with("https://"))
        .ok_or_else(|| ApiError::invalid("携帯が接続する https の URL が決まっていません。server.toml の public_url に書いてください"))?;
    let secret = ctx
        .store
        .issue_pairing(Role::Reader, READER_PAIRING_LIFETIME)
        .map_err(|_| ApiError::internal())?;
    Ok(Json(serde_json::json!({
        "payload": pairing_payload(Role::Reader, &url, ctx.fingerprint.as_deref(), &secret),
        "expiresInSeconds": READER_PAIRING_LIFETIME.as_secs(),
    })))
}

async fn admin_devices(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
) -> Result<Json<Vec<super::store::DeviceSummary>>, ApiError> {
    require_admin(&ctx, &headers)?;
    Ok(Json(
        ctx.store.list_devices().map_err(|_| ApiError::internal())?,
    ))
}

async fn admin_revoke(
    State(ctx): State<Ctx>,
    Extension(ip): Extension<admin_web::ClientIp>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Result<StatusCode, ApiError> {
    require_admin(&ctx, &headers)?;
    if ctx
        .store
        .revoke_device(&id)
        .map_err(|_| ApiError::internal())?
    {
        admin_web::note_device(&ctx, &headers, &ip, "端末の解除");
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found())
    }
}

fn receiver(ctx: &ServerContext) -> Result<Arc<SyncReceiver>, ApiError> {
    ctx.receiver.clone().ok_or_else(ApiError::not_found)
}

async fn blocking<T, F>(work: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, SyncError> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(ApiError::from)
}

async fn sync_open(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&ctx, &headers)?;
    let receiver = receiver(&ctx)?;
    let opened = blocking(move || receiver.open_session()).await?;
    Ok(Json(
        serde_json::to_value(opened).map_err(|_| ApiError::internal())?,
    ))
}

async fn sync_abort(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Result<StatusCode, ApiError> {
    require_admin(&ctx, &headers)?;
    let receiver = receiver(&ctx)?;
    blocking(move || receiver.abort_session(&id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManifestRequest {
    files: Vec<ManifestEntry>,
}

async fn sync_manifest(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    ApiJson(request): ApiJson<ManifestRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&ctx, &headers)?;
    if request.files.len() > super::MANIFEST_BATCH {
        return Err(ApiError::invalid("一度に送る一覧が大きすぎます"));
    }
    let receiver = receiver(&ctx)?;
    let needed = blocking(move || receiver.add_manifest(&id, &request.files)).await?;
    Ok(Json(serde_json::json!({ "needed": needed })))
}

async fn sync_blob_status(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath((id, sha)): UrlPath<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&ctx, &headers)?;
    let receiver = receiver(&ctx)?;
    let state = blocking(move || receiver.blob_state(&id, &sha)).await?;
    Ok(Json(
        serde_json::to_value(state).map_err(|_| ApiError::internal())?,
    ))
}

#[derive(Debug, Deserialize)]
struct BlobQuery {
    offset: u64,
    total: u64,
}

async fn sync_blob_put(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath((id, sha)): UrlPath<(String, String)>,
    Query(query): Query<BlobQuery>,
    body: Bytes,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&ctx, &headers)?;
    let receiver = receiver(&ctx)?;
    let state =
        blocking(move || receiver.write_blob(&id, &sha, query.offset, query.total, &body)).await?;
    Ok(Json(
        serde_json::to_value(state).map_err(|_| ApiError::internal())?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BindRequest {
    files: Vec<BindEntry>,
}

async fn sync_bind(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    ApiJson(request): ApiJson<BindRequest>,
) -> Result<StatusCode, ApiError> {
    require_admin(&ctx, &headers)?;
    if request.files.len() > super::MANIFEST_BATCH {
        return Err(ApiError::invalid("一度に送る一覧が大きすぎます"));
    }
    let receiver = receiver(&ctx)?;
    blocking(move || receiver.bind_files(&id, &request.files)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn sync_commit(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    ApiJson(request): ApiJson<CommitRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&ctx, &headers)?;
    let receiver = receiver(&ctx)?;
    let outcome = blocking(move || receiver.commit(&id, &request)).await?;
    let value = serde_json::to_value(outcome).map_err(|_| ApiError::internal())?;
    let detail = value["generation"].as_i64().map(|at| format!("世代 {at}"));
    admin_web::note_library(&ctx, "ライブラリの受信", detail.as_deref());
    Ok(Json(value))
}

/// DB に書かれたパスが置き場の中を指すときだけ、実体のパスを返す。
fn inside_roots(path: &str, roots: &[PathBuf]) -> Result<PathBuf, ApiError> {
    let path = Path::new(path)
        .canonicalize()
        .map_err(|_| ApiError::not_found())?;
    let inside = roots
        .iter()
        .filter_map(|root| root.canonicalize().ok())
        .any(|root| path.starts_with(root));
    if inside {
        Ok(path)
    } else {
        Err(ApiError::not_found())
    }
}

fn image_response(bytes: impl Into<Body>, mime: &str) -> Result<Response, ApiError> {
    let mut response = Response::new(bytes.into());
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime).map_err(|_| ApiError::internal())?,
    );
    Ok(response)
}

/// 表紙を返す。本棚の接続を握ったまま変換すると、ほかの要求まで待たされるので、
/// パスが分かって接続を返してから変換する。
async fn cover_response(ctx: &Ctx, path: PathBuf, size: CoverSize) -> Result<Response, ApiError> {
    let ctx = ctx.clone();
    let (bytes, mime) = tokio::task::spawn_blocking(move || {
        let metadata = fs::metadata(&path).map_err(|_| ApiError::not_found())?;
        let modified = metadata.modified().map_err(|_| ApiError::not_found())?;
        let key = (path.clone(), modified, metadata.len(), size);
        if let Some(found) = ctx
            .covers
            .lock()
            .ok()
            .and_then(|mut covers| covers.get(&key))
        {
            return Ok(found);
        }
        let (bytes, mime) = cover_bytes(&path, size)?;
        let made = (Bytes::from(bytes), mime);
        if let Ok(mut covers) = ctx.covers.lock() {
            covers.put(key, made.clone());
        }
        Ok::<_, ApiError>(made)
    })
    .await
    .map_err(|_| ApiError::internal())??;
    image_response(bytes, &mime)
}

fn cover_bytes(path: &Path, cover_size: CoverSize) -> Result<(Vec<u8>, String), ApiError> {
    let size = fs::metadata(path).map_err(|_| ApiError::not_found())?.len();
    if size > MAX_TRANSCODE_SOURCE_BYTES {
        return Err(ApiError::asset_too_large());
    }
    let (width, height) = image::image_dimensions(path).map_err(|_| ApiError::not_found())?;
    if u64::from(width) * u64::from(height) > MAX_TRANSCODE_PIXELS {
        return Err(ApiError::asset_too_large());
    }
    let options = crate::epub::intermediate::ImageCompressOptions {
        enabled: true,
        max_width: Some(cover_size.bounds().0),
        max_height: Some(cover_size.bounds().1),
        output_format: Some("webp".into()),
        webp_quality: 78,
        webp_method: 0,
        ..Default::default()
    };
    let (bytes, mime) = crate::epub::image_processor::process_image(path, &options)
        .map_err(|_| ApiError::not_found())?;
    if bytes.len() as u64 > MAX_ASSET_BYTES || !matches!(mime.as_str(), "image/webp" | "image/gif")
    {
        return Err(ApiError::asset_too_large());
    }
    Ok((bytes, mime))
}

/// 挿絵をどう返すか。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssetPlan {
    Raw,
    Shrink,
    Refuse,
}

/// 圧縮後の大きさでは展開後のメモリが分からないので、寸法で決める。
/// 9000×9000 の PNG は数百 KB でも、展開すると 300MB を超える。
fn asset_plan(mime: &str, size: u64, width: u32, height: u32) -> AssetPlan {
    let pixels = u64::from(width) * u64::from(height);
    let oversize = width.max(height) > MAX_RAW_ASSET_EDGE
        || pixels > MAX_RAW_ASSET_PIXELS
        || size > MAX_ASSET_BYTES;
    if !oversize {
        return AssetPlan::Raw;
    }
    if size > MAX_TRANSCODE_SOURCE_BYTES
        || pixels > MAX_ASSET_SOURCE_PIXELS
        || !matches!(mime, "image/jpeg" | "image/png" | "image/webp")
    {
        return AssetPlan::Refuse;
    }
    AssetPlan::Shrink
}

/// AVIF の寸法を読む。`image` の機能では開けないので、必須の `ispe` ボックスを探す。
/// 画像が複数あるファイルでは、いちばん大きいものを返す。
fn avif_dimensions(path: &Path) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut head = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(256 * 1024)
        .read_to_end(&mut head)
        .ok()?;
    let mut largest: Option<(u32, u32)> = None;
    for (at, _) in head
        .windows(4)
        .enumerate()
        .filter(|(_, tag)| tag == b"ispe")
    {
        let field = |offset: usize| -> Option<u32> {
            let bytes = head.get(at + offset..at + offset + 4)?;
            Some(u32::from_be_bytes(bytes.try_into().ok()?))
        };
        // 名前の次に、版と印が 4 バイト、幅と高さが 4 バイトずつ続く。
        let (Some(width), Some(height)) = (field(8), field(12)) else {
            continue;
        };
        if largest
            .is_none_or(|(w, h)| u64::from(w) * u64::from(h) < u64::from(width) * u64::from(height))
        {
            largest = Some((width, height));
        }
    }
    largest
}

fn asset_dimensions(path: &Path, mime: &str) -> Option<(u32, u32)> {
    if mime == "image/avif" {
        avif_dimensions(path)
    } else {
        image::image_dimensions(path).ok()
    }
}

/// 大きな画像の展開は 1 枚ずつにする。何枚も同時に頼まれてメモリを使い切らないように。
static LARGE_ASSET_DECODE: Mutex<()> = Mutex::new(());

fn asset_bytes(path: &Path, mime: &str, size: u64) -> Result<(Vec<u8>, String), ApiError> {
    let (width, height) = asset_dimensions(path, mime).ok_or_else(ApiError::not_found)?;
    match asset_plan(mime, size, width, height) {
        AssetPlan::Raw => {
            return fs::read(path)
                .map(|bytes| (bytes, mime.to_string()))
                .map_err(|_| ApiError::not_found());
        }
        AssetPlan::Refuse => return Err(ApiError::asset_too_large()),
        AssetPlan::Shrink => {}
    }
    // 高解像度の挿絵をそのまま WebView に渡すと、転送量と展開後のメモリが大きい。
    // 元ファイルは変更せず、端末向けの縮小版だけを応答として生成する。
    let _decoding = LARGE_ASSET_DECODE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let options = crate::epub::intermediate::ImageCompressOptions {
        enabled: true,
        max_width: Some(3072),
        max_height: Some(3072),
        output_format: Some("webp".into()),
        webp_quality: 85,
        webp_method: 0,
        ..Default::default()
    };
    let (bytes, output_mime) = crate::epub::image_processor::process_image(path, &options)
        .map_err(|error| {
            log::warn!("端末向けの画像変換に失敗しました: {error}");
            ApiError::internal()
        })?;
    if bytes.len() as u64 > MAX_ASSET_BYTES {
        return Err(ApiError::asset_too_large());
    }
    Ok((bytes, output_mime))
}

#[cfg(test)]
mod asset_tests {
    use super::*;

    #[test]
    fn dimensions_decide_before_the_file_size() {
        let png = "image/png";
        assert_eq!(asset_plan(png, 1_000, 2000, 3000), AssetPlan::Raw);
        // 監査の例。236KB の 9000×9000 は 81MP で、そのままは渡さない。
        assert_eq!(asset_plan(png, 236_266, 9000, 9000), AssetPlan::Shrink);
        assert_eq!(asset_plan(png, 1_000, 8193, 10), AssetPlan::Shrink);
        assert_eq!(asset_plan(png, 1_000, 4001, 4001), AssetPlan::Shrink);
        assert_eq!(
            asset_plan(png, MAX_ASSET_BYTES + 1, 100, 100),
            AssetPlan::Shrink
        );
        // 展開しないものと、縮小できない形式。
        assert_eq!(asset_plan(png, 1_000, 10_001, 10_000), AssetPlan::Refuse);
        assert_eq!(
            asset_plan(png, MAX_TRANSCODE_SOURCE_BYTES + 1, 100, 100),
            AssetPlan::Refuse
        );
        assert_eq!(
            asset_plan("image/gif", 1_000, 9000, 9000),
            AssetPlan::Refuse
        );
        assert_eq!(
            asset_plan("image/avif", 1_000, 9000, 9000),
            AssetPlan::Refuse
        );
        assert_eq!(asset_plan("image/gif", 1_000, 300, 300), AssetPlan::Raw);
    }

    #[test]
    fn a_small_file_with_a_long_edge_comes_back_shrunk() {
        let dir = std::env::temp_dir().join(format!("piep-asset-{}", random_name()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("long.png");
        image::GrayImage::new(9000, 900).save(&path).unwrap();
        let size = fs::metadata(&path).unwrap().len();
        assert!(size < MAX_ASSET_BYTES);
        let (bytes, mime) = asset_bytes(&path, "image/png", size).unwrap();
        assert_eq!(mime, "image/webp");
        let (width, _) = image::load_from_memory(&bytes)
            .unwrap()
            .to_rgba8()
            .dimensions();
        assert!(width <= 3072, "{width}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn avif_size_comes_from_the_largest_ispe_box() {
        let dir = std::env::temp_dir().join(format!("piep-avif-{}", random_name()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("x.avif");
        let mut file = vec![0_u8; 32];
        for (width, height) in [(320_u32, 200_u32), (9000, 9000)] {
            file.extend_from_slice(&20_u32.to_be_bytes());
            file.extend_from_slice(b"ispe");
            file.extend_from_slice(&[0; 4]);
            file.extend_from_slice(&width.to_be_bytes());
            file.extend_from_slice(&height.to_be_bytes());
        }
        fs::write(&path, &file).unwrap();
        assert_eq!(avif_dimensions(&path), Some((9000, 9000)));
        fs::write(&path, b"not an image").unwrap();
        assert_eq!(avif_dimensions(&path), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    fn random_name() -> String {
        super::super::store::random_token(6)
    }
}

#[cfg(test)]
mod illustration_tests {
    use super::*;

    #[test]
    fn widths_round_up_to_a_shared_step() {
        assert_eq!(illustration_width(0), None);
        assert_eq!(illustration_width(1), Some(720));
        assert_eq!(illustration_width(1080), Some(1080));
        assert_eq!(illustration_width(1081), Some(1600));
        assert_eq!(illustration_width(100_000), Some(2160));
    }

    #[test]
    fn a_large_picture_shrinks_to_the_width_and_keeps_its_shape() {
        let dir = crate::server::tests::TestRoot::created("illustration");
        let path = dir.0.join("large.png");
        image::RgbImage::from_pixel(2400, 1200, image::Rgb([200, 100, 50]))
            .save(&path)
            .expect("save");
        let size = fs::metadata(&path).expect("metadata").len();
        let (bytes, mime) = illustration_bytes(&path, size, 1080).expect("shrunk");
        assert_eq!(mime, "image/webp");
        let made = image::load_from_memory(&bytes).expect("decode");
        assert_eq!((made.width(), made.height()), (1080, 540));
    }

    #[test]
    fn moving_pictures_are_left_alone() {
        let dir = crate::server::tests::TestRoot::created("illustration");
        let still = dir.0.join("still.png");
        let moving = dir.0.join("moving.png");
        fs::write(&still, b"\x89PNG\r\n\x1a\nIHDR....IDAT").expect("write");
        fs::write(&moving, b"\x89PNG\r\n\x1a\nIHDR....acTL....IDAT").expect("write");
        assert!(!is_animated(&still, "image/png"));
        assert!(is_animated(&moving, "image/png"));
        assert!(!is_animated(&still, "image/jpeg"));
        assert!(is_animated(&still, "image/gif"));
    }

    #[test]
    fn a_tall_strip_shrinks_within_what_the_phone_accepts() {
        let dir = crate::server::tests::TestRoot::created("illustration");
        let path = dir.0.join("tall.png");
        image::GrayImage::new(2400, 12_000)
            .save(&path)
            .expect("save");
        let size = fs::metadata(&path).expect("metadata").len();
        for width in ILLUSTRATION_WIDTHS {
            let (bytes, _) = illustration_bytes(&path, size, width).expect("shrunk");
            let made = image::load_from_memory(&bytes).expect("decode");
            assert!(made.width() <= width, "{width}");
            assert!(made.height() <= MAX_RAW_ASSET_EDGE, "{width}");
            assert!(
                u64::from(made.width()) * u64::from(made.height()) <= MAX_RAW_ASSET_PIXELS,
                "{width}"
            );
        }
    }
}
