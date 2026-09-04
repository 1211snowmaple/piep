//! 型定義されたAPIレスポンスモデル（pixivpy3.models の Rust 移植版）。
//!
//! すべての型は Serde を使用して JSON のシリアライズ/デシリアライズを行います。
//! Pixiv API は snake_case を返しますが、`WebviewNovel` のみ camelCase を使用します（HTML埋め込み由来）。

// we consider fields in these structs self-descriptive enough
#![allow(missing_docs)]

use chrono::{DateTime, FixedOffset};
use reqwest::StatusCode;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::pixiv_api::error::PixivError;
use log::{error, warn};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PixivUser {
    pub id: String,
    pub name: String,
    pub profile_image_urls: serde_json::Value,
}

// ----------------------------------------------------------------------------
// User & profile
// ----------------------------------------------------------------------------

/// プロフィール画像のURL（中サイズ）。
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileImageUrls {
    pub medium: String,
}

/// 一覧や詳細で返される基本的なユーザー情報。
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: u64,
    pub name: String,
    pub account: String,
    pub profile_image_urls: ProfileImageUrls,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub is_followed: Option<bool>,
    #[serde(default)]
    pub is_access_blocking_user: Option<bool>,
    #[serde(default)]
    pub is_accept_request: Option<bool>,
}

/// User info as shown in comments.
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentUser {
    pub id: u64,
    pub name: String,
    pub account: String,
    pub profile_image_urls: ProfileImageUrls,
}

/// User profile (detailed).
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub webpage: Option<String>,
    #[serde(default)]
    pub gender: Option<String>,
    #[serde(default)]
    pub birth: Option<String>,
    #[serde(default)]
    pub birth_day: Option<String>,
    #[serde(default)]
    pub birth_year: Option<i64>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub address_id: Option<i64>,
    #[serde(default)]
    pub country_code: Option<String>,
    #[serde(default)]
    pub job: Option<String>,
    #[serde(default)]
    pub job_id: Option<i64>,
    #[serde(default)]
    pub total_follow_users: Option<i64>,
    #[serde(default)]
    pub total_mypixiv_users: Option<i64>,
    #[serde(default)]
    pub total_illusts: Option<i64>,
    #[serde(default)]
    pub total_manga: Option<i64>,
    #[serde(default)]
    pub total_novels: Option<i64>,
    #[serde(default)]
    pub total_illust_bookmarks_public: Option<i64>,
    #[serde(default)]
    pub total_illust_series: Option<i64>,
    #[serde(default)]
    pub total_novel_series: Option<i64>,
    #[serde(default)]
    pub background_image_url: Option<String>,
    #[serde(default)]
    pub twitter_account: Option<String>,
    #[serde(default)]
    pub twitter_url: Option<String>,
    #[serde(default)]
    pub pawoo_url: Option<String>,
    #[serde(default)]
    pub is_premium: Option<bool>,
    #[serde(default)]
    pub is_using_custom_profile_image: Option<bool>,
}

/// Profile publicity settings.
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfilePublicity {
    #[serde(default)]
    pub gender: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub birth_day: Option<String>,
    #[serde(default)]
    pub birth_year: Option<String>,
    #[serde(default)]
    pub job: Option<String>,
    #[serde(default)]
    pub pawoo: Option<bool>,
}

/// User workspace info (tools, desk, etc.).
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    #[serde(default)]
    pub pc: Option<String>,
    #[serde(default)]
    pub monitor: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub scanner: Option<String>,
    #[serde(default)]
    pub tablet: Option<String>,
    #[serde(default)]
    pub mouse: Option<String>,
    #[serde(default)]
    pub printer: Option<String>,
    #[serde(default)]
    pub desktop: Option<String>,
    #[serde(default)]
    pub music: Option<String>,
    #[serde(default)]
    pub desk: Option<String>,
    #[serde(default)]
    pub chair: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub workspace_image_url: Option<String>,
}

/// 詳細なユーザー情報（ユーザー + プロフィール + ワークスペース）。user_detail レスポンスの移植。
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfoDetailed {
    pub user: UserInfo,
    pub profile: Profile,
    pub profile_publicity: ProfilePublicity,
    pub workspace: Workspace,
}

