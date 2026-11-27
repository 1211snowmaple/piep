//! 携帯の作者・シリーズ・コレクションの画面を、棚から返す（評価文書 6.69）。
//!
//! 作者・シリーズは取得元と ID で指し、コレクションは棚の UUID で指す。
//! コレクションの回の作品は一覧と同じ棚の作品の形（お気に入りは利用者ごと）で返す。

use super::list::{count, image_response, shelf_work, shrunk_image, CoverSize};
use super::{
    internal, invalid, library_error, not_found, read_permit, ApiError, Context, Principal,
};
use crate::library::protocol::Id;
use crate::library::{EntityKind, Fingerprint, LibraryId, WorkFilter};
use crate::library_entity::SeriesSort;
use crate::library_entity_list::{CollectionSummary, EntityRow, EntityScope};
use axum::{
    extract::{Path, Query, State},
    response::Response,
    Extension,
};
use piep_mobile_api::MobileEntitySort;
use piep_mobile_api::{
    MobileCollection, MobileCollectionCoverTile, MobileCollectionDetail, MobileCollectionMember,
    MobileCollectionPage, MobileEntity, MobileEntityDetail, MobileEntityPage,
    MobileEntitySeriesPage, MobileFacet,
};
use serde::Deserialize;
use std::collections::VecDeque;
use std::sync::Arc;

/// 一覧の一ページの既定と上限。
const PAGE: usize = 30;
const MAX_PAGE: usize = 50;
/// 名前の絞り込みの長さの上限。
const MAX_TEXT_CHARS: usize = 200;
const MAX_OFFSET: usize = 1_000_000;
/// 作者・シリーズの画面のタグの数。
const TAGS: usize = 60;
/// 覚えておく一覧の集計の数。ページをめくるたびに集め直さない。
const REMEMBERED_ROWS: usize = 8;

/// 作者・シリーズの一覧の集計を、棚の世代と条件で覚えておく。
#[derive(Default)]
pub(super) struct EntityRows(VecDeque<(String, i64, Arc<Vec<EntityRow>>)>);

type PathResult<T> = Result<Path<T>, axum::extract::rejection::PathRejection>;
type QueryResult<T> = Result<Query<T>, axum::extract::rejection::QueryRejection>;

fn json(value: &impl serde::Serialize) -> Result<Vec<u8>, ApiError> {
    serde_json::to_vec(value).map_err(|_| internal())
}

fn this_library(ctx: &Context, library: Id) -> Result<(), ApiError> {
    if library == ctx.library_id {
        Ok(())
    } else {
        Err(not_found())
    }
}

/// 取得元と ID。空や長すぎるものは棚に無いのと同じに扱う。
fn valid_key(provider: &str, key: &str) -> bool {
    !provider.is_empty()
        && provider.len() <= 128
        && !key.is_empty()
        && key.chars().count() <= MAX_TEXT_CHARS
}

fn text(value: Option<String>) -> Result<String, ApiError> {
    let value = value.unwrap_or_default().trim().to_owned();
    if value.chars().count() > MAX_TEXT_CHARS {
        return Err(invalid());
    }
    Ok(value)
}

/// 携帯の作者・シリーズの並べ方を棚の並べ方にする。名前は同じ。
fn entity_sort(sort: MobileEntitySort) -> SeriesSort {
    match sort {
        MobileEntitySort::WorkCount => SeriesSort::WorkCount,
        MobileEntitySort::Saved => SeriesSort::Saved,
        MobileEntitySort::Published => SeriesSort::Published,
        MobileEntitySort::Updated => SeriesSort::Updated,
        MobileEntitySort::Name => SeriesSort::Name,
        MobileEntitySort::Length => SeriesSort::Length,
        MobileEntitySort::Size => SeriesSort::Size,
        MobileEntitySort::Media => SeriesSort::Media,
        MobileEntitySort::Version => SeriesSort::Version,
    }
}

/// 並べる鍵と向き。向きを省くと鍵ごとの既定（名前だけ昇順）。
fn sort(sort: Option<&str>, order: Option<&str>) -> Result<(SeriesSort, bool), ApiError> {
    let sort = match sort {
        None => SeriesSort::WorkCount,
        Some(value) => entity_sort(
            serde_json::from_value(serde_json::Value::String(value.to_owned()))
                .map_err(|_| invalid())?,
        ),
    };
    let ascending = match order {
        None => sort.ascending_by_default(),
        Some("asc") => true,
        Some("desc") => false,
        Some(_) => return Err(invalid()),
    };
    Ok((sort, ascending))
}

