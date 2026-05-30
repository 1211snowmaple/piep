use reqwest::header::{HeaderMap, HeaderValue as HV, ACCEPT, COOKIE, ORIGIN, REFERER, USER_AGENT};
use reqwest::{Client, StatusCode};
use serde::de::DeserializeOwned;
use std::time::Duration;

use crate::fanbox_api::error::FanboxError;
use crate::fanbox_api::models::*;

/// Fanbox API クライアント
#[derive(Clone)]
pub struct FanboxAPI {
    client: Client,
    cookie: String,
    user_agent: String,
}

impl FanboxAPI {
    /// 新規クライアントの作成
    pub fn new(cookie: String, user_agent: String) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("Failed to build reqwest client");

        Self {
            client,
            cookie,
            user_agent,
        }
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

    /// 低レベル GET リクエストの共通ハンドラ
    async fn api_get<T: DeserializeOwned>(&self, url: &str) -> Result<T, FanboxError> {
        let headers = self.headers()?;
        let res = self.client.get(url).headers(headers).send().await?;

        let status = res.status();
        let body = res.text().await?;

        match status {
            StatusCode::TOO_MANY_REQUESTS => {
                log::error!("Fanbox API Rate Limited: {}", body);
                Err(FanboxError::RateLimited { body })
            }
            StatusCode::NOT_FOUND => {
                log::error!("Fanbox Resource Not Found: {}", body);
                Err(FanboxError::NotFound { body })
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

    /// 3. 単一投稿の詳細取得
    pub async fn get_post(&self, post_id: &str) -> Result<FanboxPost, FanboxError> {
        let url = format!("https://api.fanbox.cc/post.info?postId={}", post_id);
        let resp: FanboxResponse<FanboxPost> = self.api_get(&url).await?;
        Ok(resp.body)
    }

    /// 4. 支援中クリエイターの投稿一覧
    pub async fn list_supporting_posts(
        &self,
        limit: Option<u32>,
    ) -> Result<FanboxPostList, FanboxError> {
        let limit = limit.unwrap_or(20);
        let url = format!("https://api.fanbox.cc/post.listSupporting?limit={}", limit);
        let resp: FanboxResponse<FanboxPostList> = self.api_get(&url).await?;
        Ok(resp.body)
    }

    /// 5. 支援中プラン一覧
    pub async fn list_supporting_plans(&self) -> Result<Vec<FanboxPlan>, FanboxError> {
        let url = "https://api.fanbox.cc/plan.listSupporting";
        let resp: FanboxResponse<Vec<FanboxPlan>> = self.api_get(url).await?;
        Ok(resp.body)
    }

    /// 6. クリエイターの投稿ページネーションURLの一覧取得
    pub async fn paginate_creator_posts(
        &self,
        creator_id: &str,
    ) -> Result<Vec<String>, FanboxError> {
        let url = format!(
            "https://api.fanbox.cc/post.paginateCreator?creatorId={}",
            creator_id
        );
        let resp: FanboxPaginatedCreatorPosts = self.api_get(&url).await?;

        let mut urls = Vec::new();
        if let Some(arr) = resp.body.as_array() {
            for val in arr {
                if let Some(s) = val.as_str() {
                    urls.push(s.to_string());
                }
            }
        } else {
            log::warn!(
                "post.paginateCreator returned non-array body: {:?}",
                resp.body
            );
        }
        Ok(urls)
    }

    /// 7. クリエイターの投稿一覧取得（特定のURLまたはパラメータから）
    pub async fn list_creator_posts(
        &self,
        creator_id: &str,
        limit: Option<u32>,
    ) -> Result<FanboxPostList, FanboxError> {
        let limit = limit.unwrap_or(20);
        let url = format!(
            "https://api.fanbox.cc/post.listCreator?creatorId={}&limit={}",
            creator_id, limit
        );
        let resp: FanboxResponse<FanboxPostList> = self.api_get(&url).await?;
        Ok(resp.body)
    }

    /// ホームタイムラインの投稿一覧取得
    pub async fn list_home_posts(&self, limit: Option<u32>) -> Result<FanboxPostList, FanboxError> {
        let limit = limit.unwrap_or(20);
        let url = format!("https://api.fanbox.cc/post.listHome?limit={}", limit);
        let resp: FanboxResponse<FanboxPostList> = self.api_get(&url).await?;
        Ok(resp.body)
    }

    /// next_url またはページネーションURLから汎用的に投稿リストを取得
    pub async fn get_posts_by_url(&self, url: &str) -> Result<FanboxPostList, FanboxError> {
        let resp: FanboxResponse<FanboxPostList> = self.api_get(url).await?;
        Ok(resp.body)
    }

    /// 8. クリエイターの全投稿を走査して一括取得するヘルパーメソッド (ページネーションの全トラバース)
    pub async fn get_all_creator_posts(
        &self,
        creator_id: &str,
    ) -> Result<Vec<FanboxPost>, FanboxError> {
        let mut all_posts = Vec::new();

        // 1. paginateCreatorでページURLの一覧を取得
        let url = format!(
            "https://api.fanbox.cc/post.paginateCreator?creatorId={}",
            creator_id
        );
        let resp: FanboxPaginatedCreatorPosts = self.api_get(&url).await?;

        if let Some(arr) = resp.body.as_array() {
            if !arr.is_empty() {
                if arr[0].is_string() {
                    // パターンA: ページURL의 リストが返ってきた場合 (Python版と同じ巡回アルゴリズム)
                    for val in arr {
                        if let Some(page_url) = val.as_str() {
                            log::info!("Fetching page: {}", page_url);

                            // 🛠️ どちらの構造が来ても FlexibleResponse で安全にパース！
                            let flexible_resp: FlexibleResponse = self.api_get(page_url).await?;

                            // 中身を取り出して一本の配列にまとめる
                            match flexible_resp.body {
                                FlexiblePostList::Object(list) => all_posts.extend(list.items),
                                FlexiblePostList::Array(posts) => all_posts.extend(posts),
                            }
                        }
                    }
                    return Ok(all_posts);
                } else if arr[0].is_object() {
                    // パターンB: paginateCreator自体が直接投稿配列を返してきた場合
                    log::info!(
                        "paginateCreator directly returned post array. Deserializing directly..."
                    );
                    let posts: Vec<FanboxPost> = serde_json::from_value(resp.body.clone())
                        .map_err(|e| FanboxError::Serde {
                            error: e,
                            body: resp.body.to_string(),
                        })?;
                    return Ok(posts);
                }
            }
        }

        // 2. 究極のフォールバック (URL配列が得られなかった場合)
        log::warn!("Pagination empty or unrecognized. Falling back to listCreator API.");
        let fallback_url = format!(
            "https://api.fanbox.cc/post.listCreator?creatorId={}&limit=100",
            creator_id
        );

        // 🛠️ ここでも FlexibleResponse で安全にパースしてブレを完全に吸収！
        let flexible_resp: FlexibleResponse = self.api_get(&fallback_url).await?;
        match flexible_resp.body {
            FlexiblePostList::Object(list) => Ok(list.items),
            FlexiblePostList::Array(posts) => Ok(posts),
        }
    }
}
