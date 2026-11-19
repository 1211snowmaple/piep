//! 携帯と新しい棚の読書位置を受け渡す。
//!
//! 書き込みはお気に入りと同じく、端末が作る要求 UUID と手元の更新世代を添える。棚はその要求の
//! 結果を受領として残すので、送り直しても一度だけ効く。世代が違えば（別の端末やデスクトップで
//! 先に読み進めていれば）409 と今の位置を返し、端末の時刻で勝者を決めない。
//!
//! 一覧は、初めは利用者の位置を作品の順に全部、そのあとは棚の変更の記録（outbox）の番号で
//! 変わった分だけを返す。全部を返し始める前に記録の番号を控えるので、途中で変わった位置も
//! 次の差分で届く。番号は棚の写しごとなので、続きの印には同期の世代を入れ、違えば断る。

use super::{internal, invalid, not_found, read_permit, ApiError, Context, Principal};
use crate::library::protocol::Id;
use crate::library::{
    server_device, LibraryId, ReadingAnchor, ReadingLocation, ReadingPosition, ReadingUpdate,
    PAGE_RATIO_SCALE,
};
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use piep_mobile_api::{MobileShelfReading, MobileShelfReadingLocation, MobileShelfReadingPage};
use std::sync::Arc;

/// 一ページの既定と上限。
const PAGE: i64 = 200;
const MAX_PAGE: i64 = 500;
/// 読書画面のページ番号の上限（旧 API と同じ）。
const MAX_PAGE_NUMBER: u64 = 1_000_000;

pub(super) fn reading_item(
    work: LibraryId,
    position: &ReadingPosition,
    device: LibraryId,
) -> MobileShelfReading {
    let location = position.location.as_ref().and_then(|location| {
        let (view_kind, view, anchor) = match location {
            ReadingLocation::Source { snapshot, anchor } => ("snapshot", snapshot, anchor),
            ReadingLocation::Edit { edit, anchor } => ("edit", edit, anchor),
        };
        match anchor {
            ReadingAnchor::Page { page, ratio_ppm } => Some(MobileShelfReadingLocation {
                view_kind: view_kind.into(),
                view_id: view.to_string(),
                page: *page,
                ratio_ppm: *ratio_ppm,
            }),
            // 文字の位置はまだどの画面も書かない。携帯はページの位置だけを使う。
            _ => None,
        }
    });
    MobileShelfReading {
        work: work.to_string(),
        location,
        finished: position.finished,
        mine: position.device == Some(device),
        updated_at: position.updated_at,
        generation: position.generation.to_string(),
    }
}

/// 十進の数字だけ（符号・空白・先頭の 0 の重ねを受け付けない）。
fn decimal(value: &str) -> Option<i64> {
    value
        .parse::<u64>()
        .ok()
        .filter(|parsed| parsed.to_string() == value)
        .and_then(|parsed| i64::try_from(parsed).ok())
}

/// 続きの印。`{同期の世代}.f.{控えた番号}.{最後の作品}` は全部を返している途中、
/// `{同期の世代}.s.{番号}` は変わった分だけ。
#[derive(Debug, PartialEq, Eq)]
enum Cursor {
    Full {
        mark: Option<i64>,
        after: Option<LibraryId>,
    },
    Since(i64),
}

