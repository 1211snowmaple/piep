//! Pixiv App API (6.x app-api.pixiv.net) - pixivpy3.aapi.AppPixivAPI の移植版。
//! 認証、HTTPクライアント、ダウンロード（BasePixivAPI由来）の基本ロジックを含みます。
#![allow(clippy::needless_lifetimes, clippy::too_many_arguments)]

use std::{sync::LazyLock, time::Duration};

use kv_pairs::{kv_pairs, KVPairs};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue as HV, AUTHORIZATION, HOST, USER_AGENT};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use tokio::io::AsyncWriteExt;

use crate::pixiv_api::error::PixivError;
use crate::pixiv_api::models::*;
use crate::pixiv_api::params::*;
use crate::pixiv_api::token_manager::TokenManager;

const API_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const API_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

fn build_api_client(
    connect_timeout: Duration,
    request_timeout: Duration,
) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        // 認証ヘッダーを別 URL に再送しない。API の redirect は仕様外として呼び出し側に返す。
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

/// 内部リクエスト用のシンプルなHTTPメソッド列挙型。
///
#[derive(Copy, Clone, Debug)]
#[non_exhaustive]
pub enum HttpMethod {
    /// GETリクエスト。
    GET,
    /// POSTリクエスト。
    POST,
    /// DELETEリクエスト。
    DELETE,
}

/// App-API (6.x) クライアント。`AppPixivAPI` の移植版（認証/HTTP/ダウンロード機能を内包）。
pub struct AppPixivAPI {
    hosts: String,
    client: reqwest::Client,
    token_manager: TokenManager,
}

impl AppPixivAPI {
    /// 認証なしのAPIクライアントを作成します。認証が必要な呼び出しは `PixivError::NoAuth` で失敗します。
    ///
    pub fn new_no_auth() -> Self {
        log::debug!("認証なしで AppPixivAPI を作成しています");
        Self::new_with(TokenManager::new_no_auth())
    }

    /// 既存のアクセストークンからAPIクライアントを作成します。リフレッシュは行われません。
    ///
    pub fn new_from_access_token(access_token: String) -> Self {
        log::debug!("アクセストークンを使用して AppPixivAPI を作成しています");
        Self::new_with(TokenManager::new_from_access_token(access_token))
    }

    /// リフレッシュトークンからAPIクライアントを作成します。アクセストークンは必要に応じて取得・更新されます。
    ///
    pub fn new_from_refresh_token(refresh_token: String) -> Self {
        log::debug!("リフレッシュトークンを使用して AppPixivAPI を作成しています");
        Self::new_with(TokenManager::new_from_refresh_token(refresh_token))
    }

    fn new_with(token_manager: TokenManager) -> Self {
        let client =
            build_api_client(API_CONNECT_TIMEOUT, API_REQUEST_TIMEOUT).expect("reqwest client");
        Self {
            hosts: "https://app-api.pixiv.net".to_string(),
            client,
            token_manager,
        }
    }

    /// 認証が設定されていることを確認し、アクセストークンを取得します。
    pub async fn get_access_token(&self) -> Result<String, PixivError> {
        self.token_manager.get_access_token().await
    }

    /// プロキシホストを設定します（例: pixivlite.com）。`set_api_proxy` の移植版。
    pub fn set_api_proxy(&mut self, proxy_hosts: &str) {
        self.hosts = proxy_hosts.to_string();
    }

    /// 低レベルのHTTP呼び出し（`requests_call` の移植版）。
    async fn do_http_request(
        &self,
        method: HttpMethod,
        url: &str,
        headers: Option<HeaderMap>,
        params: Option<KVPairs<'_>>,
        data: Option<KVPairs<'_>>,
    ) -> Result<reqwest::Response, PixivError> {
        let mut req = match method {
            HttpMethod::GET => self.client.get(url),
            HttpMethod::POST => self.client.post(url),
            HttpMethod::DELETE => self.client.delete(url),
        };
        if let Some(h) = headers {
            req = req.headers(h);
        }
        if let Some(p) = params {
            req = req.query(&p.content);
        }
        if let Some(d) = data {
            req = req.form(&d.content);
        }
        let res = req.send().await?;
        Ok(res)
    }

    /// 認証とアプリヘッダーを付与してAPIリクエストを実行します。カスタムエンドポイントに使用します。
    ///
    pub async fn do_api_request(
        &self,
        method: HttpMethod,
        url: &str,
        headers: Option<HeaderMap>,
        params: Option<KVPairs<'_>>,
        data: Option<KVPairs<'_>>,
        with_auth: bool,
    ) -> Result<reqwest::Response, PixivError> {
        let mut headers = headers.unwrap_or_default();
        if self.hosts != "https://app-api.pixiv.net" {
            headers.insert(HOST, HV::from_static("app-api.pixiv.net"));
        }

        if !headers.contains_key("user-agent") {
            headers.insert(HeaderName::from_static("app-os"), HV::from_static("ios"));
            headers.insert(
                HeaderName::from_static("app-os-version"),
                HV::from_static("14.6"),
            );
            headers.insert(
                USER_AGENT,
                HV::from_static("PixivIOSApp/7.13.3 (iOS 14.6; iPhone13,2)"),
            );
        }
        if with_auth {
            let access_token = self.get_access_token().await?;
            headers.insert(
                AUTHORIZATION,
                HV::from_str(&format!("Bearer {}", access_token)).map_err(|e| {
                    PixivError::BadAccessToken {
                        access_token,
                        message: format!("{}", e),
                    }
                })?,
            );
        }
        self.do_http_request(method, url, Some(headers), params, data)
            .await
    }
}

