//! 新ライブラリ用の認証付き読取 API。旧 API の意味や本番の保存先を変更しない。
mod auth;
pub use auth::{Authority, DeviceAuthority};

use crate::library::{
    protocol::{self, Decimal, Hash, Id},
    Library, ResourceReference, ResourceScope,
};
use axum::{
    body::Body,
    extract::{Path, Query, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Extension, Json, Router,
};
use headers::{authorization::Bearer, Authorization, ETag, HeaderMapExt, IfNoneMatch};
use serde::Deserialize;
use std::sync::Arc;

pub struct Context {
    library: Arc<Library>,
    library_id: Id,
    authority: Arc<dyn Authority>,
    sync_epoch: Id,
    readers: Arc<tokio::sync::Semaphore>,
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
            readers: Arc::new(tokio::sync::Semaphore::new(4)),
        })
    }
}
#[derive(Clone)]
struct Principal {
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
    let subject = ctx
        .authority
        .subject(token.token(), ctx.library_id)
        .ok_or_else(denied)?;
    if request.headers().contains_key(header::ORIGIN) {
        return Err(ApiError(StatusCode::FORBIDDEN, "origin_forbidden"));
    }
    if request.uri().to_string().len() > 32768 {
        return Err(ApiError(StatusCode::URI_TOO_LONG, "uri_too_long"));
    }
    request.extensions_mut().insert(Principal { subject });
    let mut response = next.run(request).await;
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

fn read_permit(ctx: &Context) -> Result<tokio::sync::OwnedSemaphorePermit, ApiError> {
    ctx.readers
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::TOO_MANY_REQUESTS, "busy"))
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
        if !matches!(info.role.as_str(), "attachment" | "document") {
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
fn representation(
    bytes: Vec<u8>,
    mime: &'static str,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let etag_value = format!("\"sha256-{}\"", crate::library::Fingerprint::of(&bytes));
    let etag: ETag = etag_value.parse().map_err(|_| internal())?;
    let conditional = headers
        .typed_try_get::<IfNoneMatch>()
        .map_err(|_| invalid())?;
    let not_modified = conditional.is_some_and(|condition| !condition.precondition_passes(&etag));
    let mut response = if not_modified {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        ([(header::CONTENT_TYPE, mime)], Body::from(bytes)).into_response()
    };
    response.headers_mut().typed_insert(etag);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, no-cache, no-transform"),
    );
    response.headers_mut().insert(
        header::VARY,
        header::HeaderValue::from_static("Authorization"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}
async fn manifest(
    State(ctx): State<Arc<Context>>,
    path: Result<Path<Location>, axum::extract::rejection::PathRejection>,
    query: Result<Query<ResourceQuery>, axum::extract::rejection::QueryRejection>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let Path(location) = path.map_err(|_| invalid())?;
    let Query(query) = query.map_err(|_| invalid())?;
    let (entity, scope, reference) = target(&ctx, location, query)?;
    let permit = read_permit(&ctx)?;
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
    representation(
        bytes,
        "application/vnd.piep.resource-transfer+json",
        &headers,
    )
}
async fn chunk(
    State(ctx): State<Arc<Context>>,
    path: Result<Path<ChunkLocation>, axum::extract::rejection::PathRejection>,
    query: Result<Query<ResourceQuery>, axum::extract::rejection::QueryRejection>,
    headers: HeaderMap,
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
    let permit = read_permit(&ctx)?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        reader_resource(&ctx, entity, scope, &reference)?;
        // 順番や ETag の照合より前に、要求した保存状態・資産・file の所属を確認する。
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
    representation(bytes, "application/vnd.piep.object-v1", &headers)
}

#[cfg(test)]
mod tests;