// ----------------------------------------------------------------------------
// Illust / image
// ----------------------------------------------------------------------------

/// イラストの画像URL（正方形、中、大）。
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrls {
    pub square_medium: String,
    pub medium: String,
    pub large: String,
}

/// Tag on an illustration.
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IllustrationTag {
    pub name: String,
    pub translated_name: Option<String>,
}

/// Series info (id and title).
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Series {
    pub id: u64,
    pub title: String,
}

/// Pixiv returns `{}` instead of `null` for empty objects.
///
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyObject {}

/// Series or empty object (Pixiv uses `{}` for "no series").
///
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SeriesOrEmpty {
    Series(Series),
    Empty(EmptyObject),
}

/// Single-page illust meta (original image URL).
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaSinglePage {
    pub original_image_url: Option<String>,
}

/// One page of a multi-page illust (image URLs).
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaPage {
    pub image_urls: ImageUrls,
}

/// イラスト情報（一覧または詳細）。
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IllustrationInfo {
    pub id: u64,
    pub title: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub image_urls: ImageUrls,
    pub caption: String,
    pub restrict: i32,
    pub user: UserInfo,
    pub tags: Vec<IllustrationTag>,
    pub tools: Vec<String>,
    pub create_date: DateTime<FixedOffset>,
    pub page_count: i32,
    pub width: i32,
    pub height: i32,
    pub sanity_level: i32,
    pub x_restrict: i32,
    pub series: Option<Series>,
    pub meta_single_page: MetaSinglePage,
    pub meta_pages: Vec<MetaPage>,
    pub total_view: i64,
    pub total_bookmarks: i64,
    pub is_bookmarked: bool,
    pub visible: bool,
    pub is_muted: bool,
    pub illust_ai_type: i32,
    pub illust_book_style: i32,
    #[serde(default)]
    pub total_comments: Option<i32>,
    #[serde(default)]
    pub restriction_attributes: Vec<String>,
}

/// Illust detail response (wraps single illust).
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IllustDetail {
    pub illust: IllustrationInfo,
}

/// Novel detail response (wraps single novel).
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NovelDetail {
    pub novel: NovelInfo,
}

// ----------------------------------------------------------------------------
// Novel
// ----------------------------------------------------------------------------

/// Tag on a novel.
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NovelTag {
    pub name: String,
    pub translated_name: Option<String>,
    pub added_by_uploaded_user: bool,
}

/// 小説情報（一覧または詳細）。
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NovelInfo {
    pub id: u64,
    pub title: String,
    pub caption: String,
    pub restrict: i32,
    pub x_restrict: i32,
    pub is_original: bool,
    pub image_urls: ImageUrls,
    pub create_date: String,
    pub tags: Vec<NovelTag>,
    pub page_count: i32,
    pub text_length: i64,
    pub user: UserInfo,
    pub series: SeriesOrEmpty,
    pub is_bookmarked: bool,
    pub total_bookmarks: i64,
    pub total_view: i64,
    pub visible: bool,
    pub total_comments: i32,
    pub is_muted: bool,
    pub is_mypixiv_only: bool,
    pub is_x_restricted: bool,
    pub novel_ai_type: i32,
    #[serde(default)]
    pub comment_access_control: Option<i32>,
    #[serde(default, deserialize_with = "lenient_option")]
    pub series_navigation: Option<SeriesNavigationOrEmpty>,
}

/// Recursive: comment or empty object (Pixiv uses `{}` for no parent).
///
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CommentOrEmpty {
    Comment(Box<Comment>),
    Empty(EmptyObject),
}

/// A single comment (illust or novel).
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub id: u64,
    pub comment: String,
    pub date: String,
    pub user: Option<CommentUser>,
    pub parent_comment: CommentOrEmpty,
}

/// Novel comments list with pagination.
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NovelComments {
    pub total_comments: i32,
    pub comments: Vec<Comment>,
    pub next_url: Option<String>,
    pub comment_access_control: i32,
}

/// Novel stats (like, bookmark, view counts).
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NovelRating {
    pub like: i64,
    pub bookmark: i64,
    pub view: i64,
}

