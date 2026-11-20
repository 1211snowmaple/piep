//! 携帯からお気に入りを付け外しする（新しい棚）。
//!
//! 端末は要求 UUID と手元の更新世代を添えて送る。棚はその要求の結果を受領として残すので、
//! 同じ要求を送り直しても一度だけ効く。世代が違えば（別の端末やデスクトップで変わっていれば）
//! 409 と今の値を返し、端末の時刻で勝者を決めない。
//!
//! 単独サーバーは同じ変更を旧 API の変更の記録にも残す。デスクトップ版はその記録を今までどおり
//! 取り込み、旧 API の画面（作者・シリーズ・コレクション）も同じ値を出す。写しを入れ替えたら、
//! デスクトップ版がまだ取り込んでいない変更を新しい写しへ当て直す。

use super::{internal, invalid, not_found, ApiError, Context, Principal};
use crate::library::protocol::Id;
use crate::library::{LibraryId, UserState};
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use piep_mobile_api::MobileShelfFavorite;
use std::sync::Arc;

/// 旧 API の変更の記録へ写す口。引数は取得元・取得元の作品 ID・端末・お気に入りか。
pub type FavoriteMirror = Box<dyn Fn(&str, &str, &str, bool) + Send + Sync>;

pub(super) fn favorite_state(state: &UserState) -> MobileShelfFavorite {
    MobileShelfFavorite {
        favorite: state.favorite,
        generation: state.generation.to_string(),
    }
}

/// `MobileShelfFavoriteUpdate` と同じ形。知らない項目は断る。
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
    body: Result<Json<FavoriteBody>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, ApiError> {
    let Path(location) = path.map_err(|_| invalid())?;
    let Json(update) = body.map_err(|_| invalid())?;
    if location.library != ctx.library_id {
        return Err(not_found());
    }
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
    let device = principal.device.clone();
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
            Ok(state) => {
                ctx.mirror_favorite(work, &device, state.favorite);
                Ok(Json(favorite_state(&state)).into_response())
            }
            // 世代が違う（または同じ要求 ID を別の変更に使った）。今の値を返し、端末に合わせ直させる。
            Err(crate::library::Error::Integrity(_)) => {
                let current = library.user_state(work, subject).map_err(|_| internal())?;
                let mut response = (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({
                        "error": "favorite_conflict",
                        "current": favorite_state(&current),
                    })),
                )
                    .into_response();
                response.headers_mut().insert(
                    header::CACHE_CONTROL,
                    header::HeaderValue::from_static("no-store"),
                );
                Ok(response)
            }
            Err(crate::library::Error::Busy) => {
                Err(ApiError(StatusCode::SERVICE_UNAVAILABLE, "busy"))
            }
            Err(error) => {
                log::error!("新しい棚のお気に入りを書けません: {error}");
                Err(internal())
            }
        }
    })
    .await
    .map_err(|_| internal())?
}

impl Context {
    /// 旧 API の変更の記録へ写す口を決める（単独サーバーだけ）。
    pub fn set_favorite_mirror(&self, mirror: FavoriteMirror) {
        let _ = self.favorite_mirror.set(mirror);
    }

    fn mirror_favorite(&self, work: LibraryId, device: &str, favorite: bool) {
        let Some(mirror) = self.favorite_mirror.get() else {
            return;
        };
        match self.library.work_head(work) {
            Ok(head) => mirror(
                &head.card.provider,
                &head.card.external_key,
                device,
                favorite,
            ),
            Err(error) => log::warn!("お気に入りを旧 API の記録へ写せません: {error}"),
        }
    }

    /// 旧 API で付け外ししたお気に入りを、この棚の利用者にも当てる。端末に新しい棚の許可が
    /// なければ棚の持ち主（デスクトップと同じ利用者）に当てる。値が同じなら書かない。
    /// 棚に無い作品は `false`。
    pub fn follow_favorite(
        &self,
        device: Option<&str>,
        provider: &str,
        key: &str,
        favorite: bool,
    ) -> crate::library::Result<bool> {
        let subject = device
            .and_then(|device| self.authority.subject_of(device, self.library_id))
            .map_or(self.library.id, Id::get);
        let Some(view) = self.library.reader_work_by_key(subject, provider, key)? else {
            return Ok(false);
        };
        let work = view.entity_id.get();
        let state = self.library.user_state(work, subject)?;
        if state.favorite == favorite {
            return Ok(true);
        }
        self.library.set_favorite(
            uuid::Uuid::now_v7(),
            work,
            subject,
            state.generation,
            favorite,
        )?;
        Ok(true)
    }
}
