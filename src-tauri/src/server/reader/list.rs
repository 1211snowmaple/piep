//! 携帯の作品の一覧・作品の画面・表紙を、棚から返す。
//!
//! 一覧の条件（`MobileSearchRequest`）を検査してから棚の問い合わせ（`TextSearch` と
//! `WorkListQuery`）にする。意味はデスクトップの一覧と同じ問い合わせになる。作品は棚の UUID で返す。

use super::{
    internal, invalid, library_error, not_found, read_permit, ApiError, Context, Principal,
};
use crate::library::protocol::Id;
use crate::library::{
    CountRange, DateField, DateRange, EntityKind, Fingerprint, LibraryId, RevisionFilter,
    UserState, WorkCard, WorkCursor, WorkFilter, WorkListQuery, WorkSort,
};
use crate::library_search::{parse_query, TextSearch};
use crate::library_view::CardKeys;
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use chrono::{NaiveDate, SecondsFormat};
use piep_mobile_api::{MobileDateField, MobileSearchRequest, MobileSortOrder, MobileWorkSort};
use piep_mobile_api::{
    MobilePerson, MobileSearchSuggestion, MobileSearchSuggestions, MobileSeriesMembership,
    MobileVersion, MobileWork, MobileWorkDetail, MobileWorkPage,
};
use serde::Deserialize;
use std::collections::VecDeque;
use std::sync::{atomic::Ordering, Arc};

/// 表紙の縮小前の上限。
const MAX_COVER_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_COVER_PIXELS: u64 = 64_000_000;
/// 縮めた表紙を覚えておく数。一覧を行き来しても縮め直さない。
const REMEMBERED_COVERS: usize = 256;

/// 縮める大きさ。作品の表紙は一覧（`Card`）と画面（`Large`）、作者のヘッダーは `Banner`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CoverSize {
    Card,
    Large,
    Banner,
}

impl CoverSize {
    fn bounds(self) -> (u32, u32) {
        match self {
            Self::Card => (320, 480),
            Self::Large => (640, 960),
            Self::Banner => (1280, 480),
        }
    }
}

/// 縮めた表紙。元のファイルと大きさで引く。
pub(super) struct CoverEntry {
    file: Fingerprint,
    size: CoverSize,
    bytes: Arc<Vec<u8>>,
    mime: &'static str,
}

/// 縮めた表紙を、新しく使ったものから覚えておく。
#[derive(Default)]
pub(super) struct Covers(VecDeque<CoverEntry>);

impl Covers {
    fn get(&mut self, file: Fingerprint, size: CoverSize) -> Option<(Arc<Vec<u8>>, &'static str)> {
        let index = self
            .0
            .iter()
            .position(|entry| entry.file == file && entry.size == size)?;
        let entry = self.0.remove(index)?;
        let found = (Arc::clone(&entry.bytes), entry.mime);
        self.0.push_front(entry);
        Some(found)
    }

    fn put(&mut self, entry: CoverEntry) {
        self.0.push_front(entry);
        self.0.truncate(REMEMBERED_COVERS);
    }
}

pub(super) fn iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub(super) fn count(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// `state` はこの利用者のお気に入り（一覧の行は棚の持ち主のものなので、利用者ごとに引き直す）。
pub(super) fn shelf_work(card: &WorkCard, keys: CardKeys, state: &UserState) -> MobileWork {
    MobileWork {
        id: card.id.to_string(),
        snapshot_id: card.snapshot_id.to_string(),
        source: card.provider.clone(),
        source_id: card.external_key.clone(),
        title: card.title.clone(),
        author_name: card.author_name.clone(),
        author_id: keys.author.unwrap_or_default(),
        content_type: card.content_type.clone(),
        tags: card.tags.clone(),
        excerpt: card.excerpt.clone(),
        text_length: count(card.text_length),
        asset_count: count(card.media_count),
        has_cover: card.cover.is_some(),
        current_version: count(card.version_number),
        edited: card.edit_id.is_some(),
        edit_id: card.edit_id.map(|id| id.to_string()),
        favorite: state.favorite,
        favorite_generation: state.generation.to_string(),
        series_title: card.series_title.clone(),
        series_id: keys.series,
        source_created_at: card.published_at.map(iso),
        saved_at: iso(card.saved_at),
    }
}

/// 携帯の送る日付（その日の UTC の 0 時から）をミリ秒にする。`days` 日あとにずらす。
fn utc_day(value: Option<&str>, days: i64) -> Option<i64> {
    let date = NaiveDate::parse_from_str(value?, "%Y-%m-%d").ok()?;
    Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis() + days * 86_400_000)
}