/// 構造化されたAPI呼び出し。
impl AppPixivAPI {
    pub async fn user_detail<'a0>(
        &'a0 self,
        user_id: u64,
        filter: Option<Filter>,
        with_auth: bool,
    ) -> Result<UserInfoDetailed, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/user/detail");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(user_id), user_id);
        }
        {
            let filter = filter.unwrap_or(Filter::ForIos);
            params.push(stringify!(filter), filter);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(user_detail),
            "/v1/user/detail"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<UserInfoDetailed>(r).await
    }
    pub async fn user_illusts<'a0, 'a1>(
        &'a0 self,
        user_id: u64,
        type_: Option<IllustType>,
        filter: Option<Filter>,
        offset: Option<&'a1 str>,
        with_auth: bool,
    ) -> Result<UserIllustrations, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/user/illusts");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(user_id), user_id);
        }
        {
            let type_ = type_.unwrap_or(IllustType::Illust);
            params.push("type", type_);
        }
        {
            params.push(stringify!(filter), filter);
        }
        {
            params.push(stringify!(offset), offset);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(user_illusts),
            "/v1/user/illusts"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<UserIllustrations>(r).await
    }
    pub fn user_illusts_iter<'a0, 'a1>(
        &'a0 self,
        user_id: u64,
        type_: Option<IllustType>,
        filter: Option<Filter>,
        offset: Option<&'a1 str>,
        with_auth: bool,
    ) -> impl ::futures_core::stream::Stream<
        Item = Result<IllustrationInfo, crate::pixiv_api::error::PixivError>,
    > + use<'a0, 'a1> {
        log::debug!(
            "イテレータ呼び出し: {} ({} のイテレータ版)",
            stringify!(user_illusts),
            stringify!(user_illusts_iter)
        );
        async_stream::try_stream! {
            log::debug!
            ("{}: 最初のページをリクエスト中 エンドポイント: {}", stringify! (user_illusts_iter),
            "/v1/user/illusts"); let mut result =
            self.user_illusts(user_id, type_, filter, offset, with_auth).await ? ;
            let mut next_url = result.next_url; loop
            {
                for item in result.illusts { yield item; } match & next_url
                {
                    Some(url) =>
                    {
                        log::debug!
                        ("{}: 次のページをリクエスト中 エンドポイント: {}", stringify! (user_illusts_iter),
                        url); result = self.visit_next_url :: < UserIllustrations >
                        (url, with_auth).await ? ; next_url = result.next_url;
                    } None =>
                    {
                        log::debug!
                        ("{}: 終端に達しました", stringify!
                        (user_illusts_iter)); break;
                    },
                }
            }
        }
    }
    pub async fn user_bookmarks_illust<'a0, 'a1, 'a2>(
        &'a0 self,
        user_id: u64,
        restrict: Option<Restrict>,
        filter: Option<Filter>,
        max_bookmark_id: Option<&'a1 str>,
        tag: Option<&'a2 str>,
        with_auth: bool,
    ) -> Result<UserBookmarksIllustrations, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/user/bookmarks/illust");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(user_id), user_id);
        }
        {
            let restrict = restrict.unwrap_or(Restrict::Public);
            params.push(stringify!(restrict), restrict);
        }
        {
            let filter = filter.unwrap_or(Filter::ForIos);
            params.push(stringify!(filter), filter);
        }
        {
            params.push(stringify!(max_bookmark_id), max_bookmark_id);
        }
        {
            params.push(stringify!(tag), tag);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(user_bookmarks_illust),
            "/v1/user/bookmarks/illust"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<UserBookmarksIllustrations>(r).await
    }
    pub fn user_bookmarks_illust_iter<'a0, 'a1, 'a2>(
        &'a0 self,
        user_id: u64,
        restrict: Option<Restrict>,
        filter: Option<Filter>,
        max_bookmark_id: Option<&'a1 str>,
        tag: Option<&'a2 str>,
        with_auth: bool,
    ) -> impl ::futures_core::stream::Stream<
        Item = Result<IllustrationInfo, crate::pixiv_api::error::PixivError>,
    > + use<'a0, 'a1, 'a2> {
        log::debug!(
            "イテレータ呼び出し: {} ({} のイテレータ版)",
            stringify!(user_bookmarks_illust),
            stringify!(user_bookmarks_illust_iter)
        );
        async_stream::try_stream! {
            log::debug!
            ("{}: 最初のページをリクエスト中 エンドポイント: {}", stringify! (user_bookmarks_illust_iter),
            "/v1/user/bookmarks/illust"); let mut result =
            self.user_bookmarks_illust(user_id, restrict, filter, max_bookmark_id,
            tag, with_auth).await ? ; let mut next_url = result.next_url; loop
            {
                for item in result.illusts { yield item; } match & next_url
                {
                    Some(url) =>
                    {
                        log::debug!
                        ("{}: 次のページをリクエスト中 エンドポイント: {}", stringify!
                        (user_bookmarks_illust_iter), url); result =
                        self.visit_next_url :: < UserBookmarksIllustrations >
                        (url, with_auth).await ? ; next_url = result.next_url;
                    } None =>
                    {
                        log::debug!
                        ("{}: 終端に達しました", stringify!
                        (user_bookmarks_illust_iter)); break;
                    },
                }
            }
        }
    }
    pub async fn user_bookmarks_novel<'a0, 'a1, 'a2>(
        &'a0 self,
        user_id: u64,
        restrict: Option<Restrict>,
        filter: Option<Filter>,
        max_bookmark_id: Option<&'a1 str>,
        tag: Option<&'a2 str>,
        with_auth: bool,
    ) -> Result<UserBookmarksNovel, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/user/bookmarks/novel");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(user_id), user_id);
        }
        {
            let restrict = restrict.unwrap_or(Restrict::Public);
            params.push(stringify!(restrict), restrict);
        }
        {
            let filter = filter.unwrap_or(Filter::ForIos);
            params.push(stringify!(filter), filter);
        }
        {
            params.push(stringify!(max_bookmark_id), max_bookmark_id);
        }
        {
            params.push(stringify!(tag), tag);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(user_bookmarks_novel),
            "/v1/user/bookmarks/novel"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<UserBookmarksNovel>(r).await
    }
    pub fn user_bookmarks_novel_iter<'a0, 'a1, 'a2>(
        &'a0 self,
        user_id: u64,
        restrict: Option<Restrict>,
        filter: Option<Filter>,
        max_bookmark_id: Option<&'a1 str>,
        tag: Option<&'a2 str>,
        with_auth: bool,
    ) -> impl ::futures_core::stream::Stream<
        Item = Result<NovelInfo, crate::pixiv_api::error::PixivError>,
    > + use<'a0, 'a1, 'a2> {
        log::debug!(
            "イテレータ呼び出し: {} ({} のイテレータ版)",
            stringify!(user_bookmarks_novel),
            stringify!(user_bookmarks_novel_iter)
        );
        async_stream::try_stream! {
            log::debug!
            ("{}: 最初のページをリクエスト中 エンドポイント: {}", stringify! (user_bookmarks_novel_iter),
            "/v1/user/bookmarks/novel"); let mut result =
            self.user_bookmarks_novel(user_id, restrict, filter, max_bookmark_id,
            tag, with_auth).await ? ; let mut next_url = result.next_url; loop
            {
                for item in result.novels { yield item; } match & next_url
                {
                    Some(url) =>
                    {
                        log::debug!
                        ("{}: 次のページをリクエスト中 エンドポイント: {}", stringify!
                        (user_bookmarks_novel_iter), url); result =
                        self.visit_next_url :: < UserBookmarksNovel >
                        (url, with_auth).await ? ; next_url = result.next_url;
                    } None =>
                    {
                        log::debug!
                        ("{}: 終端に達しました", stringify!
                        (user_bookmarks_novel_iter)); break;
                    },
                }
            }
        }
    }
    pub async fn user_related<'a0, 'a1>(
        &'a0 self,
        seed_user_id: u64,
        filter: Option<Filter>,
        offset: Option<&'a1 str>,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/user/related");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(seed_user_id), seed_user_id);
        }
        {
            let filter = filter.unwrap_or(Filter::ForIos);
            params.push(stringify!(filter), filter);
        }
        {
            let offset = offset.unwrap_or("0");
            params.push(stringify!(offset), offset);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(user_related),
            "/v1/user/related"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
    pub async fn user_recommended<'a0, 'a1>(
        &'a0 self,
        filter: Option<Filter>,
        offset: Option<&'a1 str>,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/user/recommended");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            let filter = filter.unwrap_or(Filter::ForIos);
            params.push(stringify!(filter), filter);
        }
        {
            params.push(stringify!(offset), offset);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(user_recommended),
            "/v1/user/recommended"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
    pub async fn illust_follow<'a0, 'a1>(
        &'a0 self,
        restrict: Option<Restrict>,
        offset: Option<&'a1 str>,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v2/illust/follow");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            let restrict = restrict.unwrap_or(Restrict::Public);
            params.push(stringify!(restrict), restrict);
        }
        {
            params.push(stringify!(offset), offset);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(illust_follow),
            "/v2/illust/follow"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
    pub async fn illust_detail<'a0>(
        &'a0 self,
        illust_id: u64,
        with_auth: bool,
    ) -> Result<IllustDetail, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/illust/detail");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(illust_id), illust_id);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(illust_detail),
            "/v1/illust/detail"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<IllustDetail>(r).await
    }
    pub async fn illust_comments<'a0, 'a1>(
        &'a0 self,
        illust_id: u64,
        offset: Option<&'a1 str>,
        include_total_comments: Option<bool>,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v3/illust/comments");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(illust_id), illust_id);
        }
        {
            params.push(stringify!(offset), offset);
        }
        {
            params.push(stringify!(include_total_comments), include_total_comments);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(illust_comments),
            "/v3/illust/comments"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
    pub async fn illust_ranking<'a0, 'a1, 'a2>(
        &'a0 self,
        mode: Option<RankingMode>,
        filter: Option<Filter>,
        date: Option<&'a1 str>,
        offset: Option<&'a2 str>,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/illust/ranking");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            let mode = mode.unwrap_or(RankingMode::Day);
            params.push(stringify!(mode), mode);
        }
        {
            let filter = filter.unwrap_or(Filter::ForIos);
            params.push(stringify!(filter), filter);
        }
        {
            params.push(stringify!(date), date);
        }
        {
            params.push(stringify!(offset), offset);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(illust_ranking),
            "/v1/illust/ranking"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
    pub async fn trending_tags_illust<'a0>(
        &'a0 self,
        filter: Option<Filter>,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/trending-tags/illust");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            let filter = filter.unwrap_or(Filter::ForIos);
            params.push(stringify!(filter), filter);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(trending_tags_illust),
            "/v1/trending-tags/illust"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
    pub async fn search_illust<'a0, 'a1, 'a2, 'a3, 'a4, 'a5>(
        &'a0 self,
        word: &'a1 str,
        search_target: Option<SearchTarget>,
        sort: Option<Sort>,
        duration: Option<&'a2 str>,
        start_date: Option<&'a3 str>,
        end_date: Option<&'a4 str>,
        filter: Option<Filter>,
        search_ai_type: Option<u8>,
        offset: Option<&'a5 str>,
        with_auth: bool,
    ) -> Result<SearchIllustrations, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/search/illust");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(word), word);
        }
        {
            let search_target = search_target.unwrap_or(SearchTarget::PartialMatchForTags);
            params.push(stringify!(search_target), search_target);
        }
        {
            let sort = sort.unwrap_or(Sort::DateDesc);
            params.push(stringify!(sort), sort);
        }
        {
            params.push(stringify!(duration), duration);
        }
        {
            params.push(stringify!(start_date), start_date);
        }
        {
            params.push(stringify!(end_date), end_date);
        }
        {
            let filter = filter.unwrap_or(Filter::ForIos);
            params.push(stringify!(filter), filter);
        }
        {
            params.push(stringify!(search_ai_type), search_ai_type);
        }
        {
            params.push(stringify!(offset), offset);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(search_illust),
            "/v1/search/illust"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<SearchIllustrations>(r).await
    }
    pub fn search_illust_iter<'a0, 'a1, 'a2, 'a3, 'a4, 'a5>(
        &'a0 self,
        word: &'a1 str,
        search_target: Option<SearchTarget>,
        sort: Option<Sort>,
        duration: Option<&'a2 str>,
        start_date: Option<&'a3 str>,
        end_date: Option<&'a4 str>,
        filter: Option<Filter>,
        search_ai_type: Option<u8>,
        offset: Option<&'a5 str>,
        with_auth: bool,
    ) -> impl ::futures_core::stream::Stream<
        Item = Result<IllustrationInfo, crate::pixiv_api::error::PixivError>,
    > + use<'a0, 'a1, 'a2, 'a3, 'a4, 'a5> {
        log::debug!(
            "イテレータ呼び出し: {} ({} のイテレータ版)",
            stringify!(search_illust),
            stringify!(search_illust_iter)
        );
        async_stream::try_stream! {
            log::debug!
            ("{}: 最初のページをリクエスト中 エンドポイント: {}", stringify! (search_illust_iter),
            "/v1/search/illust"); let mut result =
            self.search_illust(word, search_target, sort, duration, start_date,
            end_date, filter, search_ai_type, offset, with_auth).await ? ; let mut
            next_url = result.next_url; loop
            {
                for item in result.illusts { yield item; } match & next_url
                {
                    Some(url) =>
                    {
                        log::debug!
                        ("{}: 次のページをリクエスト中 エンドポイント: {}", stringify! (search_illust_iter),
                        url); result = self.visit_next_url :: < SearchIllustrations
                        > (url, with_auth).await ? ; next_url = result.next_url;
                    } None =>
                    {
                        log::debug!
                        ("{}: 終端に達しました", stringify!
                        (search_illust_iter)); break;
                    },
                }
            }
        }
    }
    pub async fn search_novel<'a0, 'a1, 'a2, 'a3, 'a4, 'a5, 'a6, 'a7>(
        &'a0 self,
        word: &'a1 str,
        search_target: Option<SearchTarget>,
        sort: Option<Sort>,
        merge_plain_keyword_results: Option<&'a2 str>,
        include_translated_tag_results: Option<&'a3 str>,
        start_date: Option<&'a4 str>,
        end_date: Option<&'a5 str>,
        filter: Option<&'a6 str>,
        search_ai_type: Option<u8>,
        offset: Option<&'a7 str>,
        with_auth: bool,
    ) -> Result<SearchNovel, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/search/novel");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(word), word);
        }
        {
            let search_target = search_target.unwrap_or(SearchTarget::PartialMatchForTags);
            params.push(stringify!(search_target), search_target);
        }
        {
            let sort = sort.unwrap_or(Sort::DateDesc);
            params.push(stringify!(sort), sort);
        }
        {
            let merge_plain_keyword_results = merge_plain_keyword_results.unwrap_or("true");
            params.push(
                stringify!(merge_plain_keyword_results),
                merge_plain_keyword_results,
            );
        }
        {
            let include_translated_tag_results = include_translated_tag_results.unwrap_or("true");
            params.push(
                stringify!(include_translated_tag_results),
                include_translated_tag_results,
            );
        }
        {
            params.push(stringify!(start_date), start_date);
        }
        {
            params.push(stringify!(end_date), end_date);
        }
        {
            params.push(stringify!(filter), filter);
        }
        {
            params.push(stringify!(search_ai_type), search_ai_type);
        }
        {
            params.push(stringify!(offset), offset);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(search_novel),
            "/v1/search/novel"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<SearchNovel>(r).await
    }
    pub fn search_novel_iter<'a0, 'a1, 'a2, 'a3, 'a4, 'a5, 'a6, 'a7>(
        &'a0 self,
        word: &'a1 str,
        search_target: Option<SearchTarget>,
        sort: Option<Sort>,
        merge_plain_keyword_results: Option<&'a2 str>,
        include_translated_tag_results: Option<&'a3 str>,
        start_date: Option<&'a4 str>,
        end_date: Option<&'a5 str>,
        filter: Option<&'a6 str>,
        search_ai_type: Option<u8>,
        offset: Option<&'a7 str>,
        with_auth: bool,
    ) -> impl ::futures_core::stream::Stream<
        Item = Result<NovelInfo, crate::pixiv_api::error::PixivError>,
    > + use<'a0, 'a1, 'a2, 'a3, 'a4, 'a5, 'a6, 'a7> {
        log::debug!(
            "イテレータ呼び出し: {} ({} のイテレータ版)",
            stringify!(search_novel),
            stringify!(search_novel_iter)
        );
        async_stream::try_stream! {
            log::debug!
            ("{}: 最初のページをリクエスト中 エンドポイント: {}", stringify! (search_novel_iter),
            "/v1/search/novel"); let mut result =
            self.search_novel(word, search_target, sort,
            merge_plain_keyword_results, include_translated_tag_results,
            start_date, end_date, filter, search_ai_type, offset, with_auth).await
            ? ; let mut next_url = result.next_url; loop
            {
                for item in result.novels { yield item; } match & next_url
                {
                    Some(url) =>
                    {
                        log::debug!
                        ("{}: 次のページをリクエスト中 エンドポイント: {}", stringify! (search_novel_iter),
                        url); result = self.visit_next_url :: < SearchNovel >
                        (url, with_auth).await ? ; next_url = result.next_url;
                    } None =>
                    {
                        log::debug!
                        ("{}: 終端に達しました", stringify!
                        (search_novel_iter)); break;
                    },
                }
            }
        }
    }
    pub async fn search_user<'a0, 'a1, 'a2, 'a3>(
        &'a0 self,
        word: &'a1 str,
        sort: Option<Sort>,
        duration: Option<&'a2 str>,
        filter: Option<Filter>,
        offset: Option<&'a3 str>,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/search/user");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(word), word);
        }
        {
            let sort = sort.unwrap_or(Sort::DateDesc);
            params.push(stringify!(sort), sort);
        }
        {
            params.push(stringify!(duration), duration);
        }
        {
            let filter = filter.unwrap_or(Filter::ForIos);
            params.push(stringify!(filter), filter);
        }
        {
            params.push(stringify!(offset), offset);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(search_user),
            "/v1/search/user"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
    pub async fn illust_bookmark_detail<'a0>(
        &'a0 self,
        illust_id: u64,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v2/illust/bookmark/detail");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(illust_id), illust_id);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(illust_bookmark_detail),
            "/v2/illust/bookmark/detail"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
    pub async fn user_bookmark_tags_illust<'a0, 'a1>(
        &'a0 self,
        user_id: u64,
        restrict: Option<Restrict>,
        offset: Option<&'a1 str>,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/user/bookmark-tags/illust");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(user_id), user_id);
        }
        {
            let restrict = restrict.unwrap_or(Restrict::Public);
            params.push(stringify!(restrict), restrict);
        }
        {
            params.push(stringify!(offset), offset);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(user_bookmark_tags_illust),
            "/v1/user/bookmark-tags/illust"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
    pub async fn user_following<'a0, 'a1>(
        &'a0 self,
        user_id: u64,
        restrict: Option<Restrict>,
        offset: Option<&'a1 str>,
        with_auth: bool,
    ) -> Result<UserFollowing, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/user/following");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(user_id), user_id);
        }
        {
            let restrict = restrict.unwrap_or(Restrict::Public);
            params.push(stringify!(restrict), restrict);
        }
        {
            params.push(stringify!(offset), offset);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(user_following),
            "/v1/user/following"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<UserFollowing>(r).await
    }
    pub fn user_following_iter<'a0, 'a1>(
        &'a0 self,
        user_id: u64,
        restrict: Option<Restrict>,
        offset: Option<&'a1 str>,
        with_auth: bool,
    ) -> impl ::futures_core::stream::Stream<
        Item = Result<UserPreview, crate::pixiv_api::error::PixivError>,
    > + use<'a0, 'a1> {
        log::debug!(
            "イテレータ呼び出し: {} ({} のイテレータ版)",
            stringify!(user_following),
            stringify!(user_following_iter)
        );
        async_stream::try_stream! {
            log::debug!
            ("{}: 最初のページをリクエスト中 エンドポイント: {}", stringify! (user_following_iter),
            "/v1/user/following"); let mut result =
            self.user_following(user_id, restrict, offset, with_auth).await ? ;
            let mut next_url = result.next_url; loop
            {
                for item in result.user_previews { yield item; } match & next_url
                {
                    Some(url) =>
                    {
                        log::debug!
                        ("{}: 次のページをリクエスト中 エンドポイント: {}", stringify! (user_following_iter),
                        url); result = self.visit_next_url :: < UserFollowing >
                        (url, with_auth).await ? ; next_url = result.next_url;
                    } None =>
                    {
                        log::debug!
                        ("{}: 終端に達しました", stringify!
                        (user_following_iter)); break;
                    },
                }
            }
        }
    }
    pub async fn user_follower<'a0, 'a1>(
        &'a0 self,
        user_id: u64,
        filter: Option<Filter>,
        offset: Option<&'a1 str>,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/user/follower");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(user_id), user_id);
        }
        {
            let filter = filter.unwrap_or(Filter::ForIos);
            params.push(stringify!(filter), filter);
        }
        {
            params.push(stringify!(offset), offset);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(user_follower),
            "/v1/user/follower"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
    pub async fn user_mypixiv<'a0, 'a1>(
        &'a0 self,
        user_id: u64,
        offset: Option<&'a1 str>,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/user/mypixiv");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(user_id), user_id);
        }
        {
            params.push(stringify!(offset), offset);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(user_mypixiv),
            "/v1/user/mypixiv"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
    pub async fn user_list<'a0, 'a1>(
        &'a0 self,
        user_id: u64,
        filter: Option<Filter>,
        offset: Option<&'a1 str>,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v2/user/list");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(user_id), user_id);
        }
        {
            let filter = filter.unwrap_or(Filter::ForIos);
            params.push(stringify!(filter), filter);
        }
        {
            params.push(stringify!(offset), offset);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(user_list),
            "/v2/user/list"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
    pub async fn ugoira_metadata<'a0>(
        &'a0 self,
        illust_id: u64,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/ugoira/metadata");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(illust_id), illust_id);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(ugoira_metadata),
            "/v1/ugoira/metadata"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
    pub async fn user_novels<'a0, 'a1>(
        &'a0 self,
        user_id: u64,
        filter: Option<Filter>,
        offset: Option<&'a1 str>,
        with_auth: bool,
    ) -> Result<UserNovels, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/user/novels");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(user_id), user_id);
        }
        {
            let filter = filter.unwrap_or(Filter::ForIos);
            params.push(stringify!(filter), filter);
        }
        {
            params.push(stringify!(offset), offset);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(user_novels),
            "/v1/user/novels"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<UserNovels>(r).await
    }
    pub fn user_novels_iter<'a0, 'a1>(
        &'a0 self,
        user_id: u64,
        filter: Option<Filter>,
        offset: Option<&'a1 str>,
        with_auth: bool,
    ) -> impl ::futures_core::stream::Stream<
        Item = Result<NovelInfo, crate::pixiv_api::error::PixivError>,
    > + use<'a0, 'a1> {
        log::debug!(
            "イテレータ呼び出し: {} ({} のイテレータ版)",
            stringify!(user_novels),
            stringify!(user_novels_iter)
        );
        async_stream::try_stream! {
            log::debug!
            ("{}: 最初のページをリクエスト中 エンドポイント: {}", stringify! (user_novels_iter),
            "/v1/user/novels"); let mut result =
            self.user_novels(user_id, filter, offset, with_auth).await ? ; let mut
            next_url = result.next_url; loop
            {
                for item in result.novels { yield item; } match & next_url
                {
                    Some(url) =>
                    {
                        log::debug!
                        ("{}: 次のページをリクエスト中 エンドポイント: {}", stringify! (user_novels_iter),
                        url); result = self.visit_next_url :: < UserNovels >
                        (url, with_auth).await ? ; next_url = result.next_url;
                    } None =>
                    {
                        log::debug!
                        ("{}: 終端に達しました", stringify!
                        (user_novels_iter)); break;
                    },
                }
            }
        }
    }
    pub async fn novel_series<'a0, 'a1>(
        &'a0 self,
        series_id: u64,
        filter: Option<Filter>,
        last_order: Option<&'a1 str>,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v2/novel/series");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(series_id), series_id);
        }
        {
            let filter = filter.unwrap_or(Filter::ForIos);
            params.push(stringify!(filter), filter);
        }
        {
            params.push(stringify!(last_order), last_order);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(novel_series),
            "/v2/novel/series"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
    pub async fn novel_detail<'a0>(
        &'a0 self,
        novel_id: u64,
        with_auth: bool,
    ) -> Result<NovelDetail, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v2/novel/detail");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(novel_id), novel_id);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(novel_detail),
            "/v2/novel/detail"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<NovelDetail>(r).await
    }
    pub async fn novel_comments<'a0, 'a1>(
        &'a0 self,
        novel_id: u64,
        offset: Option<&'a1 str>,
        include_total_comments: Option<bool>,
        with_auth: bool,
    ) -> Result<NovelComments, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/novel/comments");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(novel_id), novel_id);
        }
        {
            params.push(stringify!(offset), offset);
        }
        {
            params.push(stringify!(include_total_comments), include_total_comments);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(novel_comments),
            "/v1/novel/comments"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<NovelComments>(r).await
    }
    pub fn novel_comments_iter<'a0, 'a1>(
        &'a0 self,
        novel_id: u64,
        offset: Option<&'a1 str>,
        include_total_comments: Option<bool>,
        with_auth: bool,
    ) -> impl ::futures_core::stream::Stream<
        Item = Result<Comment, crate::pixiv_api::error::PixivError>,
    > + use<'a0, 'a1> {
        log::debug!(
            "イテレータ呼び出し: {} ({} のイテレータ版)",
            stringify!(novel_comments),
            stringify!(novel_comments_iter)
        );
        async_stream::try_stream! {
            log::debug!
            ("{}: 最初のページをリクエスト中 エンドポイント: {}", stringify! (novel_comments_iter),
            "/v1/novel/comments"); let mut result =
            self.novel_comments(novel_id, offset, include_total_comments,
            with_auth).await ? ; let mut next_url = result.next_url; loop
            {
                for item in result.comments { yield item; } match & next_url
                {
                    Some(url) =>
                    {
                        log::debug!
                        ("{}: 次のページをリクエスト中 エンドポイント: {}", stringify! (novel_comments_iter),
                        url); result = self.visit_next_url :: < NovelComments >
                        (url, with_auth).await ? ; next_url = result.next_url;
                    } None =>
                    {
                        log::debug!
                        ("{}: 終端に達しました", stringify!
                        (novel_comments_iter)); break;
                    },
                }
            }
        }
    }
    pub async fn novel_new<'a0, 'a1>(
        &'a0 self,
        filter: Option<Filter>,
        max_novel_id: Option<&'a1 str>,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/novel/new");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            let filter = filter.unwrap_or(Filter::ForIos);
            params.push(stringify!(filter), filter);
        }
        {
            params.push(stringify!(max_novel_id), max_novel_id);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(novel_new),
            "/v1/novel/new"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
    pub async fn illust_new<'a0, 'a1>(
        &'a0 self,
        content_type: Option<IllustType>,
        filter: Option<Filter>,
        max_illust_id: Option<&'a1 str>,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/illust/new");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            let content_type = content_type.unwrap_or(IllustType::Illust);
            params.push(stringify!(content_type), content_type);
        }
        {
            let filter = filter.unwrap_or(Filter::ForIos);
            params.push(stringify!(filter), filter);
        }
        {
            params.push(stringify!(max_illust_id), max_illust_id);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(illust_new),
            "/v1/illust/new"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
    pub async fn novel_follow<'a0>(
        &'a0 self,
        restrict: Option<Restrict>,
        offset: Option<u32>,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/novel/follow");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            let restrict = restrict.unwrap_or(Restrict::Public);
            params.push(stringify!(restrict), restrict);
        }
        {
            params.push(stringify!(offset), offset);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(novel_follow),
            "/v1/novel/follow"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
    pub async fn illust_bookmark_delete<'a0>(
        &'a0 self,
        illust_id: u64,
        with_auth: bool,
    ) -> Result<EmptyObject, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/illust/bookmark/delete");
        let mut data: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            data.push(stringify!(illust_id), illust_id);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(illust_bookmark_delete),
            "/v1/illust/bookmark/delete"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::POST,
                &url,
                None,
                None,
                Some(data),
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<EmptyObject>(r).await
    }
    pub async fn user_follow_add<'a0>(
        &'a0 self,
        user_id: u64,
        restrict: Option<Restrict>,
        with_auth: bool,
    ) -> Result<EmptyObject, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/user/follow/add");
        let mut data: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            data.push(stringify!(user_id), user_id);
        }
        {
            let restrict = restrict.unwrap_or(Restrict::Public);
            data.push(stringify!(restrict), restrict);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(user_follow_add),
            "/v1/user/follow/add"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::POST,
                &url,
                None,
                None,
                Some(data),
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<EmptyObject>(r).await
    }
    pub async fn user_follow_delete<'a0>(
        &'a0 self,
        user_id: u64,
        with_auth: bool,
    ) -> Result<EmptyObject, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/user/follow/delete");
        let mut data: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            data.push(stringify!(user_id), user_id);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(user_follow_delete),
            "/v1/user/follow/delete"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::POST,
                &url,
                None,
                None,
                Some(data),
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<EmptyObject>(r).await
    }
    pub async fn user_edit_ai_show_settings<'a0, 'a1>(
        &'a0 self,
        setting: &'a1 str,
        with_auth: bool,
    ) -> Result<EmptyObject, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/user/ai-show-settings/edit");
        let mut data: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            data.push("show_ai", setting);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(user_edit_ai_show_settings),
            "/v1/user/ai-show-settings/edit"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::POST,
                &url,
                None,
                None,
                Some(data),
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<EmptyObject>(r).await
    }
    pub async fn illust_related<'a0, 'a1, 'a2, 'a3>(
        &'a0 self,
        illust_id: u64,
        filter: Option<Filter>,
        seed_illust_ids: Option<&'a1 [String]>,
        offset: Option<&'a2 str>,
        viewed: Option<&'a3 [String]>,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v2/illust/related");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            params.push(stringify!(illust_id), illust_id);
        }
        {
            let filter = filter.unwrap_or(Filter::ForIos);
            params.push(stringify!(filter), filter);
        }
        {
            let seed_illust_ids = seed_illust_ids.unwrap_or(&[]);
            params.push("seed_illust_ids[]", seed_illust_ids);
        }
        {
            params.push(stringify!(offset), offset);
        }
        {
            let viewed = viewed.unwrap_or(&[]);
            params.push("viewed[]", viewed);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(illust_related),
            "/v2/illust/related"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
    pub async fn illust_bookmark_add<'a0, 'a1>(
        &'a0 self,
        illust_id: u64,
        restrict: Option<Restrict>,
        tags: Option<&'a1 [String]>,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v2/illust/bookmark/add");
        let mut data: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            data.push(stringify!(illust_id), illust_id);
        }
        {
            let restrict = restrict.unwrap_or(Restrict::Public);
            data.push(stringify!(restrict), restrict);
        }
        {
            let tags = tags.map(|t| t.join(" "));
            data.push("tags[]", tags);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(illust_bookmark_add),
            "/v2/illust/bookmark/add"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::POST,
                &url,
                None,
                None,
                Some(data),
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
    pub async fn novel_recommended<'a0, 'a1, 'a2, 'a3, 'a4>(
        &'a0 self,
        include_ranking_label: Option<bool>,
        filter: Option<Filter>,
        offset: Option<&'a1 str>,
        include_ranking_novels: Option<bool>,
        already_recommended: Option<&'a2 [String]>,
        max_bookmark_id_for_recommend: Option<&'a3 str>,
        include_privacy_policy: Option<&'a4 str>,
        with_auth: bool,
    ) -> Result<ParsedJson, crate::pixiv_api::error::PixivError> {
        let url = format!("{}{}", self.hosts, "/v1/novel/recommended");
        let mut params: kv_pairs::KVPairs<'_> = kv_pairs::kv_pairs![];
        {
            let include_ranking_label = include_ranking_label.unwrap_or(true);
            params.push(stringify!(include_ranking_label), include_ranking_label);
        }
        {
            let filter = filter.unwrap_or(Filter::ForIos);
            params.push(stringify!(filter), filter);
        }
        {
            params.push(stringify!(offset), offset);
        }
        {
            params.push(stringify!(include_ranking_novels), include_ranking_novels);
        }
        {
            let already_recommended = already_recommended.map(|arr| arr.join(","));
            params.push(stringify!(already_recommended), already_recommended);
        }
        {
            params.push(
                stringify!(max_bookmark_id_for_recommend),
                max_bookmark_id_for_recommend,
            );
        }
        {
            params.push(stringify!(include_privacy_policy), include_privacy_policy);
        }
        log::debug!(
            "API呼び出し: {} エンドポイント: {}",
            stringify!(novel_recommended),
            "/v1/novel/recommended"
        );
        let r = self
            .do_api_request(
                crate::pixiv_api::aapi::HttpMethod::GET,
                &url,
                None,
                Some(params),
                None,
                with_auth,
            )
            .await?;
        crate::pixiv_api::models::parse_response_into::<ParsedJson>(r).await
    }
}

