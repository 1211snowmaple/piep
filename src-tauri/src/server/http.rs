//! サーバーの HTTP 層。読み手の API（`/api/v1/...`）と管理の API（`/api/v1/admin/...`）。
//!
//! URL が知られていても読めないことを前提にする。無認証で答えるのは `hello` と
//! `pair` だけで、応答にパスや内部の誤り文を出さない。

use super::dto::{
    mobile_page, MobileCollection, MobileCollectionDetail, MobileEntity, MobileEntityPage,
    MobileMetadata, MobilePage, MobilePerson, MobileSearchPage, MobileSearchRequest,
    MobileSeriesMembership,
};
use super::library::Library;
use super::store::{AuthorizedDevice, PairError, ReadingUpdate, Role, ServerStore};
use super::sync_receive::{BindEntry, CommitRequest, ManifestEntry, SyncError, SyncReceiver};
use super::update::{UpdateController, UpdateStatus};
use super::{pairing_payload, API_VERSION};
use crate::database::Database;
use axum::body::{Body, Bytes};
use axum::extract::rejection::JsonRejection;
use axum::extract::{
    DefaultBodyLimit, FromRequest, MatchedPath, Path as UrlPath, Query, Request, State,
};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_ASSET_BYTES: u64 = 25 * 1024 * 1024;
const MAX_TRANSCODE_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TRANSCODE_PIXELS: u64 = 64_000_000;
const MAX_JSON_BYTES: usize = 64 * 1024;
const MAX_ADMIN_BYTES: usize = super::CHUNK_BYTES + 64 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const ADMIN_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const READER_PAIRING_LIFETIME: Duration = Duration::from_secs(10 * 60);
const PAIR_ATTEMPTS_PER_MINUTE: usize = 10;

/// `hello` で知らせる機能の一覧。
const CAPABILITIES_EMBEDDED: &[&str] = &["read", "reading-sync"];
const CAPABILITIES_STANDALONE: &[&str] = &["read", "reading-sync", "admin-sync"];

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
    pair_attempts: Mutex<VecDeque<Instant>>,
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
            pair_attempts: Mutex::new(VecDeque::new()),
        })
    }

    pub fn set_update(&self, controller: Arc<UpdateController>) {
        let _ = self.update.set(controller);
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
            "本棚を準備しています",
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
        .route("/api/v1/works/search", post(search))
        .route("/api/v1/works/{id}", get(metadata))
        .route("/api/v1/works/{id}/pages/{page}", get(page))
        .route("/api/v1/works/{id}/outline", get(outline))
        .route("/api/v1/works/{id}/assets/{asset_id}", get(asset))
        .route("/api/v1/works/{id}/cover", get(cover))
        .route("/api/v1/works/{id}/collections", get(work_collections))
        .route("/api/v1/entities/{kind}", get(entities))
        .route("/api/v1/entities/{kind}/cover", get(entity_cover))
        .route("/api/v1/collections", get(collections))
        .route("/api/v1/collections/{id}", get(collection))
        .route(
            "/api/v1/collections/{id}/cover/{slot}",
            get(collection_cover),
        )
        .route("/api/v1/reading", get(reading_since))
        .route("/api/v1/reading/{source}/{source_id}", put(reading_put))
        .layer(DefaultBodyLimit::max(MAX_JSON_BYTES))
        .layer(middleware::from_fn(read_timeout))
        .with_state(ctx)
}

