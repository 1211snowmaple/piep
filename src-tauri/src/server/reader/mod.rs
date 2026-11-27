//! 携帯が棚を読む認証付きの API（`/api/identity`・`/api/libraries/...`）。
mod auth;
mod favorite;
mod groups;
mod list;
mod reading;
pub use auth::{Authority, DeviceAuthority, Grant};

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
    routing::{get, post, put},
    Extension, Json, Router,
};
use headers::{authorization::Bearer, Authorization, HeaderMapExt};
use piep_mobile_api::ReaderErrorCode as ErrorCode;
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

/// 全端末あわせて同時に読める数。
const READERS: usize = 8;
/// 一台が同時に読める数（一台が全体を占めないように）。
const READERS_PER_DEVICE: usize = 4;
/// 空きを待つ長さ。越えたら 429 と `Retry-After` を返す。
const READ_WAIT: Duration = Duration::from_secs(2);

#[cfg(test)]
std::thread_local! {
    pub(super) static SEARCH_START_GATE: std::cell::Cell<Option<std::sync::mpsc::Receiver<()>>> =
        const { std::cell::Cell::new(None) };
}

pub struct Context {
    library: Arc<Library>,
    library_id: Id,
    authority: Arc<dyn Authority>,
    sync_epoch: Id,
    readers: Arc<tokio::sync::Semaphore>,
    devices: Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>,
    /// 本文の検索の索引が棚に追いついたか。追いつくまで語の検索には答えない。
    search_ready: AtomicBool,
    covers: Mutex<list::Covers>,
    /// 作者・シリーズの一覧の集計（棚の世代と条件ごと）。
    entity_rows: Mutex<groups::EntityRows>,
    /// 検索欄の候補の、正規化で足す名前の束（棚の世代ごと）。
    suggestions: crate::library_suggest::SuggestionPool,
    /// 読書位置のページ番号を確かめるための、読む画面のページ数。
    page_counts: crate::library_view::ReaderPageCounts,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ShelfStatus {
    library_id: Id,
    sync_epoch: Id,
    works: Decimal,
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
            search_ready: AtomicBool::new(false),
            covers: Mutex::new(list::Covers::default()),
            entity_rows: Mutex::new(groups::EntityRows::default()),
            suggestions: Default::default(),
            page_counts: Default::default(),
        })
    }

    /// 本文の検索の索引を棚に合わせる（初めては数分かかる）。済むまで語の検索は 503 で断り、
    /// 絞り込みだけの一覧は先に答える。配信する棚の写しの索引は、写しの `derived/` に作る。
    pub fn prepare_search(self: &Arc<Self>) {
        let ctx = Arc::clone(self);
        #[cfg(test)]
        let gate = SEARCH_START_GATE.take();
        let spawned = std::thread::Builder::new()
            .name("piep-reader-search-index".into())
            .spawn(move || {
                #[cfg(test)]
                if let Some(gate) = gate {
                    let _ = gate.recv();
                }
                match ctx.library.sync_search_index(&mut |_, _| {}) {
                    Ok(_) => ctx.search_ready.store(true, Ordering::Release),
                    Err(error) => log::error!("棚の検索の索引を作れません: {error}"),
                }
            });
        if let Err(error) = spawned {
            log::error!("棚の検索の索引を作るスレッドを作れません: {error}");
        }
    }

    /// 配っている棚（受け取った写しと実体を共有するため）。
    pub(crate) fn library(&self) -> &Arc<Library> {
        &self.library
    }

    /// 端末がこの棚を読めるか。
    pub(crate) fn reads(&self, device: &str) -> bool {
        self.authority.subject_of(device, self.library_id).is_some()
    }

    /// 端末に棚を読む許可を付け外しする（読む利用者は `subject`、`None` で外す）。
    pub(crate) fn allow_reader(&self, device: &str, subject: Option<Id>) -> Result<(), String> {
        self.authority.allow(device, self.library_id, subject)
    }

    pub(super) fn shelf_status(&self) -> crate::library::Result<ShelfStatus> {
        let conn = self.library.reader()?;
        let works: i64 =
            conn.query_row("SELECT count(*) FROM library_items", [], |row| row.get(0))?;
        Ok(ShelfStatus {
            library_id: self.library_id,
            sync_epoch: self.sync_epoch,
            works: Decimal::new(works as u64),
        })
    }
}
#[derive(Clone)]
struct Principal {
    device: String,
    subject: Id,
}

#[derive(Debug)]
pub struct ApiError(StatusCode, ErrorCode);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response =
            (self.0, Json(piep_mobile_api::ReaderError { code: self.1 })).into_response();
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
    ApiError(StatusCode::UNAUTHORIZED, ErrorCode::Unauthorized)
}
fn invalid() -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, ErrorCode::InvalidRequest)
}
fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, ErrorCode::ResourceUnavailable)
}
fn internal() -> ApiError {
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::StorageError)
}

