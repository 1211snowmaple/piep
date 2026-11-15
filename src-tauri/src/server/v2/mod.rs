//! 新ライブラリ用の認証付き読取 API。旧 API の意味や本番の保存先を変更しない。
mod auth;
pub use auth::{Authority, DeviceAuthority, Grant};

use crate::library::{
    protocol::{self, Decimal, Hash, Id},
    Library, ResourceReference, ResourceScope,
};
use axum::{
    body::Body,
    extract::{Path, Query, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Extension, Json, Router,
};
use headers::{authorization::Bearer, Authorization, HeaderMapExt};
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

/// 全端末あわせて同時に読める数。
const READERS: usize = 8;
/// 一台が同時に読める数（一台が全体を占めないように）。
const READERS_PER_DEVICE: usize = 4;
/// 空きを待つ長さ。越えたら 429 と `Retry-After` を返す。
const READ_WAIT: Duration = Duration::from_secs(2);

pub struct Context {
    library: Arc<Library>,
    library_id: Id,
    authority: Arc<dyn Authority>,
    sync_epoch: Id,
    readers: Arc<tokio::sync::Semaphore>,
    devices: Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>,
}
impl Context {
    /// sync_epoch は管理側の永続状態から渡し、棚の復元・再作成時には更新する。
    /// router や要求ごとに新しい epoch を生成しない。
    pub fn new(
        library: Arc<Library>,
        authority: Arc<dyn Authority>,
        sync_epoch: Id,
    ) -> protocol::Result<Self> {
        Ok(Self {
            library_id: Id::new(library.id)?,
            library,
            authority,
            sync_epoch,
            readers: Arc::new(tokio::sync::Semaphore::new(READERS)),
            devices: Mutex::new(HashMap::new()),
        })
    }
}
#[derive(Clone)]
struct Principal {
    device: String,
    subject: Id,
}

#[derive(Debug)]
pub struct ApiError(StatusCode, &'static str);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.0, Json(serde_json::json!({"error":self.1}))).into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-store"),
        );
        if self.0 == StatusCode::TOO_MANY_REQUESTS {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, header::HeaderValue::from_static("1"));
        }
        response
    }
}
fn denied() -> ApiError {
    ApiError(StatusCode::UNAUTHORIZED, "unauthorized")
}
fn invalid() -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, "invalid_request")
}
fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "resource_unavailable")
}
fn internal() -> ApiError {
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, "storage_error")
}
fn library_error(error: crate::library::Error) -> ApiError {
    match error {
        crate::library::Error::ResourceUnavailable => not_found(),
        crate::library::Error::InvalidRange => {
            ApiError(StatusCode::RANGE_NOT_SATISFIABLE, "invalid_range")
        }
        _ => internal(),
    }
}
async fn authorize(
    State(ctx): State<Arc<Context>>,
    mut request: Request,
    next: Next,
) -> Result<Response, ApiError> {
    // Cookie や query の鍵は受け入れない。権限の解除は各要求で再確認する。
    if request
        .headers()
        .get_all(header::AUTHORIZATION)
        .iter()
        .count()
        != 1
    {
        return Err(denied());
    }
    let token = request
        .headers()
        .typed_try_get::<Authorization<Bearer>>()
        .map_err(|_| denied())?
        .ok_or_else(denied)?;
    let Grant { device, subject } = ctx
        .authority
        .grant(token.token(), ctx.library_id)
        .ok_or_else(denied)?;
    if request.headers().contains_key(header::ORIGIN) {
        return Err(ApiError(StatusCode::FORBIDDEN, "origin_forbidden"));
    }
    if request.uri().to_string().len() > 32768 {
        return Err(ApiError(StatusCode::URI_TOO_LONG, "uri_too_long"));
    }
    request
        .extensions_mut()
        .insert(Principal { device, subject });
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        "x-piep-subject-id",
        subject.get().to_string().parse().map_err(|_| internal())?,
    );
    response.headers_mut().insert(
        "x-piep-library-id",
        ctx.library_id
            .get()
            .to_string()
            .parse()
            .map_err(|_| internal())?,
    );
    response.headers_mut().insert(
        "x-piep-sync-epoch",
        ctx.sync_epoch
            .get()
            .to_string()
            .parse()
            .map_err(|_| internal())?,
    );
    Ok(response)
}

/// 読む許可。端末ごとの枠と全体の枠の両方を取る。空きがなければ少し待ち、それでも
/// なければ 429 で断る（`Retry-After` を付ける）。
struct ReadPermit {
    _device: tokio::sync::OwnedSemaphorePermit,
    _all: tokio::sync::OwnedSemaphorePermit,
}