fn limit(value: Option<usize>) -> usize {
    value.unwrap_or(PAGE).clamp(1, MAX_PAGE)
}

/// 続きの位置は何件目からかを十進で渡す。
fn cursor_offset(cursor: Option<&str>) -> Result<usize, ApiError> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    if cursor.is_empty() || cursor.len() > 7 || !cursor.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let offset = cursor.parse::<usize>().map_err(|_| invalid())?;
    if offset > MAX_OFFSET {
        return Err(invalid());
    }
    Ok(offset)
}

/// 画像のファイルの識別子（16 進の 64 文字）。
fn file(hex: &str) -> Option<Fingerprint> {
    Fingerprint::parse(hex).ok()
}

#[cfg(test)]
#[test]
fn image_fingerprint_rejects_a_sign_in_a_byte() {
    assert!(file(&format!("+0{}", "00".repeat(31))).is_none());
}

pub(super) fn mobile_collection(
    summary: CollectionSummary,
    position: Option<i64>,
) -> MobileCollection {
    MobileCollection {
        id: summary.id.to_string(),
        name: summary.name,
        description: summary.description,
        collection_kind: if summary.ordered {
            "ordered"
        } else {
            "unordered"
        }
        .into(),
        member_count: count(summary.member_count),
        available_count: count(summary.available_count),
        // 作品番号は無い。単独の表紙は `cover/single` から引く。
        cover_mode: summary.cover_mode,
        cover_revision: summary.revision,
        has_single_cover: summary.cover.is_some(),
        has_file_cover: summary.cover_image.is_some(),
        cover_tiles: summary
            .tiles
            .into_iter()
            .map(|tile| MobileCollectionCoverTile {
                source: tile.provider,
                source_id: tile.external_key,
                title: tile.title,
                author_name: tile.author_name,
                has_cover: tile.cover.is_some(),
            })
            .collect(),
        position,
    }
}

fn entity_row(kind: EntityKind, row: EntityRow) -> MobileEntity {
    MobileEntity {
        has_cover: match kind {
            EntityKind::Person => row.icon.is_some(),
            _ => row.cover.is_some(),
        },
        source: row.provider,
        source_key: row.external_key,
        display_name: row.name,
        count: count(row.work_count),
        description: row.description,
        sample_title: row.sample_title,
        is_concluded: row.is_concluded,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ListQuery {
    text: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
    sort: Option<String>,
    order: Option<String>,
}

async fn list(
    ctx: Arc<Context>,
    principal: Principal,
    path: PathResult<Id>,
    query: QueryResult<ListQuery>,
    kind: EntityKind,
) -> Result<Response, ApiError> {
    let Path(library) = path.map_err(|_| invalid())?;
    let Query(query) = query.map_err(|_| invalid())?;
    this_library(&ctx, library)?;
    let needle = text(query.text)?;
    let offset = query.offset.unwrap_or(0);
    if offset > MAX_OFFSET {
        return Err(invalid());
    }
    let (sort, ascending) = sort(query.sort.as_deref(), query.order.as_deref())?;
    let limit = limit(query.limit);
    let permit = read_permit(&ctx, &principal.device).await?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let library = &ctx.library;
        let generation = library.catalogue_generation().map_err(library_error)?;
        let key = format!("{kind:?}\u{1f}{needle}");
        let remembered = ctx
            .entity_rows
            .lock()
            .map_err(|_| internal())?
            .0
            .iter()
            .find(|(k, g, _)| *k == key && *g == generation)
            .map(|(_, _, rows)| Arc::clone(rows));
        let rows = match remembered {
            Some(rows) => rows,
            None => {
                let rows = Arc::new(
                    library
                        .entity_rows(
                            kind,
                            &needle,
                            &WorkFilter::default(),
                            EntityScope::default(),
                        )
                        .map_err(library_error)?,
                );
                let mut cache = ctx.entity_rows.lock().map_err(|_| internal())?;
                cache.0.retain(|(k, _, _)| *k != key);
                cache.0.push_front((key, generation, Arc::clone(&rows)));
                cache.0.truncate(REMEMBERED_ROWS);
                rows
            }
        };
        let page = library
            .entity_page(&rows, kind, sort, ascending, offset, limit)
            .map_err(library_error)?;
        let end = offset + page.items.len();
        json(&MobileEntityPage {
            next_offset: (end < page.total).then(|| count(end as u64)),
            total: count(page.total as u64),
            items: page
                .items
                .into_iter()
                .map(|row| entity_row(kind, row))
                .collect(),
        })
    })
    .await
    .map_err(|_| internal())??;
    Ok(super::representation(bytes, "application/json", false))
}