fn range(min: Option<i64>, max: Option<i64>) -> CountRange {
    let unsigned = |value: Option<i64>| value.and_then(|value| u64::try_from(value).ok());
    CountRange {
        min: unsigned(min),
        max: unsigned(max),
    }
}

/// 一覧の続きの位置。同期 epoch と利用者を添え、違う棚・利用者の続きは受け付けない。
fn encode_cursor(cursor: &WorkCursor, epoch: Id, subject: Id) -> Result<String, ApiError> {
    let value = serde_json::to_string(cursor).map_err(|_| internal())?;
    Ok(format!("{}.{}.{value}", epoch.get(), subject.get()))
}

fn decode_cursor(value: &str, epoch: Id, subject: Id) -> Result<WorkCursor, ApiError> {
    let parts: Vec<_> = value.splitn(3, '.').collect();
    let [saved_epoch, saved_subject, cursor] = parts.as_slice() else {
        return Err(invalid());
    };
    if epoch.get().to_string() != *saved_epoch || subject.get().to_string() != *saved_subject {
        return Err(ApiError(StatusCode::CONFLICT, "stale_cursor"));
    }
    serde_json::from_str(cursor).map_err(|_| invalid())
}

/// 作者の指名（取得元と ID）。棚の作者の ID は要求ごとに棚で引く。
type PersonKey = (String, String);

/// 携帯の並べ方を棚の並べ方にする。名前は同じ。
fn work_sort(sort: MobileWorkSort) -> WorkSort {
    match sort {
        MobileWorkSort::Saved => WorkSort::Saved,
        MobileWorkSort::Published => WorkSort::Published,
        MobileWorkSort::Updated => WorkSort::Updated,
        MobileWorkSort::Title => WorkSort::Title,
        MobileWorkSort::Author => WorkSort::Author,
        MobileWorkSort::Length => WorkSort::Length,
        MobileWorkSort::Size => WorkSort::Size,
        MobileWorkSort::Media => WorkSort::Media,
        MobileWorkSort::Version => WorkSort::Version,
        MobileWorkSort::SeriesOrder => WorkSort::SeriesOrder,
    }
}