async fn read_permit(ctx: &Context, device: &str) -> Result<ReadPermit, ApiError> {
    let busy = || ApiError(StatusCode::TOO_MANY_REQUESTS, "busy");
    let own = {
        let mut devices = ctx.devices.lock().map_err(|_| internal())?;
        // 使われていない端末の枠は捨てる（登録を消した端末が残り続けないように）。
        devices.retain(|_, semaphore| {
            Arc::strong_count(semaphore) > 1 || semaphore.available_permits() < READERS_PER_DEVICE
        });
        devices
            .entry(device.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(READERS_PER_DEVICE)))
            .clone()
    };
    let deadline = tokio::time::Instant::now() + READ_WAIT;
    let device = tokio::time::timeout_at(deadline, own.acquire_owned())
        .await
        .map_err(|_| busy())?
        .map_err(|_| internal())?;
    let all = tokio::time::timeout_at(deadline, ctx.readers.clone().acquire_owned())
        .await
        .map_err(|_| busy())?
        .map_err(|_| internal())?;
    Ok(ReadPermit {
        _device: device,
        _all: all,
    })
}

fn reader_resource(
    ctx: &Context,
    entity: uuid::Uuid,
    scope: ResourceScope,
    reference: &ResourceReference,
) -> Result<(), ApiError> {
    // 原応答や旧ファイルの書庫は読書用 API へ出さない。元データは保管層で保持する。
    // 旧原応答にはローカルパス等の内部情報を含むものがあるため、別の管理契約が必要。
    if matches!(reference, ResourceReference::Original { .. }) {
        return Err(ApiError(StatusCode::FORBIDDEN, "archive_forbidden"));
    }
    if matches!(reference, ResourceReference::File { .. }) {
        let info = ctx
            .library
            .resource_info(entity, scope, reference)
            .map_err(library_error)?;
        if !crate::library::reader_may_read(&info) {
            return Err(ApiError(StatusCode::FORBIDDEN, "archive_forbidden"));
        }
    }
    Ok(())
}

/// 新しい棚と明示的な権限設定があるときだけ既存 router に merge する。
/// この関数だけで旧ライブラリを開き直したり、本番を切り替えたりしない。
pub fn router(ctx: Arc<Context>) -> Router {
    Router::new()
        .route("/api/v2/identity",get(identity))
        .route("/api/v2/libraries/{library}/entities/{entity}/views/{view_kind}/{view_id}/bundle",get(bundle))
        .route("/api/v2/libraries/{library}/entities/{entity}/views/{view_kind}/{view_id}/resources/{resource_kind}/manifest",get(manifest))
        .route("/api/v2/libraries/{library}/entities/{entity}/views/{view_kind}/{view_id}/resources/{resource_kind}/files/{file}/chunks/{ordinal}",get(chunk))
        .route_layer(middleware::from_fn_with_state(ctx.clone(),authorize))
        .with_state(ctx)
}
async fn identity(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
) -> Response {
    let mut response = Json(protocol::LibraryIdentity::new(
        ctx.library_id,
        principal.subject,
        ctx.sync_epoch,
    ))
    .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}