pub(super) async fn people(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: PathResult<Id>,
    query: QueryResult<ListQuery>,
) -> Result<Response, ApiError> {
    list(ctx, principal, path, query, EntityKind::Person).await
}

pub(super) async fn series(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: PathResult<Id>,
    query: QueryResult<ListQuery>,
) -> Result<Response, ApiError> {
    list(ctx, principal, path, query, EntityKind::Series).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct KeyQuery {
    provider: String,
    key: String,
}

/// プロフィールのリンク。外へ開くものだけを渡す。
fn profile_links(links: Vec<String>) -> Vec<String> {
    links
        .into_iter()
        .filter(|link| link.starts_with("https://") || link.starts_with("http://"))
        .take(20)
        .collect()
}

async fn detail(
    ctx: Arc<Context>,
    principal: Principal,
    path: PathResult<Id>,
    query: QueryResult<KeyQuery>,
    kind: EntityKind,
) -> Result<Response, ApiError> {
    let Path(library) = path.map_err(|_| invalid())?;
    let Query(KeyQuery { provider, key }) = query.map_err(|_| invalid())?;
    this_library(&ctx, library)?;
    if !valid_key(&provider, &key) {
        return Err(not_found());
    }
    let permit = read_permit(&ctx, &principal.device).await?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let library = &ctx.library;
        let head = library
            .entity_head(kind, &provider, &key)
            .map_err(library_error)?
            .ok_or_else(not_found)?;
        let tags = library
            .entity_tags(kind, head.id, TAGS)
            .map_err(library_error)?;
        let collections = library
            .entity_collections(kind, head.id)
            .map_err(library_error)?;
        let person = kind == EntityKind::Person;
        let series_count = if person {
            let page = library
                .person_series(head.id, "", SeriesSort::WorkCount, false, 0, 1)
                .map_err(library_error)?;
            Some(count(page.total as u64))
        } else {
            None
        };
        json(&MobileEntityDetail {
            source: head.provider,
            source_key: head.external_key,
            display_name: head.name,
            description: head.description,
            work_count: count(head.work_count),
            has_cover: if person {
                head.icon.is_some()
            } else {
                head.cover.is_some()
            },
            has_banner: person && head.banner.is_some(),
            is_concluded: head.is_concluded,
            published_count: head.published_content_count,
            links: if person {
                profile_links(head.links)
            } else {
                Vec::new()
            },
            tags: tags
                .into_iter()
                .map(|tag| MobileFacet {
                    name: tag.name,
                    count: count(tag.count),
                })
                .collect(),
            series_count,
            collections: collections
                .into_iter()
                .map(|summary| mobile_collection(summary, None))
                .collect(),
        })
    })
    .await
    .map_err(|_| internal())??;
    Ok(super::representation(bytes, "application/json", false))
}

pub(super) async fn person_detail(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: PathResult<Id>,
    query: QueryResult<KeyQuery>,
) -> Result<Response, ApiError> {
    detail(ctx, principal, path, query, EntityKind::Person).await
}

pub(super) async fn series_detail(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: PathResult<Id>,
    query: QueryResult<KeyQuery>,
) -> Result<Response, ApiError> {
    detail(ctx, principal, path, query, EntityKind::Series).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PersonSeriesQuery {
    provider: String,
    key: String,
    text: Option<String>,
    sort: Option<String>,
    order: Option<String>,
    /// 続きの位置（前のページの `nextCursor`）。
    cursor: Option<String>,
    limit: Option<usize>,
}

/// 作者の作品が属するシリーズの一ページ。続きの位置は何件目からかを十進で渡す。
pub(super) async fn person_series(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: PathResult<Id>,
    query: QueryResult<PersonSeriesQuery>,
) -> Result<Response, ApiError> {
    let Path(library) = path.map_err(|_| invalid())?;
    let Query(query) = query.map_err(|_| invalid())?;
    this_library(&ctx, library)?;
    if !valid_key(&query.provider, &query.key) {
        return Err(not_found());
    }
    let needle = text(query.text)?;
    let (sort, ascending) = sort(query.sort.as_deref(), query.order.as_deref())?;
    let offset = cursor_offset(query.cursor.as_deref())?;
    let limit = limit(query.limit);
    let permit = read_permit(&ctx, &principal.device).await?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let library = &ctx.library;
        let person = library
            .entity_by_key(EntityKind::Person, &query.provider, &query.key)
            .map_err(library_error)?
            .ok_or_else(not_found)?;
        let page = library
            .person_series(person, &needle, sort, ascending, offset, limit)
            .map_err(library_error)?;
        json(&MobileEntitySeriesPage {
            next_cursor: page.next.map(|next| next.to_string()),
            total: count(page.total as u64),
            items: page
                .items
                .into_iter()
                .map(|series| MobileEntity {
                    has_cover: series.cover.is_some(),
                    source: series.provider,
                    source_key: series.external_key,
                    display_name: series.name,
                    count: count(series.work_count),
                    description: series.description,
                    sample_title: series.sample_title,
                    is_concluded: series.is_concluded,
                })
                .collect(),
        })
    })
    .await
    .map_err(|_| internal())??;
    Ok(super::representation(bytes, "application/json", false))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CoverQuery {
    provider: String,
    key: String,
    /// 作者のヘッダー画像は `banner`。
    variant: Option<String>,
}