/// 携帯の検索の条件を、棚の問い合わせにする。
pub(super) fn shelf_request(
    request: MobileSearchRequest,
    epoch: Id,
    subject: Id,
) -> Result<(TextSearch, WorkListQuery, Option<PersonKey>), ApiError> {
    request.validate().map_err(|_| invalid())?;
    let mut text = request.text.as_deref().unwrap_or("").trim().to_owned();
    if let Some(key) = &request.series_key {
        let term = match &request.series_source {
            Some(source) => format!("series:{source}:{key}"),
            None => format!("series:{key}"),
        };
        text = if text.is_empty() {
            term
        } else {
            format!("{text} {term}")
        };
    }
    let sort = match request.sort_by {
        None if text.is_empty() => WorkSort::Saved,
        None => WorkSort::Relevance,
        Some(sort) => work_sort(sort),
    };
    let descending = match (request.sort_order, request.sort_by) {
        (Some(order), _) => order == MobileSortOrder::Desc,
        (None, Some(sort)) => !sort.ascending_by_default(),
        (None, None) => true,
    };
    let after = request
        .cursor
        .as_deref()
        .map(|cursor| decode_cursor(cursor, epoch, subject))
        .transpose()?;
    let limit = request
        .limit
        .unwrap_or(if request.offset.is_some() { 20 } else { 30 })
        .clamp(1, 50);
    let date = (request.date_from.is_some() || request.date_to.is_some()).then(|| DateRange {
        field: match request.date_field.unwrap_or_default() {
            MobileDateField::Saved => DateField::Saved,
            MobileDateField::Published => DateField::Published,
            MobileDateField::Updated => DateField::Updated,
        },
        from: utc_day(request.date_from.as_deref(), 0),
        until: utc_day(request.date_to.as_deref(), 1),
    });
    let filter = WorkFilter {
        provider: request.source.clone(),
        content_type: request.content_type.clone(),
        favorite: request.favorite.filter(|favorite| *favorite),
        edited: request
            .edit_filter
            .as_deref()
            .map(|value| value == "edited"),
        has_cover: request
            .cover_filter
            .as_deref()
            .map(|value| value == "has_cover"),
        revision: request.revision_filter.as_deref().map(|value| match value {
            "revised" => RevisionFilter::Revised,
            _ => RevisionFilter::FirstVersion,
        }),
        text_length: range(request.min_char_count, request.max_char_count),
        media_count: range(request.min_asset_count, request.max_asset_count),
        date,
        tags: request.tags_include.clone().unwrap_or_default(),
        any_tag: request.tag_filter_mode.as_deref() == Some("or"),
        excluded_tags: request.tags_exclude.clone().unwrap_or_default(),
        authors: request.authors_include.clone().unwrap_or_default(),
        excluded_authors: request.authors_exclude.clone().unwrap_or_default(),
        ..WorkFilter::default()
    };
    let person = request
        .person_source
        .clone()
        .zip(request.person_key.clone());
    Ok((
        TextSearch::current(text),
        WorkListQuery {
            sort,
            descending,
            limit: usize::try_from(limit).map_err(|_| invalid())?,
            after,
            offset: usize::try_from(request.offset.unwrap_or(0)).map_err(|_| invalid())?,
            filter,
            count: request.cursor.is_none(),
        },
        person,
    ))
}

pub(super) async fn search(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: Result<Path<Id>, axum::extract::rejection::PathRejection>,
    body: Result<Json<MobileSearchRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    let Path(library) = path.map_err(|_| invalid())?;
    let Json(request) = body.map_err(|_| invalid())?;
    if library != ctx.library_id {
        return Err(not_found());
    }
    let numbered = request.offset.is_some();
    let (epoch, subject) = (ctx.sync_epoch, principal.subject);
    let (search, mut query, person) = shelf_request(request, epoch, subject)?;
    // 語の検索は索引が棚に追いつくまで答えない（古い索引の結果を返さない）。絞り込みだけなら読める。
    let words = !parse_query(&search.text).text.trim().is_empty();
    if words && !ctx.search_ready.load(Ordering::Acquire) {
        return Err(ApiError(StatusCode::SERVICE_UNAVAILABLE, "search_indexing"));
    }
    let permit = read_permit(&ctx, &principal.device).await?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let library = &ctx.library;
        if let Some((provider, key)) = person {
            match library
                .entity_by_key(EntityKind::Person, &provider, &key)
                .map_err(library_error)?
            {
                Some(id) => query.filter.person = Some(id),
                // 棚にない作者の作品はない。
                None => query.filter.works = Some(Vec::new()),
            }
        }
        let page = library
            .search_works(&search, &query)
            .map_err(library_error)?;
        let keys = library.card_keys(&page.page.items).map_err(library_error)?;
        let next_cursor = match (&page.page.next, numbered) {
            // ページ番号で開いたページに続きの位置はない（次のページも番号で開く）。
            (Some(next), false) => Some(encode_cursor(next, epoch, subject)?),
            _ => None,
        };
        let items = page
            .page
            .items
            .iter()
            .zip(keys)
            .map(|(card, keys)| {
                let state = library
                    .user_state(card.id, subject.get())
                    .map_err(library_error)?;
                Ok(shelf_work(card, keys, &state))
            })
            .collect::<Result<Vec<_>, ApiError>>()?;
        serde_json::to_vec(&MobileWorkPage {
            items,
            next_cursor,
            total_estimate: page.page.total.map(count),
            index_complete: true,
        })
        .map_err(|_| internal())
    })
    .await
    .map_err(|_| internal())??;
    Ok(super::representation(bytes, "application/json", false))
}