#[derive(Deserialize)]
struct Location {
    library: Id,
    entity: Id,
    view_kind: String,
    view_id: Id,
    resource_kind: String,
}
#[derive(Deserialize)]
struct ViewLocation {
    library: Id,
    entity: Id,
    view_kind: String,
    view_id: Id,
}
#[derive(Deserialize)]
struct ChunkLocation {
    library: Id,
    entity: Id,
    view_kind: String,
    view_id: Id,
    resource_kind: String,
    file: Hash,
    ordinal: Decimal,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceQuery {
    slot: Option<String>,
    observation: Option<Id>,
}

fn target(
    ctx: &Context,
    location: Location,
    query: ResourceQuery,
) -> Result<(uuid::Uuid, ResourceScope, ResourceReference), ApiError> {
    if location.library != ctx.library_id {
        return Err(not_found());
    }
    let scope = match location.view_kind.as_str() {
        "snapshot" => ResourceScope::Snapshot(location.view_id.get()),
        "edit" => ResourceScope::Edit(location.view_id.get()),
        _ => return Err(invalid()),
    };
    let reference = match (
        location.resource_kind.as_str(),
        query.slot,
        query.observation,
    ) {
        ("body", None, None) => ResourceReference::Body,
        ("media", Some(slot), None) if !slot.is_empty() && slot.len() <= 4096 => {
            ResourceReference::Media { slot }
        }
        ("file", Some(slot), None) if !slot.is_empty() && slot.len() <= 4096 => {
            ResourceReference::File { slot }
        }
        ("original", None, Some(observation)) => ResourceReference::Original {
            observation: observation.get(),
        },
        _ => return Err(invalid()),
    };
    Ok((location.entity.get(), scope, reference))
}
/// 中身のハッシュで指定して取るので、取り直しの確認（ETag・304）は持たない。部分実体は
/// 同じ URL の中身が変わらないので長く持ってよい。記録（manifest・bundle）は毎回確かめる。
fn representation(bytes: Vec<u8>, mime: &'static str, immutable: bool) -> Response {
    let mut response = ([(header::CONTENT_TYPE, mime)], Body::from(bytes)).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static(if immutable {
            "private, max-age=31536000, immutable, no-transform"
        } else {
            "private, no-cache, no-transform"
        }),
    );
    response.headers_mut().insert(
        header::VARY,
        header::HeaderValue::from_static("Authorization"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    response
}
async fn bundle(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: Result<Path<ViewLocation>, axum::extract::rejection::PathRejection>,
    query: Result<Query<EmptyQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, ApiError> {
    let Path(location) = path.map_err(|_| invalid())?;
    query.map_err(|_| invalid())?;
    if location.library != ctx.library_id {
        return Err(not_found());
    }
    let scope = match location.view_kind.as_str() {
        "snapshot" => ResourceScope::Snapshot(location.view_id.get()),
        "edit" => ResourceScope::Edit(location.view_id.get()),
        _ => return Err(invalid()),
    };
    let entity = location.entity.get();
    let permit = read_permit(&ctx, &principal.device).await?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        ctx.library
            .reader_bundle(entity, scope)
            .map_err(library_error)?
            .encode()
            .map_err(|_| internal())
    })
    .await
    .map_err(|_| internal())??;
    Ok(representation(
        bytes,
        "application/vnd.piep.reader-bundle+json",
        false,
    ))
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyQuery {}
async fn manifest(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: Result<Path<Location>, axum::extract::rejection::PathRejection>,
    query: Result<Query<ResourceQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, ApiError> {
    let Path(location) = path.map_err(|_| invalid())?;
    let Query(query) = query.map_err(|_| invalid())?;
    let (entity, scope, reference) = target(&ctx, location, query)?;
    let permit = read_permit(&ctx, &principal.device).await?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        reader_resource(&ctx, entity, scope, &reference)?;
        ctx.library
            .resource_transfer_manifest(entity, scope, &reference)
            .map_err(library_error)?
            .encode()
            .map_err(|_| internal())
    })
    .await
    .map_err(|_| internal())??;
    Ok(representation(
        bytes,
        "application/vnd.piep.resource-transfer+json",
        false,
    ))
}
async fn chunk(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: Result<Path<ChunkLocation>, axum::extract::rejection::PathRejection>,
    query: Result<Query<ResourceQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, ApiError> {
    let Path(location) = path.map_err(|_| invalid())?;
    let Query(query) = query.map_err(|_| invalid())?;
    let (entity, scope, reference) = target(
        &ctx,
        Location {
            library: location.library,
            entity: location.entity,
            view_kind: location.view_kind,
            view_id: location.view_id,
            resource_kind: location.resource_kind,
        },
        query,
    )?;
    let permit = read_permit(&ctx, &principal.device).await?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        reader_resource(&ctx, entity, scope, &reference)?;
        // 順番より前に、要求した保存状態・資産・file の所属を確認する。
        let owned = ctx
            .library
            .resource_transfer_manifest(entity, scope, &reference)
            .map_err(library_error)?;
        if owned.file.id != location.file {
            return Err(not_found());
        }
        let ordinal = usize::try_from(location.ordinal.get())
            .map_err(|_| ApiError(StatusCode::RANGE_NOT_SATISFIABLE, "invalid_range"))?;
        let mut bytes = Vec::new();
        ctx.library
            .export_resource_chunk(
                entity,
                scope,
                &reference,
                location.file.get(),
                ordinal,
                &mut bytes,
            )
            .map_err(library_error)?;
        Ok(bytes)
    })
    .await
    .map_err(|_| internal())??;
    Ok(representation(
        bytes,
        "application/vnd.piep.object-v1",
        true,
    ))
}

#[cfg(test)]
mod tests;