/// 作者のアイコン・ヘッダー、シリーズの表紙。作者・シリーズの今の版の画像を縮めて返す。
async fn entity_cover(
    ctx: Arc<Context>,
    principal: Principal,
    path: PathResult<Id>,
    query: QueryResult<CoverQuery>,
    kind: EntityKind,
) -> Result<Response, ApiError> {
    let Path(library) = path.map_err(|_| invalid())?;
    let Query(query) = query.map_err(|_| invalid())?;
    this_library(&ctx, library)?;
    let banner = match query.variant.as_deref() {
        None => false,
        Some("banner") if kind == EntityKind::Person => true,
        Some(_) => return Err(invalid()),
    };
    if !valid_key(&query.provider, &query.key) {
        return Err(not_found());
    }
    let permit = read_permit(&ctx, &principal.device).await?;
    let (bytes, mime) = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let head = ctx
            .library
            .entity_head(kind, &query.provider, &query.key)
            .map_err(library_error)?
            .ok_or_else(not_found)?;
        let image = match (kind, banner) {
            (EntityKind::Person, true) => head.banner,
            (EntityKind::Person, false) => head.icon,
            _ => head.cover,
        };
        let image = image.as_deref().and_then(file).ok_or_else(not_found)?;
        shrunk_image(
            &ctx,
            image,
            if banner {
                CoverSize::Banner
            } else {
                CoverSize::Card
            },
        )
    })
    .await
    .map_err(|_| internal())??;
    Ok(image_response(bytes, mime))
}

pub(super) async fn person_cover(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: PathResult<Id>,
    query: QueryResult<CoverQuery>,
) -> Result<Response, ApiError> {
    entity_cover(ctx, principal, path, query, EntityKind::Person).await
}

pub(super) async fn series_cover(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: PathResult<Id>,
    query: QueryResult<CoverQuery>,
) -> Result<Response, ApiError> {
    entity_cover(ctx, principal, path, query, EntityKind::Series).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CollectionQuery {
    cursor: Option<String>,
    limit: Option<usize>,
}

/// 次の一件があるときは続きの位置を返し、その一件は次のページに回す。
fn collection_cursor<T>(items: &mut Vec<T>, offset: usize, limit: usize) -> Option<String> {
    let end = offset + limit;
    let next = (items.len() > limit && end <= MAX_OFFSET).then(|| end.to_string());
    items.truncate(limit);
    next
}

/// コレクションの一覧の一ページ（更新の新しい順）。
pub(super) async fn collections(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: PathResult<Id>,
    query: QueryResult<CollectionQuery>,
) -> Result<Response, ApiError> {
    let Path(library) = path.map_err(|_| invalid())?;
    let Query(query) = query.map_err(|_| invalid())?;
    this_library(&ctx, library)?;
    let offset = cursor_offset(query.cursor.as_deref())?;
    let limit = limit(query.limit);
    let permit = read_permit(&ctx, &principal.device).await?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let mut summaries = ctx
            .library
            .collection_summaries_page(limit + 1, offset)
            .map_err(library_error)?;
        let next_cursor = collection_cursor(&mut summaries, offset, limit);
        json(&MobileCollectionPage {
            items: summaries
                .into_iter()
                .map(|summary| mobile_collection(summary, None))
                .collect(),
            next_cursor,
        })
    })
    .await
    .map_err(|_| internal())??;
    Ok(super::representation(bytes, "application/json", false))
}

#[derive(Deserialize)]
pub(super) struct CollectionLocation {
    library: Id,
    collection: String,
}

/// 画面の ID（棚の UUID）からコレクションを引く。
fn collection_target(id: &str) -> Result<LibraryId, ApiError> {
    id.parse::<LibraryId>().map_err(|_| not_found())
}

