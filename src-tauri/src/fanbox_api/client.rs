use reqwest::header::{HeaderMap, HeaderValue as HV, ACCEPT, COOKIE, ORIGIN, REFERER, USER_AGENT};
use reqwest::{Client, StatusCode};
use serde::de::DeserializeOwned;
use std::time::Duration;

use crate::fanbox_api::error::FanboxError;
use crate::fanbox_api::models::*;

const FANBOX_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const FANBOX_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_FANBOX_JSON_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_INITIAL_RESPONSE_CAPACITY: usize = 64 * 1024;
/// 一覧を辿るときに、ページとページの間で空ける時間。
///
/// 長く続けている creator の履歴は数十ページになる。続けざまに取りに行けば
/// 断られ、断られた時点で一覧そのものが手に入らなくなる。更新ジョブの
/// 800ms と揃えてある。
const FANBOX_PAGE_DELAY: Duration = Duration::from_millis(800);
/// 断られたときに、同じページを何回までやり直すか。
const FANBOX_PAGE_RETRIES: u32 = 3;

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
    Ok((status, String::from_utf8_lossy(&body).into_owned()))
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
        let client = build_api_client(FANBOX_CONNECT_TIMEOUT, FANBOX_REQUEST_TIMEOUT)?;
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
        let mut backoff = FANBOX_PAGE_DELAY;
        for attempt in 0..=FANBOX_PAGE_RETRIES {
            match self.api_get_once::<T>(url).await {
                Err(FanboxError::RateLimited { body }) if attempt < FANBOX_PAGE_RETRIES => {
                    backoff *= 2;
                    log::warn!(
                        "FANBOX rate limited while paging ({}); retrying in {:?}: {}",
                        url,
                        backoff,
                        body
                    );
                    tokio::time::sleep(backoff).await;
                }
                other => return other,
            }
        }
        unreachable!("the loop returns on its last attempt")
    }

    /// 低レベル GET リクエストの共通ハンドラ。
    ///
    /// 取得制限だけは、一覧と同じように待って試し直す。一覧（`api_get_paged`）
    /// にはこれがあり、単発の投稿・クリエイター取得には無かった。断られる
    /// 理由は同じなのに、扱いが場所によって違っていた。
    async fn api_get<T: DeserializeOwned>(&self, url: &str) -> Result<T, FanboxError> {
        let mut backoff = FANBOX_PAGE_DELAY;
        for attempt in 0..=FANBOX_PAGE_RETRIES {
            match self.api_get_once::<T>(url).await {
                Err(FanboxError::RateLimited { body }) if attempt < FANBOX_PAGE_RETRIES => {
                    backoff *= 2;
                    log::warn!("FANBOX rate limited ({url}); retrying in {backoff:?}: {body}");
                    tokio::time::sleep(backoff).await;
                }
                other => return other,
            }
        }
        unreachable!("the loop returns on its last attempt")
    }

    async fn api_get_once<T: DeserializeOwned>(&self, url: &str) -> Result<T, FanboxError> {
        let url = allowed_api_url(url)?;
        let headers = self.headers()?;
        let res = self.client.get(url).headers(headers).send().await?;

        let (status, body) =
            read_response_text_limited(res, MAX_FANBOX_JSON_RESPONSE_BYTES).await?;

        match status {
            StatusCode::TOO_MANY_REQUESTS => {
                log::error!("Fanbox API Rate Limited: {}", body);
                Err(FanboxError::RateLimited { body })
            }
            StatusCode::NOT_FOUND => {
                log::error!("Fanbox Resource Not Found: {}", body);
                Err(FanboxError::NotFound { body })
            }
            StatusCode::FORBIDDEN if is_cloudflare_challenge(&body) => {
                log::warn!("Fanbox API temporarily refused an automated request");
                Err(FanboxError::RateLimited {
                    body: "Cloudflare challenge".to_string(),
                })
            }
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                log::warn!("Fanbox API Authentication Required: {}", body);
                Err(FanboxError::NoAuth)
            }
            _ => {
                if !status.is_success() {
                    log::warn!(
                        "Fanbox API returned non-success status: {}, body: {}",
                        status,
                        body
                    );
                    return Err(FanboxError::ApiError {
                        status: status.as_u16(),
                        body,
                    });
                }

                serde_json::from_str::<T>(&body).map_err(|e| {
                    log::error!("Fanbox Deserialization Error: {}, body: {}", e, body);
                    FanboxError::Serde { error: e, body }
                })
            }
        }
    }

    // --- エンドポイント実装 ---

    /// 1. セッション（Cookie）が有効かチェック
    pub async fn check_session(&self) -> Result<(), FanboxError> {
        // 未読通知数を取得する軽量なエンドポイントでテスト
        let url = "https://api.fanbox.cc/bell.countUnread";
        let _res: FanboxResponse<serde_json::Value> = self.api_get(url).await?;
        Ok(())
    }

    /// 2. クリエイター情報取得
    pub async fn get_creator(&self, creator_id: &str) -> Result<FanboxCreator, FanboxError> {
        let url = format!("https://api.fanbox.cc/creator.get?creatorId={}", creator_id);
        let resp: FanboxResponse<FanboxCreator> = self.api_get(&url).await?;
        Ok(resp.body)
    }

    /// 投稿APIは追加フィールドや投稿種別ごとの差が大きいため、保存経路では
    /// 構造を失わない生のJSONを利用する。
    pub async fn get_post_value(&self, post_id: &str) -> Result<serde_json::Value, FanboxError> {
        let url = format!("https://api.fanbox.cc/post.info?postId={}", post_id);
        let resp: FanboxResponse<serde_json::Value> = self.api_get(&url).await?;
        crate::fanbox_api::payload::into_post(resp.body).ok_or_else(|| {
            FanboxError::Other(
                "FANBOX投稿応答の構造を解釈できませんでした（応答形式が変わった可能性があります）"
                    .to_string(),
            )
        })
    }

    /// 8. クリエイターの全投稿を走査して一括取得するヘルパーメソッド (ページネーションの全トラバース)
    pub async fn get_all_creator_posts(
        &self,
        creator_id: &str,
    ) -> Result<Vec<FanboxPost>, FanboxError> {
        self.get_creator_posts_since(creator_id, None).await
    }

    pub async fn get_creator_posts_since(
        &self,
        creator_id: &str,
        stop_source_id: Option<&str>,
    ) -> Result<Vec<FanboxPost>, FanboxError> {
        let mut all_posts = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();

        // 1. paginateCreatorでページURLの一覧を取得
        let url = format!(
            "https://api.fanbox.cc/post.paginateCreator?creatorId={}",
            creator_id
        );
        let resp: FanboxResponse<serde_json::Value> = self.api_get_paged(&url).await?;

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
                if append_fanbox_posts(&mut all_posts, &mut seen_ids, posts, stop_source_id)? {
                    return Ok(all_posts);
                }
            }
            if !page_urls.is_empty() {
                return Ok(all_posts);
            }
        } else if post_list_shape(&resp.body) {
            // Some historical versions returned posts from paginateCreator itself.
            let (posts, _) = decode_post_list(resp.body)?;
            append_fanbox_posts(&mut all_posts, &mut seen_ids, posts, stop_source_id)?;
            return Ok(all_posts);
        }

        // 2. 究極のフォールバック (URL配列が得られなかった場合)
        log::warn!("Pagination empty or unrecognized. Falling back to listCreator API.");
        let fallback_url = format!(
            "https://api.fanbox.cc/post.listCreator?creatorId={}&limit=100",
            creator_id
        );

        // Follow nextUrl as well. The previous one-shot fallback silently truncated
        // creators with more than 100 posts when paginateCreator changed shape.
        let mut next_url = Some(fallback_url);
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
            if append_fanbox_posts(&mut all_posts, &mut seen_ids, posts, stop_source_id)? {
                break;
            }
            next_url = following_url;
            page_index += 1;
        }
        Ok(all_posts)
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
    let body_for_error = body.to_string();
    if body.is_array() {
        let posts = serde_json::from_value(body).map_err(|error| FanboxError::Serde {
            error,
            body: body_for_error,
        })?;
        return Ok((posts, None));
    }
    if !post_list_shape(&body) {
        return Err(FanboxError::Other(
            "FANBOX投稿一覧の応答形式を解釈できませんでした".to_string(),
        ));
    }
    let list: FanboxPostList =
        serde_json::from_value(body).map_err(|error| FanboxError::Serde {
            error,
            body: body_for_error,
        })?;
    Ok((list.items, list.next_url))
}

fn append_fanbox_posts(
    output: &mut Vec<FanboxPost>,
    seen_ids: &mut std::collections::HashSet<String>,
    posts: Vec<FanboxPost>,
    stop_source_id: Option<&str>,
) -> Result<bool, FanboxError> {
    for post in posts {
        if stop_source_id.is_some_and(|stop| stop == post.id) {
            return Ok(true);
        }
        if seen_ids.insert(post.id.clone()) {
            output.push(post);
        }
        if output.len() >= 10_000 {
            return Err(FanboxError::Other(
                "FANBOX history exceeded the 10,000-item safety limit".to_string(),
            ));
        }
    }
    Ok(false)
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

    fn listed_post(id: &str) -> serde_json::Value {
        json!({
            "id": id,
            "title": "投稿",
            "feeRequired": 0,
            "type": "text",
            "coverImageUrl": null,
            "cover": null,
            "creatorId": "ponkan",
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
        let url = "https://api.fanbox.cc/post.listCreator?creatorId=ponkan&maxPublishedDatetime=x";
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