/// Novel navigation entry in a series (order, title, cover).
///
/// 隣の話は、読めるとは限らない。
///
/// マイピク限定などで読めない回は `viewable: false` と断り書きだけが返り、
/// 題も表紙も null になる。読めない回を表せない型にしていたため、本文は
/// 手元にあるのに保存が丸ごと失敗していた。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NovelNavigationInfo {
    pub id: u64,
    pub viewable: bool,
    pub content_order: String,
    pub title: Option<String>,
    pub cover_url: Option<String>,
    pub viewable_message: Option<String>,
}

/// Series navigation。
///
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesNavigation {
    pub prev_novel: Option<NovelNavigationInfo>,
    pub next_novel: Option<NovelNavigationInfo>,
}

/// Series navigation or empty (Pixiv uses `{}` for none).
///
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SeriesNavigationOrEmpty {
    Info(Box<SeriesNavigation>),
    Empty(EmptyObject),
}

/// 読めなかった飾りは、捨てて先へ進む。
///
/// 前後の話への案内は本文の取得には要らない飾りだが、取得元が返す形は
/// ときどき変わる。ここで型に合わないだけで、本文まで届いている保存が
/// 丸ごと失敗していた。形が変わったことは記録に残し、値だけを手放す。
fn lenient_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: DeserializeOwned,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    if value.is_null() {
        return Ok(None);
    }
    match serde_json::from_value(value) {
        Ok(parsed) => Ok(Some(parsed)),
        Err(error) => {
            warn!(
                "{} として読めない値を無視しました: {error}",
                std::any::type_name::<T>()
            );
            Ok(None)
        }
    }
}

/// webview HTML埋め込みから取得される小説データ。camelCaseを使用します。
///
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebviewNovel {
    pub id: String,
    pub title: String,
    pub series_id: Option<String>,
    pub series_title: Option<String>,
    pub series_is_watched: Option<bool>,
    pub user_id: String,
    pub cover_url: String,
    pub tags: Vec<String>,
    pub caption: String,
    pub cdate: String,
    pub rating: NovelRating,
    pub text: String,
    pub marker: Option<String>,
    pub illusts: serde_json::Value,
    pub images: serde_json::Value,
    #[serde(default, deserialize_with = "lenient_option")]
    pub series_navigation: Option<SeriesNavigationOrEmpty>,
    pub glossary_items: serde_json::Value,
    pub replaceable_item_ids: serde_json::Value,
    pub ai_type: i32,
    pub is_original: bool,
}

// ----------------------------------------------------------------------------
// Response wrappers (illust/user/novel lists)
// ----------------------------------------------------------------------------

/// User bookmarked novels (paged).
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserBookmarksNovel {
    pub novels: Vec<NovelInfo>,
    pub next_url: Option<String>,
}

/// User novels list (paged).
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserNovels {
    pub user: UserInfo,
    pub novels: Vec<NovelInfo>,
    pub next_url: Option<String>,
}

/// Novel search result (paged).
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchNovel {
    pub novels: Vec<NovelInfo>,
    pub next_url: Option<String>,
    pub search_span_limit: i32,
    pub show_ai: bool,
}

/// Illust search result (paged).
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchIllustrations {
    pub illusts: Vec<IllustrationInfo>,
    pub next_url: Option<String>,
    pub search_span_limit: i32,
    pub show_ai: bool,
}

/// User bookmarked illusts (paged).
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserBookmarksIllustrations {
    pub illusts: Vec<IllustrationInfo>,
    pub next_url: Option<String>,
}

/// User preview (user + sample illusts/novels) in following/follower lists.
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPreview {
    pub user: UserInfo,
    pub illusts: Vec<IllustrationInfo>,
    pub novels: Vec<NovelInfo>,
    pub is_muted: bool,
}

/// User following list (paged).
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserFollowing {
    pub user_previews: Vec<UserPreview>,
    pub next_url: Option<String>,
}

/// User illusts list (paged).
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserIllustrations {
    pub user: UserInfo,
    pub illusts: Vec<IllustrationInfo>,
    pub next_url: Option<String>,
}

/// OAuth token refresh response (access_token, expires_in, etc.).
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRefreshResult {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<i64>,
}

// ----------------------------------------------------------------------------
// Utils
// ----------------------------------------------------------------------------