fn parse_cursor(value: &str, epoch: Id) -> Result<Cursor, ApiError> {
    if value.is_empty() {
        return Ok(Cursor::Full {
            mark: None,
            after: None,
        });
    }
    let parts: Vec<&str> = value.split('.').collect();
    let epoch = epoch.get().to_string();
    match parts.as_slice() {
        [saved, "f", mark, work] if *saved == epoch => {
            let work = uuid::Uuid::parse_str(work)
                .ok()
                .filter(|parsed| parsed.to_string() == *work)
                .ok_or_else(invalid)?;
            Ok(Cursor::Full {
                mark: Some(decimal(mark).ok_or_else(invalid)?),
                after: Some(work),
            })
        }
        [saved, "s", since] if *saved == epoch => {
            Ok(Cursor::Since(decimal(since).ok_or_else(invalid)?))
        }
        // 別の写しの印（番号の意味が違う）や崩れた印。端末は空の印から取り直す。
        _ => Err(ApiError(StatusCode::BAD_REQUEST, "reading_cursor_expired")),
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadingQuery {
    cursor: Option<String>,
    limit: Option<i64>,
}

/// 認証した利用者の読書位置。
pub(super) async fn list(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: Result<Path<Id>, axum::extract::rejection::PathRejection>,
    query: Result<Query<ReadingQuery>, axum::extract::rejection::QueryRejection>,
) -> Result<Response, ApiError> {
    let Path(library) = path.map_err(|_| invalid())?;
    let Query(query) = query.map_err(|_| invalid())?;
    if library != ctx.library_id {
        return Err(not_found());
    }
    let epoch = ctx.sync_epoch;
    let cursor = parse_cursor(query.cursor.as_deref().unwrap_or(""), epoch)?;
    let limit = usize::try_from(query.limit.unwrap_or(PAGE).clamp(1, MAX_PAGE)).unwrap_or(1);
    let permit = read_permit(&ctx, &principal.device).await?;
    let subject = principal.subject.get();
    let device = server_device(&principal.device);
    let bytes = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let library = &ctx.library;
        let fail = |error: crate::library::Error| {
            log::error!("新しい棚の読書位置を読めません: {error}");
            internal()
        };
        let page = match cursor {
            Cursor::Full { mark, after } => {
                // 返し始める前に番号を控える。この後に変わった位置は差分で届く。
                let mark = match mark {
                    Some(mark) => mark,
                    None => library.reading_sequence().map_err(fail)?,
                };
                let mut rows = library
                    .reading_positions_after(subject, after, limit + 1)
                    .map_err(fail)?;
                let more = rows.len() > limit;
                rows.truncate(limit);
                let cursor = match rows.last() {
                    Some((work, _)) if more => format!("{}.f.{mark}.{work}", epoch.get()),
                    _ => format!("{}.s.{mark}", epoch.get()),
                };
                MobileShelfReadingPage {
                    items: rows
                        .iter()
                        .map(|(work, position)| reading_item(*work, position, device))
                        .collect(),
                    cursor,
                    more,
                }
            }
            Cursor::Since(since) => {
                let mut changes = library.reading_changes(since, limit + 1).map_err(fail)?;
                let more = changes.len() > limit;
                changes.truncate(limit);
                let next = changes.last().map_or(since, |(_, sequence)| *sequence);
                let mut items = Vec::new();
                for (work, _) in changes {
                    let position = library.reading_position(work, subject).map_err(fail)?;
                    // ほかの利用者の位置だけが変わった作品は返さない。
                    if position.generation > 0 {
                        items.push(reading_item(work, &position, device));
                    }
                }
                MobileShelfReadingPage {
                    items,
                    cursor: format!("{}.s.{next}", epoch.get()),
                    more,
                }
            }
        };
        serde_json::to_vec(&page).map_err(|_| internal())
    })
    .await
    .map_err(|_| internal())??;
    Ok(super::representation(bytes, "application/json", false))
}

/// `MobileShelfReadingUpdate` と同じ形。知らない項目は断る。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ReadingBody {
    request_id: String,
    expected: String,
    location: LocationBody,
    finished: bool,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LocationBody {
    view_kind: String,
    view_id: String,
    page: u64,
    ratio_ppm: u32,
}

#[derive(serde::Deserialize)]
pub(super) struct WorkPath {
    library: Id,
    work: Id,
}