#[derive(Deserialize)]
pub(super) struct WorkLocation {
    pub(super) library: Id,
    pub(super) work: Id,
}

pub(super) async fn work(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: Result<Path<WorkLocation>, axum::extract::rejection::PathRejection>,
    query: Result<Query<super::EmptyQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, ApiError> {
    let Path(location) = path.map_err(|_| invalid())?;
    query.map_err(|_| invalid())?;
    if location.library != ctx.library_id {
        return Err(not_found());
    }
    let work: LibraryId = location.work.get();
    let subject = principal.subject.get();
    let permit = read_permit(&ctx, &principal.device).await?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let head = ctx.library.work_head(work).map_err(library_error)?;
        let state = ctx
            .library
            .user_state(work, subject)
            .map_err(library_error)?;
        let card = &head.card;
        let people = head
            .author_key
            .iter()
            .map(|key| MobilePerson {
                source: card.provider.clone(),
                key: key.clone(),
                role: "author".into(),
                display_name: card.author_name.clone(),
            })
            .collect();
        let series = head
            .series_key
            .iter()
            .zip(&card.series_title)
            .map(|(key, title)| MobileSeriesMembership {
                source: card.provider.clone(),
                key: key.clone(),
                title: title.clone(),
                content_order: card.series_position,
            })
            .collect();
        let mut work = shelf_work(
            card,
            CardKeys {
                author: head.author_key.clone(),
                series: head.series_key.clone(),
            },
            &state,
        );
        work.excerpt = head.caption.clone();
        let detail = MobileWorkDetail {
            work,
            people,
            series,
            versions: head
                .versions
                .iter()
                .map(|version| MobileVersion {
                    snapshot_id: version.snapshot.to_string(),
                    version: version.version,
                    text_length: version.text_length,
                    saved_at: iso(version.saved_at),
                })
                .collect(),
            edit_base_version: head.edit_base_version,
        };
        serde_json::to_vec(&detail).map_err(|_| internal())
    })
    .await
    .map_err(|_| internal())??;
    Ok(super::representation(bytes, "application/json", false))
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CoverQuery {
    size: Option<String>,
}

/// 作品の今の版の表紙を縮めて返す。作品から表紙を引くので、ファイルのハッシュだけでは取れない。
pub(super) async fn cover(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: Result<Path<WorkLocation>, axum::extract::rejection::PathRejection>,
    query: Result<Query<CoverQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, ApiError> {
    let Path(location) = path.map_err(|_| invalid())?;
    let Query(CoverQuery { size }) = query.map_err(|_| invalid())?;
    if location.library != ctx.library_id {
        return Err(not_found());
    }
    let size = match size.as_deref() {
        None | Some("card") => CoverSize::Card,
        Some("large") => CoverSize::Large,
        Some(_) => return Err(invalid()),
    };
    let work: LibraryId = location.work.get();
    let permit = read_permit(&ctx, &principal.device).await?;
    let (bytes, mime) = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let card = ctx.library.work_card(work).map_err(library_error)?;
        let file = card.cover.ok_or_else(not_found)?;
        shrunk_image(&ctx, file, size)
    })
    .await
    .map_err(|_| internal())??;
    Ok(image_response(bytes, mime))
}