/// Non-structured API calls (port of `AppPixivAPI` methods).
impl AppPixivAPI {
    // ---------- Illust (manual: URL by with_auth) ----------
    /// Recommended illusts. Port of `illust_recommended`. Python defaults: content_type="illust", include_ranking_label=True, filter="for_ios".
    ///
    pub async fn illust_recommended(
        &self,
        content_type: Option<IllustType>,
        include_ranking_label: Option<bool>,
        filter: Option<Filter>,
        max_bookmark_id_for_recommend: Option<&str>,
        min_bookmark_id_for_recent_illust: Option<&str>,
        offset: Option<&str>,
        include_ranking_illusts: Option<bool>,
        bookmark_illust_ids: Option<&[String]>,
        include_privacy_policy: Option<&str>,
        viewed: Option<&[String]>,
        with_auth: bool,
    ) -> Result<ParsedJson, PixivError> {
        let content_type = content_type.unwrap_or(IllustType::Illust);
        let include_ranking_label = include_ranking_label.unwrap_or(true);
        let filter = filter.unwrap_or(Filter::ForIos);
        let url = if with_auth {
            format!("{}/v1/illust/recommended", self.hosts)
        } else {
            format!("{}/v1/illust/recommended-nologin", self.hosts)
        };
        let mut params = kv_pairs!(
            "content_type" => content_type,
            "include_ranking_label" => include_ranking_label,
            "filter" => filter,
        );
        params.push(
            "max_bookmark_id_for_recommend",
            max_bookmark_id_for_recommend,
        );
        params.push(
            "min_bookmark_id_for_recent_illust",
            min_bookmark_id_for_recent_illust,
        );
        params.push("offset", offset);
        params.push("include_ranking_illusts", include_ranking_illusts);
        if let Some(v) = viewed {
            for x in v {
                params.push_owned("viewed[]", x.clone());
            }
        }
        if !with_auth {
            if let Some(ids) = bookmark_illust_ids {
                params.push_owned("bookmark_illust_ids", ids.join(","));
            }
        }
        params.push("include_privacy_policy", include_privacy_policy);
        let r = self
            .do_api_request(HttpMethod::GET, &url, None, Some(params), None, with_auth)
            .await?;
        parse_response_into(r).await
    }

