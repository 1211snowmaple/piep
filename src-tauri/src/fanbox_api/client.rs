use reqwest::header::{HeaderMap, HeaderValue as HV, ACCEPT, COOKIE, ORIGIN, REFERER, USER_AGENT};
use reqwest::{Client, StatusCode};
use serde::de::DeserializeOwned;
use std::time::Duration;

use crate::auth::fanbox::validate_creator_id;
use crate::downloader::candidate_list::CandidateList;
use crate::fanbox_api::error::FanboxError;
use crate::fanbox_api::models::*;

const FANBOX_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_FANBOX_JSON_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_INITIAL_RESPONSE_CAPACITY: usize = 64 * 1024;
/// 一覧を辿るときに、ページとページの間で空ける時間。
///
/// 長く続けている creator の履歴は数十ページになる。続けざまに取りに行けば
/// 断られ、断られた時点で一覧そのものが手に入らなくなる。更新ジョブの
/// 800ms と揃えてある。
const FANBOX_PAGE_DELAY: Duration = Duration::from_millis(800);

fn build_api_client(
    connect_timeout: Duration,
    request_timeout: Duration,
) -> Result<Client, reqwest::Error> {
    Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        // API endpoints are not expected to redirect. Refusing redirects
        // guarantees a session header can never be replayed to a new URL.
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

async fn read_response_text_limited(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<(StatusCode, String), FanboxError> {
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes as u64)
    {
        return Err(FanboxError::Other(format!(
            "FANBOX API response exceeded the {max_bytes}-byte safety limit"
        )));
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
            .ok_or_else(|| {
                FanboxError::Other(format!(
                    "FANBOX API response exceeded the {max_bytes}-byte safety limit"
                ))
            })?;
        body.reserve(next_len - body.len());
        body.extend_from_slice(&chunk);
    }
    let body = String::from_utf8(body)
        .map_err(|_| FanboxError::Other("FANBOX の応答が UTF-8 ではありません".into()))?;
    Ok((status, body))
}

/// Fanbox API クライアント
#[derive(Clone)]
pub struct FanboxAPI {
    client: Client,
    cookie: String,
    user_agent: String,
}

impl FanboxAPI {
    /// 新規クライアントの作成
    /// **組み立ての失敗で落ちない。** 詳しい理由は `AppPixivAPI::new_with` に。
    pub fn new(cookie: String, user_agent: String) -> Result<Self, FanboxError> {
        let client = build_api_client(
            FANBOX_CONNECT_TIMEOUT,
            crate::advanced_settings::source_timeout(),
        )?;
        Ok(Self {
            client,
            cookie,
            user_agent,
        })
    }

    /// 共通ヘッダーを生成する
    fn headers(&self) -> Result<HeaderMap, FanboxError> {
        let mut headers = HeaderMap::new();
        headers.insert(ORIGIN, HV::from_static("https://www.fanbox.cc"));
        headers.insert(REFERER, HV::from_static("https://www.fanbox.cc/"));
        headers.insert(ACCEPT, HV::from_static("application/json, text/plain, */*"));

        let cookie_val = HV::from_str(&self.cookie)
            .map_err(|e| FanboxError::Other(format!("Invalid Cookie format: {}", e)))?;
        headers.insert(COOKIE, cookie_val);

        let ua_val = HV::from_str(&self.user_agent)
            .map_err(|e| FanboxError::Other(format!("Invalid User-Agent format: {}", e)))?;
        headers.insert(USER_AGENT, ua_val);

        Ok(headers)
    }

    /// 一覧のページを、断られることを前提に取りに行く。
    ///
    /// 取得制限は「今は無理」であって「もう無い」ではない。`?` で即座に
    /// 諦めていたころは、一度断られただけでその creator の一覧が丸ごと
    /// 手に入らず、新作の取りこぼしになっていた。
    async fn api_get_paged<T: DeserializeOwned>(&self, url: &str) -> Result<T, FanboxError> {
        self.api_get_once(url).await
    }

    /// 低レベル GET リクエストの共通ハンドラ。
    ///
    /// 取得制限だけは、一覧と同じように待って試し直す。一覧（`api_get_paged`）
    /// にはこれがあり、単発の投稿・クリエイター取得には無かった。断られる
    /// 理由は同じなのに、扱いが場所によって違っていた。
    async fn api_get<T: DeserializeOwned>(&self, url: &str) -> Result<T, FanboxError> {
        // Retry ownership belongs to the job. Nested retries multiplied requests.
        self.api_get_once(url).await
    }

    async fn api_get_once<T: DeserializeOwned>(&self, url: &str) -> Result<T, FanboxError> {
        let body = self.api_get_body(url).await?;
        serde_json::from_str(&body).map_err(|error| FanboxError::Serde { error })
    }
    async fn api_get_body(&self, url: &str) -> Result<String, FanboxError> {
        let url = allowed_api_url(url)?;
        let headers = self.headers()?;
        crate::downloader::pacing::FANBOX.wait().await;
        let res = self.client.get(url).headers(headers).send().await?;
        if res.status() == StatusCode::TOO_MANY_REQUESTS {
            crate::downloader::pacing::FANBOX
                .defer(crate::downloader::pacing::retry_after(res.headers()))
                .await;
        }

        let (status, body) =
            read_response_text_limited(res, MAX_FANBOX_JSON_RESPONSE_BYTES).await?;

        match status {
            StatusCode::TOO_MANY_REQUESTS => {
                log::warn!("Fanbox API rate limited ({} response bytes)", body.len());
                Err(FanboxError::RateLimited)
            }
            StatusCode::NOT_FOUND => {
                log::warn!("Fanbox resource not found ({} response bytes)", body.len());
                Err(FanboxError::NotFound)
            }
            StatusCode::FORBIDDEN if is_cloudflare_challenge(&body) => {
                crate::downloader::pacing::FANBOX
                    .defer(Duration::from_secs(60))
                    .await;
                log::warn!("Fanbox API temporarily refused an automated request");
                Err(FanboxError::ChallengeRequired)
            }
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                log::warn!(
                    "Fanbox API authentication required ({} response bytes)",
                    body.len()
                );
                Err(FanboxError::NoAuth)
            }
            _ => {
                if !status.is_success() {
                    log::warn!(
                        "Fanbox API returned non-success status: {}, response bytes: {}",
                        status,
                        body.len()
                    );
                    return Err(FanboxError::ApiError {
                        status: status.as_u16(),
                    });
                }

                Ok(body)
            }
        }
    }

    // --- エンドポイント実装 ---

    /// A single lightweight permission check; never probe every inaccessible post.
    pub async fn supporting_fees(
        &self,
    ) -> Result<std::collections::HashMap<String, u64>, FanboxError> {
        let response: FanboxResponse<Vec<serde_json::Value>> = self
            .api_get("https://api.fanbox.cc/plan.listSupporting")
            .await?;
        let mut fees = std::collections::HashMap::new();
        for plan in response.body {
            let creator = plan
                .get("creatorId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| FanboxError::Other("FANBOXの支援プランを解釈できません".into()))?;
            let fee = plan
                .get("fee")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| FanboxError::Other("FANBOXの支援額を解釈できません".into()))?;
            let entry = fees.entry(creator.to_string()).or_insert(0);
            *entry = (*entry).max(fee);
        }
        Ok(fees)
    }

    /// 1. セッション（Cookie）が有効かチェック
    pub async fn check_session(&self) -> Result<(), FanboxError> {
        // 未読通知数を取得する軽量なエンドポイントでテスト
        let url = "https://api.fanbox.cc/bell.countUnread";
        let _res: FanboxResponse<serde_json::Value> = self.api_get(url).await?;
        Ok(())
    }

    /// 2. クリエイター情報取得（解釈は保存の側が生の JSON から行う）
    pub async fn get_creator_raw(&self, creator_id: &str) -> Result<String, FanboxError> {
        validate_creator_id(creator_id)?;
        let url = request_url("creator.get", &[("creatorId", creator_id)]);
        self.api_get_body(url.as_str()).await
    }

    /// 投稿APIは追加フィールドや投稿種別ごとの差が大きいため、保存経路では
    /// 構造を失わない生のJSONを利用する。
    pub async fn get_post_value(&self, post_id: &str) -> Result<serde_json::Value, FanboxError> {
        let url = request_url("post.info", &[("postId", post_id)]);
        let resp: FanboxResponse<serde_json::Value> = self.api_get(url.as_str()).await?;
        crate::fanbox_api::payload::into_post(resp.body).ok_or_else(|| {
            FanboxError::Other(
                "FANBOX投稿応答の構造を解釈できませんでした（応答形式が変わった可能性があります）"
                    .to_string(),
            )
        })
    }

    /// クリエイターの投稿を並べ、件数の上限で打ち切ったかを返す。
    pub async fn get_all_creator_posts(
        &self,
        creator_id: &str,
    ) -> Result<CandidateList<FanboxPost>, FanboxError> {
        self.get_creator_posts_since(creator_id, None).await
    }

    pub async fn get_creator_posts_since(
        &self,
        creator_id: &str,
        stop_source_id: Option<&str>,
    ) -> Result<CandidateList<FanboxPost>, FanboxError> {
        validate_creator_id(creator_id)?;
        let mut candidates = CandidateList::default();
        let mut seen_ids = std::collections::HashSet::new();

        // 1. paginateCreatorでページURLの一覧を取得
        let url = request_url("post.paginateCreator", &[("creatorId", creator_id)]);
        let resp: FanboxResponse<serde_json::Value> = self.api_get_paged(url.as_str()).await?;

        // Current FANBOX responses wrap the URL array as `{ body: { pageUrls: [...] } }`.
        // Older responses returned the array directly, so accept both deliberately.
        if let Some(page_urls) = pagination_page_urls(&resp.body) {
            if page_urls.len() > 500 {
                return Err(FanboxError::Other(
                    "FANBOX history exceeded the 500-page safety limit".to_string(),
                ));
            }
            for (index, page_url) in page_urls.iter().enumerate() {
                if index > 0 {
                    tokio::time::sleep(FANBOX_PAGE_DELAY).await;
                }
                log::info!("Fetching page: {}", page_url);
                let page: FanboxResponse<serde_json::Value> = self.api_get_paged(page_url).await?;
                let (posts, _) = decode_post_list(page.body)?;
                if append_fanbox_posts(&mut candidates, &mut seen_ids, posts, stop_source_id) {
                    return Ok(candidates);
                }
            }
            if !page_urls.is_empty() {
                return Ok(candidates);
            }
        } else if post_list_shape(&resp.body) {
            // Some historical versions returned posts from paginateCreator itself.
            let (posts, _) = decode_post_list(resp.body)?;
            append_fanbox_posts(&mut candidates, &mut seen_ids, posts, stop_source_id);
            return Ok(candidates);
        }

        // 2. 究極のフォールバック (URL配列が得られなかった場合)
        log::warn!("Pagination empty or unrecognized. Falling back to listCreator API.");
        let fallback_url = request_url(
            "post.listCreator",
            &[("creatorId", creator_id), ("limit", "100")],
        );

        // Follow nextUrl as well. The previous one-shot fallback silently truncated
        // creators with more than 100 posts when paginateCreator changed shape.
        let mut next_url = Some(fallback_url.to_string());
        let mut seen_urls = std::collections::HashSet::new();
        let mut page_index = 0usize;
        while let Some(page_url) = next_url.take() {
            if page_index >= 500 {
                return Err(FanboxError::Other(
                    "FANBOX history exceeded the 500-page safety limit".to_string(),
                ));
            }
            if !seen_urls.insert(page_url.clone()) {
                return Err(FanboxError::Other(
                    "FANBOX pagination returned the same page repeatedly".to_string(),
                ));
            }
            if page_index > 0 {
                tokio::time::sleep(FANBOX_PAGE_DELAY).await;
            }
            let page: FanboxResponse<serde_json::Value> = self.api_get_paged(&page_url).await?;
            let (posts, following_url) = decode_post_list(page.body)?;
            if append_fanbox_posts(&mut candidates, &mut seen_ids, posts, stop_source_id) {
                break;
            }
            next_url = following_url;
            page_index += 1;
        }
        Ok(candidates)
    }
}