fn conflict(current: MobileShelfReading) -> Response {
    let mut response = (
        StatusCode::CONFLICT,
        Json(serde_json::json!({ "error": "reading_conflict", "current": current })),
    )
        .into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

/// 読書位置を書く。読む画面のページ（0 から）とその中の割合を、読んでいる版の写しか編集に結ぶ。
pub(super) async fn put(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: Result<Path<WorkPath>, axum::extract::rejection::PathRejection>,
    body: Result<Json<ReadingBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    let Path(location) = path.map_err(|_| invalid())?;
    let Json(update) = body.map_err(|_| invalid())?;
    if location.library != ctx.library_id {
        return Err(not_found());
    }
    let request = uuid::Uuid::parse_str(&update.request_id)
        .ok()
        .filter(|request| !request.is_nil())
        .ok_or_else(invalid)?;
    let expected = decimal(&update.expected).ok_or_else(invalid)?;
    let view = uuid::Uuid::parse_str(&update.location.view_id).map_err(|_| invalid())?;
    if update.location.page > MAX_PAGE_NUMBER || update.location.ratio_ppm > PAGE_RATIO_SCALE {
        return Err(invalid());
    }
    let anchor = ReadingAnchor::Page {
        page: update.location.page,
        ratio_ppm: update.location.ratio_ppm,
    };
    let reading = match update.location.view_kind.as_str() {
        "snapshot" => ReadingLocation::Source {
            snapshot: view,
            anchor,
        },
        "edit" => ReadingLocation::Edit { edit: view, anchor },
        _ => return Err(invalid()),
    };
    let work: LibraryId = location.work.get();
    let subject = principal.subject.get();
    let device = server_device(&principal.device);
    tokio::task::spawn_blocking(move || {
        let library = &ctx.library;
        // 棚に載っている作品だけ（外した作品や、知らない UUID は 404）。
        if library
            .library_item_generation(work)
            .map_err(|_| internal())?
            .is_none()
        {
            return Err(not_found());
        }
        // 編集は、その利用者が読める編集だけ。
        if let ReadingLocation::Edit { edit, .. } = &reading {
            if !library
                .reader_may_read_edit(work, subject, *edit)
                .map_err(|_| internal())?
            {
                return Err(not_found());
            }
        }
        match library.set_reading_position(ReadingUpdate {
            request,
            work,
            user: subject,
            device,
            expected,
            location: Some(reading),
            finished: update.finished,
        }) {
            Ok(position) => Ok(Json(reading_item(work, &position, device)).into_response()),
            // 世代の食い違いか、本文に無い位置（別の作品の版・範囲の外）。
            Err(crate::library::Error::Integrity(_)) => {
                let current = library
                    .reading_position(work, subject)
                    .map_err(|_| internal())?;
                if current.generation != expected {
                    Ok(conflict(reading_item(work, &current, device)))
                } else {
                    Err(invalid())
                }
            }
            Err(crate::library::Error::Busy) => {
                Err(ApiError(StatusCode::SERVICE_UNAVAILABLE, "busy"))
            }
            // その作品のものでない版・編集（引いても見つからない）。
            Err(crate::library::Error::ResourceUnavailable)
            | Err(crate::library::Error::Sql(rusqlite::Error::QueryReturnedNoRows)) => {
                Err(invalid())
            }
            Err(error) => {
                log::error!("新しい棚の読書位置を書けません: {error}");
                Err(internal())
            }
        }
    })
    .await
    .map_err(|_| internal())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_belong_to_one_copy_of_the_shelf_and_are_canonical() {
        let epoch = Id::new(uuid::Uuid::now_v7()).unwrap();
        let other = Id::new(uuid::Uuid::now_v7()).unwrap();
        let work = uuid::Uuid::now_v7();
        let (e, o) = (epoch.get(), other.get());
        assert_eq!(
            parse_cursor("", epoch).unwrap(),
            Cursor::Full {
                mark: None,
                after: None
            }
        );
        assert_eq!(
            parse_cursor(&format!("{e}.f.7.{work}"), epoch).unwrap(),
            Cursor::Full {
                mark: Some(7),
                after: Some(work)
            }
        );
        assert_eq!(
            parse_cursor(&format!("{e}.s.12"), epoch).unwrap(),
            Cursor::Since(12)
        );
        for bad in [
            format!("{o}.s.12"),
            format!("{e}.s.012"),
            format!("{e}.s.-1"),
            format!("{e}.f.7.{}", work.to_string().to_uppercase()),
            format!("{e}.x.1"),
            "garbage".to_string(),
        ] {
            assert!(parse_cursor(&bad, epoch).is_err(), "{bad}");
        }
    }
}