    /// Novel via webview, raw HTML. Port of `webview_novel(raw=True)`.
    ///
    pub async fn webview_novel_raw(
        &self,
        novel_id: u64,
        with_auth: bool,
    ) -> Result<String, PixivError> {
        let url = format!("{}/webview/v2/novel", self.hosts);
        let params = kv_pairs!(
            "id" => novel_id,
            "viewer_version" => "20221031_ai",
        );
        let r = self
            .do_api_request(HttpMethod::GET, &url, None, Some(params), None, with_auth)
            .await?;
        let (status, text) =
            read_response_text_limited(r, MAX_PIXIV_WEBVIEW_RESPONSE_BYTES).await?;
        // ここは本文を HTML から切り出す経路なので、状態を見ずに読み進めていた。
        // その結果、取得制限が「解析できないレスポンス」として上がり、やり直せば
        // 通るものが二度と通らないものと同じ顔で並んでいた。
        Self::classify_pixiv_response(status, &text)?;
        Ok(text)
    }

    /// Novel via webview. Port of `webview_novel(raw=False)`.
    ///
    pub async fn webview_novel(
        &self,
        novel_id: u64,
        with_auth: bool,
    ) -> Result<WebviewNovel, PixivError> {
        /// Cached regex for extracting novel JSON from webview response (avoids recompiling on every call).
        static WEBVIEW_NOVEL_REGEX: LazyLock<regex::Regex> = LazyLock::new(|| {
            regex::Regex::new(r"novel:\s(\{.+\}),\s+isOwnWork").expect("valid regex")
        });

        let text = self.webview_novel_raw(novel_id, with_auth).await?;

        match WEBVIEW_NOVEL_REGEX.captures(&text).and_then(|c| c.get(1)) {
            Some(json_str) => parse_into(json_str.as_str()),
            None => Err(PixivError::UnintelligibleResponse { body: text }),
        }
    }