fn pagination_page_urls(body: &serde_json::Value) -> Option<Vec<String>> {
    let array = body
        .get("pageUrls")
        .and_then(|value| value.as_array())
        .or_else(|| {
            body.as_array()
                .filter(|items| items.is_empty() || items.iter().all(serde_json::Value::is_string))
        })?;
    Some(
        array
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect(),
    )
}

fn post_list_shape(body: &serde_json::Value) -> bool {
    body.as_array()
        .is_some_and(|items| items.first().is_some_and(serde_json::Value::is_object))
        || body
            .as_object()
            .is_some_and(|object| object.contains_key("posts") || object.contains_key("items"))
}

fn decode_post_list(
    body: serde_json::Value,
) -> Result<(Vec<FanboxPost>, Option<String>), FanboxError> {
    if body.is_array() {
        let posts = serde_json::from_value(body).map_err(|error| FanboxError::Serde { error })?;
        return Ok((posts, None));
    }
    if !post_list_shape(&body) {
        return Err(FanboxError::Other(
            "FANBOX投稿一覧の応答形式を解釈できませんでした".to_string(),
        ));
    }
    let list: FanboxPostList =
        serde_json::from_value(body).map_err(|error| FanboxError::Serde { error })?;
    Ok((list.items, list.next_url))
}