/// Parsed JSON response from API (same as Python `ParsedJson` / `JsonDict`).
/// In Rust we use `serde_json::Value` for attribute-like access via indexing.
///
pub type ParsedJson = serde_json::Value;

/// 通常の Pixiv API JSON はページング済みであるため十分な余裕を持たせつつ、
/// 圧縮爆弾や異常レスポンスでメモリを無制限に消費しない上限。
pub(crate) const MAX_PIXIV_JSON_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

/// 小説 webview は本文を HTML 内に含むため、通常 JSON より大きな上限を許可する。
pub(crate) const MAX_PIXIV_WEBVIEW_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const MAX_INITIAL_RESPONSE_CAPACITY: usize = 64 * 1024;

// ----------------------------------------------------------------------------
// Parsing
// ----------------------------------------------------------------------------

/// レスポンスボディが `"error"` キーを持つ JSON オブジェクトである場合に true を返します。
///
pub fn is_error_response(res_body: &str) -> bool {
    if let Ok(parsed) = serde_json::from_str::<ParsedJson>(res_body) {
        return parsed.get("error").is_some();
    }
    false
}

/// 応答の中身が取得制限を告げているか。
///
/// pixiv は制限を 429 で返すとは限らず、200 のまま `error.message` に
/// "Rate Limit" を載せて返してくることがある。状態番号だけを見ていると、
/// 間を空ければ通るものを「壊れた応答」として扱ってしまう。
pub fn pixiv_body_is_rate_limited(res_body: &str) -> bool {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(res_body) else {
        return false;
    };
    let Some(error) = parsed.get("error") else {
        return false;
    };
    ["message", "reason", "user_message"]
        .iter()
        .filter_map(|key| error.get(*key).and_then(|value| value.as_str()))
        .any(|text| {
            let text = text.to_ascii_lowercase();
            text.contains("rate limit") || text.contains("too many requests")
        })
}

/// レスポンスボディを型 `T` にデシリアライズします。失敗した場合は `PixivError::Serde` を返します。
///
pub fn parse_into<T: DeserializeOwned, S: AsRef<str> + Into<String>>(
    res_body: S,
) -> Result<T, PixivError> {
    match serde_json::from_str(res_body.as_ref()) {
        Ok(parsed) => Ok(parsed),
        Err(error) => {
            // 文面から本文を外した分、記録には残す。読めなかった理由は本文の
            // 中にしかないが、それを追うのは画面の前ではなくログの側の仕事。
            let head: String = res_body.as_ref().chars().take(400).collect();
            error!(
                "{} として読めませんでした: {error}, body(先頭): {head}",
                std::any::type_name::<T>()
            );
            Err(PixivError::Serde {
                error,
                body: res_body.into(),
            })
        }
    }
}

/// Content-Length と実際に受信した（展開後の）バイト数の両方を検査して読み込む。
pub(crate) async fn read_response_text_limited(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<(StatusCode, String), PixivError> {
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(PixivError::ResponseTooLarge {
            limit_bytes: max_bytes,
        });
    }

    let initial_capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0)
        .min(max_bytes)
        .min(MAX_INITIAL_RESPONSE_CAPACITY);
    let mut body = Vec::with_capacity(initial_capacity);
    while let Some(chunk) = response.chunk().await? {
        let next_len = body
            .len()
            .checked_add(chunk.len())
            .filter(|length| *length <= max_bytes)
            .ok_or(PixivError::ResponseTooLarge {
                limit_bytes: max_bytes,
            })?;
        body.reserve(next_len - body.len());
        body.extend_from_slice(&chunk);
    }

    // Pixiv の JSON/HTML は UTF-8。従来の reqwest::Response::text と同様、
    // 不正バイトだけを置換して後段の構造検証に委ねる。
    Ok((status, String::from_utf8_lossy(&body).into_owned()))
}