    /// 本文取得の応答が「読めなかった理由」を、読む前に見分ける。
    ///
    /// pixiv は取得制限を 429 で返すこともあれば、200 に error を載せて返す
    /// ことも 200 のまま HTML を返すこともある。切り出しに失敗してから
    /// 「解析できません」と言うと、間を空ければ通るものを、壊れた応答として
    /// 扱ってしまう。
    fn classify_pixiv_response(status: StatusCode, body: &str) -> Result<(), PixivError> {
        if status == StatusCode::TOO_MANY_REQUESTS || pixiv_body_is_rate_limited(body) {
            log::error!("pixiv webview rate limited ({status}): {body}");
            return Err(PixivError::RateLimited {
                body: body.to_string(),
            });
        }
        if status == StatusCode::NOT_FOUND {
            return Err(PixivError::NotFound {
                body: body.to_string(),
            });
        }
        Ok(())
    }

    /// Showcase article detail (no login required). Port of `showcase_article`. Manual: custom headers / host.
    ///
    pub async fn showcase_article(&self, showcase_id: u64) -> Result<ParsedJson, PixivError> {
        let url = "https://www.pixiv.net/ajax/showcase/article";
        let mut headers = HeaderMap::new();
        headers.insert(
            "User-Agent",
            HV::from_static(
                "Mozilla/5.0 (Windows NT 6.1; WOW64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/63.0.3239.132 Safari/537.36",
            ),
        );
        headers.insert("Referer", HV::from_static("https://www.pixiv.net"));
        let params = kv_pairs!(
            "article_id" => showcase_id,
        );
        let r = self
            .do_api_request(
                HttpMethod::GET,
                url,
                Some(headers),
                Some(params),
                None,
                false,
            )
            .await?;
        parse_response_into(r).await
    }