fn admin_routes(ctx: Ctx) -> Router {
    Router::new()
        .route("/api/v1/admin/status", get(admin_status))
        .route("/api/v1/admin/update", post(admin_update))
        .route("/api/v1/admin/pairings", post(admin_pairing))
        .route("/api/v1/admin/devices", get(admin_devices))
        .route("/api/v1/admin/devices/{id}", delete(admin_revoke))
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
        .with_state(ctx)
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

fn finish(router: Router) -> Router {
    router
        .fallback(|| async { ApiError::not_found() })
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
        Ok(paired) => Ok(Json(serde_json::json!({
            "deviceId": paired.device_id,
            "token": paired.token,
            "role": paired.role,
            "apiVersion": API_VERSION,
        }))),
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
                Some("work_count"),
                Some("desc"),
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
}

async fn entity_cover(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath(kind): UrlPath<String>,
    Query(query): Query<EntityCoverQuery>,
) -> Result<Response, ApiError> {
    require_device(&ctx, &headers)?;
    if !matches!(query.source.as_str(), "pixiv" | "fanbox")
        || query.key.is_empty()
        || query.key.chars().count() > 200
    {
        return Err(ApiError::not_found());
    }
    let root = ctx.root.clone();
    let (bytes, mime) = with_db(&ctx, move |db| {
        let (path, area) = match kind.as_str() {
            "people" => {
                let person = db
                    .get_person(&query.source, &query.key)
                    .map_err(|_| ApiError::not_found())?;
                (person.icon_path.or(person.cover_path), "profiles")
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
        let path = inside_roots(&path, &[root.join(area), root.join("downloads")])?;
        cover_bytes(&path)
    })
    .await?;
    image_response(bytes, &mime)
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
    let (bytes, mime) = with_db(&ctx, move |db| {
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
        let path = inside_roots(&path, &[root.join(area)])?;
        cover_bytes(&path)
    })
    .await?;
    image_response(bytes, &mime)
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
    let result = with_db(&ctx, move |db| {
        let render = || -> Result<MobilePage, String> {
            let assets = db.get_assets(id)?;
            let content = db.get_reader_content_page(id, query.version, page, false)?;
            mobile_page(content, &assets)
        };
        render().map_err(|error| {
            log::warn!("本文を渡せません: {error}");
            ApiError::internal()
        })
    })
    .await?;
    Ok(Json(result))
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

async fn asset(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath((id, asset_id)): UrlPath<(i64, i64)>,
) -> Result<Response, ApiError> {
    require_device(&ctx, &headers)?;
    if id <= 0 || asset_id <= 0 {
        return Err(ApiError::not_found());
    }
    let root = ctx.root.clone();
    let (bytes, mime) = with_db(&ctx, move |db| {
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
        let size = fs::metadata(&path)
            .map_err(|_| ApiError::not_found())?
            .len();
        asset_bytes(&path, mime, size)
    })
    .await?;
    image_response(bytes, &mime)
}

async fn cover(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<i64>,
) -> Result<Response, ApiError> {
    require_device(&ctx, &headers)?;
    if id <= 0 {
        return Err(ApiError::not_found());
    }
    let root = ctx.root.clone();
    let (bytes, mime) = with_db(&ctx, move |db| {
        let work = db.get_download(id).map_err(|_| ApiError::not_found())?;
        let path = work.cover_path.ok_or_else(ApiError::not_found)?;
        let path = inside_roots(&path, &[root.join("downloads")])?;
        cover_bytes(&path)
    })
    .await?;
    image_response(bytes, &mime)
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

async fn reading_put(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath((source, source_id)): UrlPath<(String, String)>,
    ApiJson(update): ApiJson<ReadingUpdate>,
) -> Result<Json<super::store::ReadingPosition>, ApiError> {
    let device = require_device(&ctx, &headers)?;
    if !matches!(source.as_str(), "pixiv" | "fanbox")
        || source_id.is_empty()
        || source_id.chars().count() > 200
    {
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
    }))
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
}

async fn admin_pairing(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<PairingRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&ctx, &headers)?;
    let url = ctx
        .public_url
        .clone()
        .or(request.url)
        .filter(|url| url.len() <= 512 && url.starts_with("https://"))
        .ok_or_else(|| ApiError::invalid("携帯が接続する https の URL が決まっていません"))?;
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
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Result<StatusCode, ApiError> {
    require_admin(&ctx, &headers)?;
    if ctx
        .store
        .revoke_device(&id)
        .map_err(|_| ApiError::internal())?
    {
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
    Ok(Json(
        serde_json::to_value(outcome).map_err(|_| ApiError::internal())?,
    ))
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

fn image_response(bytes: Vec<u8>, mime: &str) -> Result<Response, ApiError> {
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(mime).map_err(|_| ApiError::internal())?,
    );
    Ok(response)
}

fn cover_bytes(path: &Path) -> Result<(Vec<u8>, String), ApiError> {
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
        max_width: Some(320),
        max_height: Some(480),
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

fn asset_bytes(path: &Path, mime: &str, size: u64) -> Result<(Vec<u8>, String), ApiError> {
    if size <= MAX_ASSET_BYTES {
        return fs::read(path)
            .map(|bytes| (bytes, mime.to_string()))
            .map_err(|_| ApiError::not_found());
    }
    // 高解像度の挿絵をそのまま WebView に渡すと、転送量と展開後のメモリが大きい。
    // 元ファイルは変更せず、端末向けの縮小版だけを応答として生成する。
    if size > MAX_TRANSCODE_SOURCE_BYTES
        || !matches!(mime, "image/jpeg" | "image/png" | "image/webp")
    {
        return Err(ApiError::asset_too_large());
    }
    let (width, height) = image::image_dimensions(path).map_err(|_| ApiError::not_found())?;
    if u64::from(width) * u64::from(height) > MAX_TRANSCODE_PIXELS {
        return Err(ApiError::asset_too_large());
    }
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
