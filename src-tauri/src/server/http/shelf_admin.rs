//! 管理の口：棚の写しの受け取りと、携帯が棚を読む許可（単独サーバーだけ）。

use super::{require_admin, ApiError, ApiJson, Ctx};
use crate::library::{ContentFile, LibraryId};
use crate::server::standalone::shipment::{self, ShelfShipments, ShipError, WireChunk};
use axum::body::Bytes;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::Deserialize;
use std::sync::Arc;

impl From<ShipError> for ApiError {
    fn from(error: ShipError) -> Self {
        match error {
            ShipError::Invalid(message) => ApiError::invalid(message),
            ShipError::Conflict(message) => ApiError(StatusCode::CONFLICT, "conflict", message),
            ShipError::NotFound => ApiError::not_found(),
            ShipError::Internal(detail) => {
                log::error!("棚の受け取りに失敗しました: {detail}");
                ApiError::internal()
            }
        }
    }
}

fn shipments(ctx: &Ctx) -> Result<Arc<ShelfShipments>, ApiError> {
    ctx.shipments.get().cloned().ok_or_else(ApiError::not_found)
}

async fn run<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ShipError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(ApiError::from)
}

fn json<T: serde::Serialize>(value: &T) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(value).map_err(|_| ApiError::internal())?,
    ))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OpenRequest {
    library_id: LibraryId,
}

/// 受け取りを始める（同じ棚の受け取りが残っていれば続ける）。
pub(super) async fn open(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<OpenRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&ctx, &headers)?;
    let shipments = shipments(&ctx)?;
    let opened = run(move || shipments.open(request.library_id)).await?;
    json(&opened)
}

/// 受け取りの進み具合。
pub(super) async fn status(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&ctx, &headers)?;
    json(&shipments(&ctx)?.status())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NeededRequest {
    chunks: Vec<WireChunk>,
}

/// 送ってほしい実体を答える。
pub(super) async fn needed(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    ApiJson(request): ApiJson<NeededRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    require_admin(&ctx, &headers)?;
    let shipments = shipments(&ctx)?;
    let needed = run(move || shipments.needed(&id, &request.chunks)).await?;
    Ok(Json(serde_json::json!({ "needed": needed })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChunkQuery {
    length: u64,
}

/// 実体を一つ受け取る（圧縮したまま）。
pub(super) async fn put_chunk(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath((id, chunk)): UrlPath<(String, String)>,
    Query(query): Query<ChunkQuery>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    require_admin(&ctx, &headers)?;
    let shipments = shipments(&ctx)?;
    let wire = WireChunk {
        id: chunk,
        length: query.length,
    };
    run(move || shipments.put_chunk(&id, &wire, &body)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CommitRequest {
    /// 台帳の写しの部分の並び（部分は実体と同じ口で先に送る）。
    database: ContentFile,
}

/// 受け取りを確定する。棚は別のスレッドで組み、進み具合は `status` で返す。
pub(super) async fn commit(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
    ApiJson(request): ApiJson<CommitRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    require_admin(&ctx, &headers)?;
    let shipments = shipments(&ctx)?;
    let status = run(move || shipments.commit(&id, &request.database)).await?;
    Ok((StatusCode::ACCEPTED, json(&status)?))
}

/// 受け取りをやめる。
pub(super) async fn abort(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath(id): UrlPath<String>,
) -> Result<StatusCode, ApiError> {
    require_admin(&ctx, &headers)?;
    let shipments = shipments(&ctx)?;
    run(move || shipments.abort(&id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReaderRequest {
    read: bool,
}

/// 解除した端末の、棚を読む許可も消す（登録はもう使えないので、残っても読めはしない）。
pub(super) async fn forget_reader(ctx: &Ctx, device: String) {
    let (Some(shelf), Some(paths)) = (ctx.reader_shelf.get().cloned(), ctx.paths.get().cloned())
    else {
        return;
    };
    if !shelf.reads(&device) {
        return;
    }
    let forgotten = tokio::task::spawn_blocking(move || {
        shipment::set_reader(&paths, &device, false)?;
        shelf.allow_reader(&device, None)
    })
    .await;
    if !matches!(forgotten, Ok(Ok(()))) {
        log::warn!("解除した端末の、棚を読む許可を消せません");
    }
}

/// 携帯に棚を読む許可を付け外しする。設定に書き、配っている棚にもすぐ効かせる。
pub(super) async fn set_reader(
    State(ctx): State<Ctx>,
    headers: HeaderMap,
    UrlPath(device): UrlPath<String>,
    ApiJson(request): ApiJson<ReaderRequest>,
) -> Result<StatusCode, ApiError> {
    require_admin(&ctx, &headers)?;
    let shelf = ctx.reader_shelf.get().cloned().ok_or(ApiError(
        StatusCode::CONFLICT,
        "no_shelf",
        "このサーバーはまだ棚を配っていません",
    ))?;
    let paths = ctx.paths.get().cloned().ok_or_else(ApiError::not_found)?;
    let store = Arc::clone(&ctx.store);
    tokio::task::spawn_blocking(move || {
        let reader = store
            .list_devices()
            .map_err(|_| ApiError::internal())?
            .into_iter()
            .find(|summary| summary.id == device)
            .ok_or_else(ApiError::not_found)?;
        if reader.role != crate::server::store::Role::Reader {
            return Err(ApiError::invalid("携帯の登録だけに許可を付けられます"));
        }
        let owner = shipment::set_reader(&paths, &device, request.read).map_err(|error| {
            log::error!("棚を読む許可を書けません: {error}");
            ApiError::internal()
        })?;
        shelf
            .allow_reader(&device, request.read.then_some(owner))
            .map_err(|error| {
                log::error!("棚を読む許可を当てられません: {error}");
                ApiError::internal()
            })?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
    .map_err(|_| ApiError::internal())?
}