/// 棚のファイルの画像を縮めて返す（縮めたものは覚えておく）。画像でないファイルは 404。
pub(super) fn shrunk_image(
    ctx: &Context,
    file: Fingerprint,
    size: CoverSize,
) -> Result<(Arc<Vec<u8>>, &'static str), ApiError> {
    if let Some(found) = ctx
        .covers
        .lock()
        .ok()
        .and_then(|mut covers| covers.get(file, size))
    {
        return Ok(found);
    }
    let content = ctx.library.file(file).map_err(library_error)?;
    if content.length > MAX_COVER_SOURCE_BYTES {
        return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "asset_too_large"));
    }
    let mut source = Vec::with_capacity(usize::try_from(content.length).unwrap_or(0));
    ctx.library
        .objects
        .export(&content, &mut source)
        .map_err(|_| internal())?;
    let (width, height) = image::ImageReader::new(std::io::Cursor::new(&source))
        .with_guessed_format()
        .map_err(|_| not_found())?
        .into_dimensions()
        .map_err(|_| not_found())?;
    if u64::from(width) * u64::from(height) > MAX_COVER_PIXELS {
        return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "asset_too_large"));
    }
    let (bounds_w, bounds_h) = size.bounds();
    let options = crate::epub::intermediate::ImageCompressOptions {
        enabled: true,
        max_width: Some(bounds_w),
        max_height: Some(bounds_h),
        output_format: Some("webp".into()),
        webp_quality: 78,
        webp_method: 0,
        ..Default::default()
    };
    let (bytes, mime) = crate::epub::image_processor::process_image_bytes(&source, &options)
        .map_err(|_| not_found())?;
    let mime = match mime.as_str() {
        "image/webp" => "image/webp",
        "image/gif" => "image/gif",
        _ => return Err(not_found()),
    };
    let bytes = Arc::new(bytes);
    if let Ok(mut covers) = ctx.covers.lock() {
        covers.put(CoverEntry {
            file,
            size,
            bytes: Arc::clone(&bytes),
            mime,
        });
    }
    Ok((bytes, mime))
}

/// 縮めた画像の応答。表紙は版で変わるので、同じ URL でも確かめ直させる（携帯は版を鍵に覚える）。
pub(super) fn image_response(bytes: Arc<Vec<u8>>, mime: &'static str) -> Response {
    let mut response = (
        [(header::CONTENT_TYPE, mime)],
        Body::from(bytes.as_ref().clone()),
    )
        .into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("private, no-cache, no-transform"),
    );
    headers.insert(
        header::VARY,
        header::HeaderValue::from_static("Authorization"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    response
}

/// 候補の既定の数と上限、語の長さの上限。
const SUGGESTIONS: i64 = 8;
const MAX_SUGGESTIONS: i64 = 12;
const MAX_SUGGEST_CHARS: usize = 200;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SuggestQuery {
    text: Option<String>,
    limit: Option<i64>,
}

/// 検索欄の候補。題の候補の `sourceKey` は棚の作品の ID。
pub(super) async fn suggest(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: Result<Path<Id>, axum::extract::rejection::PathRejection>,
    query: Result<Query<SuggestQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, ApiError> {
    let Path(library) = path.map_err(|_| invalid())?;
    let Query(query) = query.map_err(|_| invalid())?;
    if library != ctx.library_id {
        return Err(not_found());
    }
    let text = query.text.unwrap_or_default();
    if text.chars().count() > MAX_SUGGEST_CHARS {
        return Err(invalid());
    }
    let limit = query.limit.unwrap_or(SUGGESTIONS).clamp(1, MAX_SUGGESTIONS);
    let permit = read_permit(&ctx, &principal.device).await?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let items = ctx
            .library
            .search_suggestions(&text, limit, &ctx.suggestions)
            .map_err(library_error)?
            .into_iter()
            .map(|item| MobileSearchSuggestion {
                kind: item.kind,
                label: item.label,
                value: item.value,
                count: item.count,
                exact_match: item.exact_match,
                source: item.source,
                source_key: item.source_key,
            })
            .collect();
        serde_json::to_vec(&MobileSearchSuggestions { items }).map_err(|_| internal())
    })
    .await
    .map_err(|_| internal())??;
    Ok(super::representation(bytes, "application/json", false))
}