    /// Download URL to file. Port of `download`.
    ///
    pub async fn download(
        &self,
        url: &str,
        path: &std::path::Path,
        name: Option<&str>,
        replace: bool,
        referer: &str,
    ) -> Result<bool, PixivError> {
        let filename = name.unwrap_or_else(|| url.split('/').next_back().unwrap_or("download"));
        let filepath = path.join(filename);
        if !replace && tokio::fs::try_exists(&filepath).await.unwrap_or(false) {
            return Ok(false);
        }
        let mut res = self
            .client
            .get(url)
            .header("Referer", referer)
            .send()
            .await?;

        let mut file = tokio::fs::File::create(&filepath).await?;
        while let Some(chunk) = res.chunk().await? {
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        Ok(true)
    }
}

/// 次のページの宛先として追ってよいか。
///
/// 一覧の続きは `next_url` として**取得元の応答の中に書かれている**。そこへは
/// アクセストークンを付けて出ていくので、宛先が変わっていないことを確かめる。
/// FANBOX 側には同じ門（`allowed_api_url`）があるのに、こちらだけ素通しだった。
///
/// 期待するホストは設定中の API の基点から取る。プロキシを差した場合でも、
/// 「いま話している相手以外へは出ていかない」という条件は変わらない。
fn allowed_next_url(raw: &str, api_base: &str) -> Result<(), PixivError> {
    let expected_host = reqwest::Url::parse(api_base)
        .ok()
        .and_then(|base| base.host_str().map(str::to_string));
    let url = reqwest::Url::parse(raw).map_err(|_| PixivError::UntrustedNextUrl)?;
    let allowed = url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && expected_host.is_some()
        && url.host_str() == expected_host.as_deref();
    if allowed {
        Ok(())
    } else {
        Err(PixivError::UntrustedNextUrl)
    }
}

/// Paged API calls (NOT port of `AppPixivAPI` methods).
impl AppPixivAPI {
    /// Fetch the next page of results from a paged API response. The URL is typically from `next_url` in the previous response.
    ///
    pub async fn visit_next_url<T: DeserializeOwned>(
        &self,
        next_url: &str,
        with_auth: bool,
    ) -> Result<T, PixivError> {
        allowed_next_url(next_url, &self.hosts)?;
        let r = self
            .do_api_request(HttpMethod::GET, next_url, None, None, None, with_auth)
            .await?;
        parse_response_into(r).await
    }
}

#[cfg(test)]
mod next_url_tests {
    use super::*;