/// コレクションの画面と回の一ページ。棚にある回は一覧と同じ作品の形。
pub(super) async fn collection(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: PathResult<CollectionLocation>,
    query: QueryResult<CollectionQuery>,
) -> Result<Response, ApiError> {
    let Path(location) = path.map_err(|_| invalid())?;
    let Query(query) = query.map_err(|_| invalid())?;
    this_library(&ctx, location.library)?;
    let offset = cursor_offset(query.cursor.as_deref())?;
    let limit = limit(query.limit);
    let subject = principal.subject.get();
    let permit = read_permit(&ctx, &principal.device).await?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let library = &ctx.library;
        let target = collection_target(&location.collection)?;
        let mut detail = library
            .collection_detail_page(target, limit + 1, offset)
            .map_err(library_error)?;
        let next_cursor = collection_cursor(&mut detail.members, offset, limit);
        let cards: Vec<_> = detail
            .members
            .iter()
            .filter_map(|member| member.work.clone())
            .collect();
        let mut keys = library
            .card_keys(&cards)
            .map_err(library_error)?
            .into_iter();
        let mut members = Vec::with_capacity(detail.members.len());
        for member in detail.members {
            let work = match &member.work {
                Some(card) => {
                    let keys = keys.next().ok_or_else(internal)?;
                    let state = library
                        .user_state(card.id, subject)
                        .map_err(library_error)?;
                    Some(shelf_work(card, keys, &state))
                }
                None => None,
            };
            members.push(MobileCollectionMember {
                title: member.title,
                author_name: member.author_name,
                position: member.position,
                work,
            });
        }
        json(&MobileCollectionDetail {
            summary: mobile_collection(detail.summary, None),
            members,
            next_cursor,
        })
    })
    .await
    .map_err(|_| internal())??;
    Ok(super::representation(bytes, "application/json", false))
}

#[derive(Deserialize)]
pub(super) struct CollectionCoverLocation {
    library: Id,
    collection: Id,
    slot: String,
}

/// コレクションの表紙。`single` は表紙に選んだ作品の表紙、`file` は選んだ画像、
/// `tile-0`〜`tile-3` は並びの先頭からの回の表紙。
pub(super) async fn collection_cover(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: PathResult<CollectionCoverLocation>,
    query: QueryResult<super::EmptyQuery>,
) -> Result<Response, ApiError> {
    let Path(location) = path.map_err(|_| invalid())?;
    query.map_err(|_| invalid())?;
    this_library(&ctx, location.library)?;
    enum Slot {
        Single,
        File,
        Tile(usize),
    }
    let slot = match location.slot.as_str() {
        "single" => Slot::Single,
        "file" => Slot::File,
        "tile-0" => Slot::Tile(0),
        "tile-1" => Slot::Tile(1),
        "tile-2" => Slot::Tile(2),
        "tile-3" => Slot::Tile(3),
        _ => return Err(not_found()),
    };
    let target: LibraryId = location.collection.get();
    let permit = read_permit(&ctx, &principal.device).await?;
    let (bytes, mime) = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let summary = ctx
            .library
            .collection_summary(target)
            .map_err(library_error)?
            .ok_or_else(not_found)?;
        let image = match slot {
            Slot::Single => summary.cover,
            Slot::File => summary.cover_image,
            Slot::Tile(index) => summary
                .tiles
                .into_iter()
                .nth(index)
                .and_then(|tile| tile.cover),
        };
        let image = image.as_deref().and_then(file).ok_or_else(not_found)?;
        shrunk_image(&ctx, image, CoverSize::Card)
    })
    .await
    .map_err(|_| internal())??;
    Ok(image_response(bytes, mime))
}

/// 作品の入ったコレクションと、その中での位置。
pub(super) async fn work_collections(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: PathResult<super::list::WorkLocation>,
    query: QueryResult<super::EmptyQuery>,
) -> Result<Response, ApiError> {
    let Path(location) = path.map_err(|_| invalid())?;
    query.map_err(|_| invalid())?;
    this_library(&ctx, location.library)?;
    let work: LibraryId = location.work.get();
    let permit = read_permit(&ctx, &principal.device).await?;
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        // 棚に無い作品のコレクションは出さない。
        ctx.library.work_card(work).map_err(library_error)?;
        let items: Vec<MobileCollection> = ctx
            .library
            .work_collections(work)
            .map_err(library_error)?
            .into_iter()
            .map(|(summary, position)| mobile_collection(summary, Some(position)))
            .collect();
        json(&items)
    })
    .await
    .map_err(|_| internal())??;
    Ok(super::representation(bytes, "application/json", false))
}
