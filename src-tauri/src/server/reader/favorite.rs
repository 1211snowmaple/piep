//! 携帯からお気に入りを付け外しする。
//!
//! 端末は開いている棚の利用者と同期世代をヘッダーに付け、要求 UUID と更新世代を本文に入れる。
//! 利用者か同期世代が食い違えば書かずに library_changed を返す。
//! 棚は要求の結果を受領として残すので、同じ要求を送り直しても一度だけ効く。
//! お気に入りの更新世代が違えば favorite_conflict と今の値を返す。
//!
//! デスクトップ版は棚の持ち主の変更を `/api/admin/shelf/favorites` から取り込む。

use super::{
    check_write_scope, internal, invalid, not_found, ApiError, Context, ErrorCode, Principal,
};
use crate::library::protocol::Id;
use crate::library::{LibraryId, UserState};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use piep_mobile_api::{MobileFavorite, MutationConflict};
use std::sync::Arc;

pub(super) fn favorite_state(state: &UserState) -> MobileFavorite {
    MobileFavorite {
        favorite: state.favorite,
        generation: state.generation.to_string(),
    }
}

/// `MobileFavoriteUpdate` と同じ形。知らない項目は断る。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct FavoriteBody {
    request_id: String,
    expected: String,
    favorite: bool,
}

#[derive(serde::Deserialize)]
pub(super) struct FavoriteLocation {
    library: Id,
    work: Id,
}

pub(super) async fn put(
    State(ctx): State<Arc<Context>>,
    Extension(principal): Extension<Principal>,
    path: Result<Path<FavoriteLocation>, axum::extract::rejection::PathRejection>,
    headers: HeaderMap,
    body: Result<Json<FavoriteBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    let Path(location) = path.map_err(|_| invalid())?;
    let Json(update) = body.map_err(|_| invalid())?;
    if location.library != ctx.library_id {
        return Err(not_found());
    }
    check_write_scope(&headers, &principal, &ctx)?;
    let request = uuid::Uuid::parse_str(&update.request_id).map_err(|_| invalid())?;
    // 十進の数字だけ（符号・空白・先頭の 0 の重ねを受け付けない）。
    let canonical = update
        .expected
        .parse::<u64>()
        .ok()
        .filter(|value| value.to_string() == update.expected);
    let expected = canonical
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(invalid)?;
    if request.is_nil() {
        return Err(invalid());
    }
    let work: LibraryId = location.work.get();
    let subject = principal.subject.get();
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
        match library.set_favorite(request, work, subject, expected, update.favorite) {
            Ok(state) => Ok(Json(favorite_state(&state)).into_response()),
            // 世代が違う（または同じ要求 ID を別の変更に使った）。今の値を返し、端末に合わせ直させる。
            Err(crate::library::Error::Integrity(_)) => {
                let current = library.user_state(work, subject).map_err(|_| internal())?;
                let mut response = (
                    StatusCode::CONFLICT,
                    Json(MutationConflict {
                        code: ErrorCode::FavoriteConflict,
                        current: favorite_state(&current),
                    }),
                )
                    .into_response();
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    header::HeaderValue::from_static("no-store"),
                );
                Ok(response)
            }
            Err(crate::library::Error::Busy) => {
                Err(ApiError(StatusCode::SERVICE_UNAVAILABLE, ErrorCode::Busy))
            }
            Err(error) => {
                log::error!("棚のお気に入りを書けません: {error}");
                Err(internal())
            }
        }
    })
    .await
    .map_err(|_| internal())?
}