/// レスポンスボディを読み込み、`T` にデシリアライズします。
/// レートリミット (429)、未検出 (404)、および API エラーペイロードを適切に処理します。
///
pub async fn parse_response_into<T: DeserializeOwned>(
    response: reqwest::Response,
) -> Result<T, PixivError> {
    let (status, body) =
        read_response_text_limited(response, MAX_PIXIV_JSON_RESPONSE_BYTES).await?;

    match status {
        _ if status == StatusCode::TOO_MANY_REQUESTS || pixiv_body_is_rate_limited(&body) => {
            // 本文をそのまま流していた。応答は 32MB まで受けるので、一件で
            // ログを埋めうる。長さを切る側は既にあるので、そちらを通す。
            crate::pixiv_api::error::log_response_body("API rate limited", &body);
            Err(PixivError::RateLimited { body })
        }
        StatusCode::NOT_FOUND => {
            crate::pixiv_api::error::log_response_body("API resource not found", &body);
            Err(PixivError::NotFound { body })
        }
        _ => {
            if !status.is_success() {
                warn!("API request returned non-success status: {status}, parse body anyway");
            }

            parse_into(body).map_err(|e| {
                // If it failed to parse, check if it's an error response
                if let PixivError::Serde { error, body } = e {
                    if is_error_response(&body) {
                        crate::pixiv_api::error::log_response_body(
                            "pixiv がエラーを返しました",
                            &body,
                        );
                        PixivError::ErrResponse { body }
                    } else {
                        PixivError::Serde { error, body }
                    }
                } else {
                    e
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn response_from_wire(wire_response: &'static [u8]) -> reqwest::Response {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await;
            stream.write_all(wire_response).await.unwrap();
        });
        let response = reqwest::get(format!("http://{address}/response"))
            .await
            .unwrap();
        server.await.unwrap();
        response
    }

    #[tokio::test]
    async fn limited_response_rejects_declared_oversize_body() {
        let response = response_from_wire(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nabcde",
        )
        .await;

        let error = read_response_text_limited(response, 4).await.unwrap_err();
        assert!(matches!(
            error,
            PixivError::ResponseTooLarge { limit_bytes: 4 }
        ));
    }

    #[tokio::test]
    async fn limited_response_rejects_chunked_oversize_body() {
        let response = response_from_wire(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nabcde\r\n0\r\n\r\n",
        )
        .await;

        let error = read_response_text_limited(response, 4).await.unwrap_err();
        assert!(matches!(
            error,
            PixivError::ResponseTooLarge { limit_bytes: 4 }
        ));
    }

    #[test]
    fn deserialize_user_info() {
        let json = r#"{
            "id": 11,
            "name": "pixiv事務局",
            "account": "pixiv",
            "profile_image_urls": { "medium": "https://example.com/img.jpg" },
            "comment": "hello",
            "is_followed": false
        }"#;
        let user: UserInfo = serde_json::from_str(json).unwrap();
        assert_eq!(user.id, 11);
        assert_eq!(user.name, "pixiv事務局");
        assert_eq!(user.account, "pixiv");
        assert_eq!(user.comment.as_deref(), Some("hello"));
        assert_eq!(user.is_followed, Some(false));
    }

    #[test]
    fn deserialize_user_detail_allows_nullable_profile_fields() {
        let json = r#"{
            "user": {
                "id": 12729442,
                "name": "みゆ",
                "account": "mcntr",
                "profile_image_urls": {
                    "medium": "https://example.com/profile.jpg"
                },
                "comment": "",
                "is_followed": true,
                "is_access_blocking_user": false
            },
            "profile": {
                "webpage": null,
                "gender": "male",
                "birth": "1991-11-11",
                "birth_day": "11-11",
                "birth_year": 1991,
                "region": "日本",
                "address_id": 0,
                "country_code": "",
                "job": "IT関係",
                "job_id": 1,
                "total_follow_users": 120,
                "total_mypixiv_users": 5,
                "total_illusts": 0,
                "total_manga": 0,
                "total_novels": 238,
                "total_illust_bookmarks_public": 67,
                "total_illust_series": 0,
                "total_novel_series": 13,
                "background_image_url": null,
                "twitter_account": "",
                "twitter_url": "",
                "pawoo_url": null,
                "is_premium": false,
                "is_using_custom_profile_image": true
            },
            "profile_publicity": {
                "gender": "public",
                "region": "public",
                "birth_day": "public",
                "birth_year": "public",
                "job": "public",
                "pawoo": true
            },
            "workspace": {
                "pc": "",
                "monitor": "",
                "tool": "",
                "scanner": "",
                "tablet": "",
                "mouse": "",
                "printer": "",
                "desktop": "",
                "music": "",
                "desk": "",
                "chair": "",
                "comment": "",
                "workspace_image_url": null
            }
        }"#;

        let detail: UserInfoDetailed = serde_json::from_str(json).unwrap();
        assert_eq!(detail.user.id, 12729442);
        assert_eq!(detail.profile.webpage, None);
        assert_eq!(detail.profile.background_image_url, None);
        assert_eq!(detail.profile.pawoo_url, None);
        assert_eq!(detail.profile.total_novels, Some(238));
    }

    #[test]
    fn deserialize_illust_detail() {
        let json = r#"{
            "illust": {
                "id": 12345,
                "title": "test illust",
                "type": "illust",
                "image_urls": {
                    "square_medium": "https://example.com/sq.jpg",
                    "medium": "https://example.com/m.jpg",
                    "large": "https://example.com/l.jpg"
                },
                "caption": "caption",
                "restrict": 0,
                "user": {
                    "id": 1,
                    "name": "user",
                    "account": "acc",
                    "profile_image_urls": { "medium": "https://example.com/p.jpg" }
                },
                "tags": [],
                "tools": [],
                "create_date": "2024-01-01T12:00:00+09:00",
                "page_count": 1,
                "width": 800,
                "height": 600,
                "sanity_level": 2,
                "x_restrict": 0,
                "meta_single_page": {},
                "meta_pages": [],
                "total_view": 100,
                "total_bookmarks": 10,
                "is_bookmarked": false,
                "visible": true,
                "is_muted": false,
                "illust_ai_type": 0,
                "illust_book_style": 0
            }
        }"#;
        let detail: IllustDetail = serde_json::from_str(json).unwrap();
        assert_eq!(detail.illust.id, 12345);
        assert_eq!(detail.illust.title, "test illust");
        assert_eq!(detail.illust.page_count, 1);
    }

    #[test]
    fn deserialize_empty_series_as_empty_object() {
        let json = r#"{}"#;
        let result: SeriesOrEmpty = serde_json::from_str(json).unwrap();
        assert!(matches!(result, SeriesOrEmpty::Empty(_)));
    }

    #[test]
    fn deserialize_series_as_series_variant() {
        let json = r#"{"id": 1, "title": "My Series"}"#;
        let result: SeriesOrEmpty = serde_json::from_str(json).unwrap();
        match &result {
            SeriesOrEmpty::Series(s) => {
                assert_eq!(s.id, 1);
                assert_eq!(s.title, "My Series");
            }
            SeriesOrEmpty::Empty(_) => panic!("expected Series variant"),
        }
    }

    /// pixiv は取得制限を 429 で返すとは限らない。200 のまま本文にだけ
    /// Rate Limit と書いて返してくる経路があり、状態番号しか見ていないと
    /// 「やり直せば通るもの」を壊れた応答として扱ってしまう。
    #[test]
    fn a_rate_limit_hidden_in_a_success_body_is_still_a_rate_limit() {
        assert!(pixiv_body_is_rate_limited(
            r#"{"error":{"user_message":"","message":"Rate Limit","reason":""}}"#
        ));
        assert!(pixiv_body_is_rate_limited(
            r#"{"error":{"message":"","reason":"Too Many Requests"}}"#
        ));
    }

    #[test]
    fn ordinary_errors_and_payloads_are_not_mistaken_for_a_rate_limit() {
        assert!(!pixiv_body_is_rate_limited(
            r#"{"error":{"message":"Invalid access token"}}"#
        ));
        assert!(!pixiv_body_is_rate_limited(r#"{"novel":{"id":1}}"#));
        assert!(!pixiv_body_is_rate_limited("not json"));
        // 本文に出てくるだけの語では反応しない。
        assert!(!pixiv_body_is_rate_limited(
            r#"{"novel":{"title":"rate limit の話"}}"#
        ));
    }

    #[test]
    fn is_error_response_detects_error() {
        let body = r#"{"error": {"message": "invalid token"}}"#;
        assert!(is_error_response(body));
    }

    #[test]
    fn is_error_response_no_error() {
        let body = r#"{"id": 1, "title": "test"}"#;
        assert!(!is_error_response(body));
    }

    #[test]
    fn parse_into_success() {
        let body = r#"{"id": 1, "title": "test"}"#;
        let result: serde_json::Value = parse_into(body.to_string()).unwrap();
        assert_eq!(result["id"], 1);
        assert_eq!(result["title"], "test");
    }

    #[test]
    fn parse_into_invalid_json_returns_serde_error() {
        let body = "not json";
        let err = parse_into::<serde_json::Value, _>(body.to_string()).unwrap_err();
        assert!(matches!(err, PixivError::Serde { .. }));
    }

    /// 本文以外を埋めた webview 応答。`series_navigation` の形だけを差し替える。
    fn webview_novel_json(series_navigation: &str) -> String {
        format!(
            r#"{{
                "id": "19398164",
                "title": "題",
                "seriesId": "1234",
                "seriesTitle": "連作",
                "seriesIsWatched": false,
                "userId": "99",
                "coverUrl": "https://example.invalid/cover.jpg",
                "tags": ["tag"],
                "caption": "",
                "cdate": "2023-05-27T00:00:01+09:00",
                "rating": {{ "like": 1, "bookmark": 2, "view": 3 }},
                "text": "本文",
                "marker": null,
                "illusts": {{}},
                "images": {{}},
                "seriesNavigation": {series_navigation},
                "glossaryItems": [],
                "replaceableItemIds": [],
                "aiType": 1,
                "isOriginal": false
            }}"#
        )
    }

    #[test]
    fn webview_novel_accepts_unviewable_neighbor() {
        // マイピク限定の隣の話。題も表紙も返らないが、本文は保存できる。
        let json = webview_novel_json(
            r##"{
                "nextNovel": { "id": 19761392, "viewable": false, "viewableMessage": "#20はマイピク限定作品です", "contentOrder": "20", "title": null, "coverUrl": null },
                "prevNovel": { "id": 17676921, "viewable": true, "contentOrder": "18", "title": "#18", "coverUrl": "https://example.invalid/18.jpg", "viewableMessage": null }
            }"##,
        );
        let novel: WebviewNovel = serde_json::from_str(&json).unwrap();
        let Some(SeriesNavigationOrEmpty::Info(navigation)) = novel.series_navigation else {
            panic!("前後の話の案内が読めていない");
        };
        let next = navigation.next_novel.unwrap();
        assert!(!next.viewable);
        assert_eq!(next.title, None);
        assert_eq!(next.cover_url, None);
        assert_eq!(navigation.prev_novel.unwrap().title.as_deref(), Some("#18"));
    }

    #[test]
    fn webview_novel_accepts_empty_navigation() {
        // 前後が無いときの `{}`。前後どちらも無い案内として読める。
        let novel: WebviewNovel = serde_json::from_str(&webview_novel_json("{}")).unwrap();
        let Some(SeriesNavigationOrEmpty::Info(navigation)) = novel.series_navigation else {
            panic!("空の案内が読めていない");
        };
        assert!(navigation.prev_novel.is_none());
        assert!(navigation.next_novel.is_none());
    }

    #[test]
    fn webview_novel_survives_unknown_navigation_shape() {
        // 案内の形が変わっても、本文は手元にある。飾りだけを捨てる。
        let novel: WebviewNovel =
            serde_json::from_str(&webview_novel_json(r#"["変わった形"]"#)).unwrap();
        assert!(novel.series_navigation.is_none());
        assert_eq!(novel.text, "本文");
    }

    #[test]
    fn serde_error_message_leaves_out_the_body() {
        let err = parse_into::<WebviewNovel, _>(
            webview_novel_json(r#"{}"#).replace("\"title\": \"題\",", ""),
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("読み取れませんでした"), "{message}");
        assert!(!message.contains("本文"), "{message}");
    }

    #[test]
    fn deserialize_token_refresh_result() {
        let json = r#"{
            "access_token": "abc123",
            "refresh_token": "xyz789",
            "expires_in": 3600
        }"#;
        let result: TokenRefreshResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.access_token, "abc123");
        assert_eq!(result.refresh_token.as_deref(), Some("xyz789"));
        assert_eq!(result.expires_in, Some(3600));
    }
}