fn scope_header_is(headers: &HeaderMap, name: &str, id: Id) -> bool {
    let values = headers.get_all(name);
    let mut values = values.iter();
    values
        .next()
        .is_some_and(|value| value == id.get().to_string().as_str())
        && values.next().is_none()
}

/// 書き込み先を、端末が開いていた棚の利用者と同期世代に限る。
fn check_write_scope(
    headers: &HeaderMap,
    principal: &Principal,
    ctx: &Context,
) -> Result<(), ApiError> {
    if !scope_header_is(headers, "x-piep-subject-id", principal.subject)
        || !scope_header_is(headers, "x-piep-sync-epoch", ctx.sync_epoch)
    {
        return Err(ApiError(StatusCode::CONFLICT, ErrorCode::LibraryChanged));
    }
    Ok(())
}

fn library_error(error: crate::library::Error) -> ApiError {
    match error {
        crate::library::Error::ResourceUnavailable => not_found(),
        crate::library::Error::InvalidRange => {
            ApiError(StatusCode::RANGE_NOT_SATISFIABLE, ErrorCode::InvalidRange)
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
    let authority = Arc::clone(&ctx.authority);
    let library_id = ctx.library_id;
    let Grant { device, subject } =
        tokio::task::spawn_blocking(move || authority.grant(token.token(), library_id))
            .await
            .map_err(|_| internal())?
            .ok_or_else(denied)?;
    if request.headers().contains_key(header::ORIGIN) {
        return Err(ApiError(StatusCode::FORBIDDEN, ErrorCode::OriginForbidden));
    }
    if request.uri().to_string().len() > 32768 {
        return Err(ApiError(StatusCode::URI_TOO_LONG, ErrorCode::UriTooLong));
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
    let busy = || ApiError(StatusCode::TOO_MANY_REQUESTS, ErrorCode::Busy);
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
        return Err(ApiError(StatusCode::FORBIDDEN, ErrorCode::ArchiveForbidden));
    }
    if matches!(reference, ResourceReference::File { .. }) {
        let info = ctx
            .library
            .resource_info(entity, scope, reference)
            .map_err(library_error)?;
        if !crate::library::reader_may_read(&info) {
            return Err(ApiError(StatusCode::FORBIDDEN, ErrorCode::ArchiveForbidden));
        }
    }
    Ok(())
}

fn authorize_reader_view(
    ctx: &Context,
    subject: Id,
    entity: uuid::Uuid,
    scope: ResourceScope,
) -> Result<(), ApiError> {
    // 棚から外しても保存版・編集・資産は台帳に残る。配る前に掲載を毎回確かめる。
    if ctx
        .library
        .library_item_generation(entity)
        .map_err(library_error)?
        .is_none()
    {
        return Err(not_found());
    }
    let ResourceScope::Edit(edit) = scope else {
        return Ok(());
    };
    if !ctx
        .library
        .reader_may_read_edit(entity, subject.get(), edit)
        .map_err(library_error)?
    {
        return Err(not_found());
    }
    Ok(())
}

/// 棚と明示的な権限設定があるときだけ既存 router に merge する。
/// この関数だけで旧ライブラリを開き直したり、本番を切り替えたりしない。
pub fn router(ctx: Arc<Context>) -> Router {
    Router::new()
        .route("/api/identity",get(identity))
        .route("/api/libraries/{library}/works",get(works))
        .route("/api/libraries/{library}/works/lookup",get(work_lookup))
        .route("/api/libraries/{library}/works/search",post(list::search))
        .route("/api/libraries/{library}/works/suggest",get(list::suggest))
        .route("/api/libraries/{library}/works/{work}",get(list::work))
        .route("/api/libraries/{library}/works/{work}/cover",get(list::cover))
        .route("/api/libraries/{library}/works/{work}/favorite",put(favorite::put))
        .route("/api/libraries/{library}/works/{work}/reading",put(reading::put))
        .route("/api/libraries/{library}/reading",get(reading::list))
        .route("/api/libraries/{library}/works/{work}/collections",get(groups::work_collections))
        .route("/api/libraries/{library}/people",get(groups::people))
        .route("/api/libraries/{library}/people/detail",get(groups::person_detail))
        .route("/api/libraries/{library}/people/series",get(groups::person_series))
        .route("/api/libraries/{library}/people/cover",get(groups::person_cover))
        .route("/api/libraries/{library}/series",get(groups::series))
        .route("/api/libraries/{library}/series/detail",get(groups::series_detail))
        .route("/api/libraries/{library}/series/cover",get(groups::series_cover))
        .route("/api/libraries/{library}/collections",get(groups::collections))
        .route("/api/libraries/{library}/collections/{collection}",get(groups::collection))
        .route("/api/libraries/{library}/collections/{collection}/cover/{slot}",get(groups::collection_cover))
        .route("/api/libraries/{library}/entities/{entity}/views/{view_kind}/{view_id}/bundle",get(bundle))
        .route("/api/libraries/{library}/entities/{entity}/views/{view_kind}/{view_id}/resources/{resource_kind}/manifest",get(manifest))
        .route("/api/libraries/{library}/entities/{entity}/views/{view_kind}/{view_id}/resources/{resource_kind}/files/{file}/chunks/{ordinal}",get(chunk))
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
#[serde(deny_unknown_fields)]
struct WorksQuery {
    limit: Option<u8>,
    after: Option<String>,
}

fn work_page_boundary(value: &str, epoch: Id, subject: Id) -> Result<(i64, uuid::Uuid), ApiError> {
    let parts: Vec<_> = value.splitn(4, '.').collect();
    let [saved_epoch, saved_subject, time, work] = parts.as_slice() else {
        return Err(invalid());
    };
    let work_id = uuid::Uuid::parse_str(work).map_err(|_| invalid())?;
    let saved_at = time.parse::<i64>().map_err(|_| invalid())?;
    if work_id.is_nil() || work_id.to_string() != *work {
        return Err(invalid());
    }
    if epoch.get().to_string() != *saved_epoch || subject.get().to_string() != *saved_subject {
        return Err(ApiError(StatusCode::CONFLICT, ErrorCode::StaleCursor));
    }
    Ok((saved_at, work_id))
}

async fn works(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: Result<Path<Id>, axum::extract::rejection::PathRejection>,
    query: Result<Query<WorksQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, ApiError> {
    let Path(library) = path.map_err(|_| invalid())?;
    let Query(WorksQuery { limit, after }) = query.map_err(|_| invalid())?;
    if library != ctx.library_id {
        return Err(not_found());
    }
    let limit = limit.unwrap_or(30);
    if !(1..=50).contains(&limit) {
        return Err(invalid());
    }
    let subject = principal.subject;
    let epoch = ctx.sync_epoch;
    let after = after
        .as_deref()
        .map(|value| work_page_boundary(value, epoch, subject))
        .transpose()?;
    let permit = read_permit(&ctx, &principal.device).await?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut rows = ctx
            .library
            .reader_works(subject.get(), limit + 1, after)
            .map_err(library_error)?;
        let limit = usize::from(limit);
        let next = if rows.len() > limit {
            rows.truncate(limit);
            rows.last().map(|(work, saved_at)| {
                format!(
                    "{}.{}.{saved_at}.{}",
                    epoch.get(),
                    subject.get(),
                    work.view.entity_id.get()
                )
            })
        } else {
            None
        };
        let page = protocol::ReaderWorkPage {
            items: rows.into_iter().map(|(work, _)| work).collect(),
            next,
        };
        serde_json::to_vec(&page).map_err(|_| internal())
    })
    .await
    .map_err(|_| internal())??;
    Ok(representation(bytes, "application/json", false))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkLookupQuery {
    provider: String,
    key: String,
}

async fn work_lookup(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: Result<Path<Id>, axum::extract::rejection::PathRejection>,
    query: Result<Query<WorkLookupQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, ApiError> {
    let Path(library) = path.map_err(|_| invalid())?;
    let Query(WorkLookupQuery { provider, key }) = query.map_err(|_| invalid())?;
    if library != ctx.library_id {
        return Err(not_found());
    }
    if provider.is_empty() || provider.len() > 128 || key.is_empty() || key.len() > 4096 {
        return Err(invalid());
    }
    let permit = read_permit(&ctx, &principal.device).await?;
    let view = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        ctx.library
            .reader_work_by_key(principal.subject.get(), &provider, &key)
            .map_err(library_error)?
            .ok_or_else(not_found)
    })
    .await
    .map_err(|_| internal())??;
    let bytes = serde_json::to_vec(&view).map_err(|_| internal())?;
    Ok(representation(bytes, "application/json", false))
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
        authorize_reader_view(&ctx, principal.subject, entity, scope)?;
        ctx.library
            .reading_bundle(entity, scope)
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
        authorize_reader_view(&ctx, principal.subject, entity, scope)?;
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
        authorize_reader_view(&ctx, principal.subject, entity, scope)?;
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
            .map_err(|_| ApiError(StatusCode::RANGE_NOT_SATISFIABLE, ErrorCode::InvalidRange))?;
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
    Ok(representation(bytes, "application/vnd.piep.object", true))
}

#[cfg(test)]
mod tests;