    const BASE: &str = "https://app-api.pixiv.net";

    /// 続きの宛先は取得元の応答に書かれている。そこへトークンを付けて出ていく
    /// ので、いま話している相手であることを確かめてから追う。
    #[test]
    fn a_next_url_must_stay_on_the_host_we_are_already_talking_to() {
        assert!(allowed_next_url(
            "https://app-api.pixiv.net/v1/user/novels?user_id=1&offset=30",
            BASE
        )
        .is_ok());

        for raw in [
            // 別のホスト。
            "https://evil.example/v1/user/novels",
            // 前方一致では通ってしまう形。
            "https://app-api.pixiv.net.evil.example/v1/user/novels",
            // 平文。
            "http://app-api.pixiv.net/v1/user/novels",
            // 既定でない港。
            "https://app-api.pixiv.net:8443/v1/user/novels",
            // 資格情報を混ぜた形。
            "https://user:pass@app-api.pixiv.net/v1/user/novels",
            // そもそも URL ではない。
            "not a url",
        ] {
            assert!(
                matches!(
                    allowed_next_url(raw, BASE),
                    Err(PixivError::UntrustedNextUrl)
                ),
                "追ってしまった: {raw}"
            );
        }
    }

    /// 宛先そのものは文面に混ぜない。応答が寄越した文字列をそのまま画面へ
    /// 出すことになるうえ、読む人にできることは増えない。
    #[test]
    fn the_refusal_does_not_repeat_the_address() {
        let shown = PixivError::UntrustedNextUrl.to_string();
        assert!(!shown.contains("http"));
        assert!(shown.contains("中止"));
    }
}

#[cfg(test)]
mod http_policy_tests {
    use super::*;
    use reqwest::StatusCode;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn authenticated_client_never_follows_redirects() {
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
            .bearer_auth("must-not-be-forwarded")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        redirect_server.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), target_listener.accept())
                .await
                .is_err(),
            "redirect target unexpectedly received the credentialed request"
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