fn append_fanbox_posts(
    output: &mut CandidateList<FanboxPost>,
    seen_ids: &mut std::collections::HashSet<String>,
    posts: Vec<FanboxPost>,
    stop_source_id: Option<&str>,
) -> bool {
    output.append_page(
        posts,
        seen_ids,
        |post| post.id.clone(),
        |id| stop_source_id.is_some_and(|stop| stop == id),
    )
}

fn request_url(endpoint: &str, pairs: &[(&str, &str)]) -> reqwest::Url {
    let mut url =
        reqwest::Url::parse(&format!("https://api.fanbox.cc/{endpoint}")).expect("fixed API URL");
    url.query_pairs_mut().extend_pairs(pairs.iter().copied());
    url
}

fn allowed_api_url(raw: &str) -> Result<reqwest::Url, FanboxError> {
    let url = reqwest::Url::parse(raw)
        .map_err(|e| FanboxError::Other(format!("Invalid FANBOX API URL: {e}")))?;
    let allowed = url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && url.host_str() == Some("api.fanbox.cc");
    if !allowed {
        return Err(FanboxError::Other(
            "Refusing to send FANBOX credentials to an untrusted host".to_string(),
        ));
    }
    Ok(url)
}

fn is_cloudflare_challenge(body: &str) -> bool {
    let body = body.to_ascii_lowercase();
    [
        "cf-chl-",
        "challenge-platform",
        "just a moment",
        "attention required",
        "cloudflare ray id",
    ]
    .iter()
    .any(|marker| body.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn request_ids_are_encoded_as_single_query_values() {
        let id = "a&other=1?/ %2F#fragment";
        for (endpoint, pairs) in [
            ("post.info", vec![("postId", id)]),
            ("creator.get", vec![("creatorId", id)]),
            ("post.paginateCreator", vec![("creatorId", id)]),
            (
                "post.listCreator",
                vec![("creatorId", id), ("limit", "100")],
            ),
        ] {
            let url = request_url(endpoint, &pairs);
            let decoded: Vec<_> = url
                .query_pairs()
                .map(|(key, value)| (key.into_owned(), value.into_owned()))
                .collect();
            assert_eq!(
                decoded,
                pairs
                    .iter()
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .collect::<Vec<_>>()
            );
            assert_eq!(url.fragment(), None);
        }
    }

    #[tokio::test]
    async fn rejects_invalid_creator_ids_before_building_headers() {
        // 不正な Cookie で、入力の検査をすり抜けても通信させない。
        let api = FanboxAPI::new("\n".into(), "test".into()).unwrap();
        for id in ["a&b", "a?b", "a/b", "a%b", "a b", "", &"a".repeat(65)] {
            let error = api.get_all_creator_posts(id).await.unwrap_err();
            assert_eq!(
                error.to_string(),
                "クリエイターIDの形式が正しくありません",
                "{id:?}"
            );
        }
    }

    #[test]
    fn creator_list_keeps_exact_limit_and_marks_only_an_additional_post() {
        use crate::downloader::candidate_list::CANDIDATE_LIMIT;

        let post = |id: usize| -> FanboxPost {
            serde_json::from_value(json!({
                "id": id.to_string(), "title": "投稿", "creatorId": "creator",
                "publishedDatetime": "", "updatedDatetime": "",
            }))
            .unwrap()
        };
        let mut candidates = CandidateList::default();
        let mut seen = std::collections::HashSet::new();
        let page = (1..=CANDIDATE_LIMIT).map(post).collect();
        assert!(!append_fanbox_posts(&mut candidates, &mut seen, page, None));
        assert_eq!(candidates.items.len(), CANDIDATE_LIMIT);
        assert!(!candidates.truncated);

        let next_page = vec![post(1), post(CANDIDATE_LIMIT + 1)];
        assert!(append_fanbox_posts(
            &mut candidates,
            &mut seen,
            next_page,
            None
        ));
        assert_eq!(candidates.items.len(), CANDIDATE_LIMIT);
        assert_eq!(
            candidates.items.last().unwrap().id,
            CANDIDATE_LIMIT.to_string()
        );
        assert!(candidates.truncated);
    }

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
    async fn stored_json_keeps_outer_whitespace_and_rejects_invalid_utf8() {
        let response = response_from_wire(
            b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n \n{\"future\":9007199254740993}\n ",
        )
        .await;
        assert_eq!(
            read_response_text_limited(response, 100).await.unwrap().1,
            " \n{\"future\":9007199254740993}\n "
        );
        let response =
            response_from_wire(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{\"name\":\"\xff\"}")
                .await;
        assert!(read_response_text_limited(response, 100).await.is_err());
    }

    fn listed_post(id: &str) -> serde_json::Value {
        json!({
            "id": id,
            "title": "投稿",
            "feeRequired": 0,
            "type": "text",
            "coverImageUrl": null,
            "cover": null,
            "creatorId": "demo_author_001",
            "publishedDatetime": "2026-09-03T00:00:00+09:00",
            "updatedDatetime": "2026-09-03T00:00:00+09:00",
            "body": null,
            "user": null,
            "nextPost": null,
            "prevPost": null
        })
    }

    #[test]
    fn pagination_accepts_current_page_urls_wrapper_and_legacy_array() {
        let url = "https://api.fanbox.cc/post.listCreator?creatorId=demo_author_001&maxPublishedDatetime=x";
        assert_eq!(
            pagination_page_urls(&json!({ "pageUrls": [url] })),
            Some(vec![url.to_string()])
        );
        assert_eq!(
            pagination_page_urls(&json!([url])),
            Some(vec![url.to_string()])
        );
    }

    #[test]
    fn post_lists_accept_posts_items_and_direct_arrays_with_next_url() {
        let (posts, next) = decode_post_list(json!({
            "posts": [listed_post("1")],
            "nextUrl": "https://api.fanbox.cc/post.listCreator?page=2"
        }))
        .unwrap();
        assert_eq!(posts[0].id, "1");
        assert_eq!(
            next.as_deref(),
            Some("https://api.fanbox.cc/post.listCreator?page=2")
        );

        let (posts, next) = decode_post_list(json!([listed_post("2")])).unwrap();
        assert_eq!(posts[0].id, "2");
        assert!(next.is_none());
    }

    #[test]
    fn malformed_post_error_does_not_retain_private_response_body() {
        let error = decode_post_list(json!([{
            "id": 123,
            "body": "private-post-content-marker"
        }]))
        .unwrap_err();
        let debug = format!("{error:?}");
        assert!(!debug.contains("private-post-content-marker"));
        assert!(!error.to_string().contains("private-post-content-marker"));
    }

    #[test]
    fn api_cookie_destination_requires_exact_official_https_host() {
        assert!(allowed_api_url("https://api.fanbox.cc/post.info?postId=1").is_ok());
        for url in [
            "http://api.fanbox.cc/post.info",
            "https://api.fanbox.cc:444/post.info",
            "https://www.fanbox.cc/post.info",
            "https://api.fanbox.cc.evil.example/post.info",
            "https://evil.example/?target=https://api.fanbox.cc",
            "not a url",
        ] {
            assert!(allowed_api_url(url).is_err(), "accepted {url}");
        }
    }

    #[test]
    fn cloudflare_challenges_are_temporary_limits_not_bad_credentials() {
        assert!(is_cloudflare_challenge(
            "<title>Just a moment...</title><script src=\"/cdn-cgi/challenge-platform/x\"></script>"
        ));
        assert!(!is_cloudflare_challenge(r#"{"error":"forbidden"}"#));
    }

    #[tokio::test]
    async fn json_body_limit_rejects_content_length_and_chunked_oversize() {
        for wire_response in [
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nabcde"
                .as_slice(),
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nabcde\r\n0\r\n\r\n"
                .as_slice(),
        ] {
            let response = response_from_wire(wire_response).await;
            let error = read_response_text_limited(response, 4).await.unwrap_err();
            assert!(error.to_string().contains("4-byte safety limit"));
        }
    }

    #[tokio::test]
    async fn cookie_client_never_follows_redirects() {
        let redirect_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redirect_address = redirect_listener.local_addr().unwrap();
        let target_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_address = target_listener.local_addr().unwrap();

        let redirect_server = tokio::spawn(async move {
            let (mut stream, _) = redirect_listener.accept().await.unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await;
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{target_address}/credential-target\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });

        let client = build_api_client(Duration::from_secs(1), Duration::from_secs(1)).unwrap();
        let response = client
            .get(format!("http://{redirect_address}/authenticated"))
            .header(COOKIE, "FANBOXSESSID=must-not-be-forwarded")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        redirect_server.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), target_listener.accept())
                .await
                .is_err(),
            "redirect target unexpectedly received the cookie"
        );
    }

    #[tokio::test]
    async fn api_client_enforces_total_request_timeout() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });

        let client =
            build_api_client(Duration::from_millis(100), Duration::from_millis(100)).unwrap();
        let started = std::time::Instant::now();
        let error = client
            .get(format!("http://{address}/never-responds"))
            .send()
            .await
            .unwrap_err();
        server.abort();

        assert!(error.is_timeout());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
