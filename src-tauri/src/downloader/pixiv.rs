use crate::pixiv_api::aapi::AppPixivAPI;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::error::Error;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PixivNovelDetail {
    pub id: u64,
    pub title: String,
    pub user: PixivNovelUser,
    pub tags: Vec<PixivNovelTag>,
    pub caption: String,
    pub create_date: String,
    pub text_length: u32,
    pub series_id: Option<String>,
    pub series_title: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PixivNovelUser {
    pub id: u64,
    pub name: String,
    pub account: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PixivNovelTag {
    pub name: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PixivNovelContent {
    pub detail: PixivNovelDetail,
    pub text: String,
    pub cover_url: Option<String>,
    pub illusts: Option<serde_json::Value>,
    pub images: Option<serde_json::Value>,
}

pub async fn get_novel_detail(
    novel_id: &str,
    refresh_token: &str,
) -> Result<PixivNovelDetail, Box<dyn Error>> {
    let api = AppPixivAPI::new_from_refresh_token(refresh_token.to_string());
    let id_u64: u64 = novel_id.parse()?;

    // APIを呼び出して小説の詳細を取得
    let detail = api.novel_detail(id_u64, true).await?;
    let novel_info = detail.novel;

    let (series_id, series_title) = match novel_info.series {
        crate::pixiv_api::models::SeriesOrEmpty::Series(series) => {
            (Some(series.id.to_string()), Some(series.title))
        }
        crate::pixiv_api::models::SeriesOrEmpty::Empty(_) => (None, None),
    };

    Ok(PixivNovelDetail {
        id: novel_info.id,
        title: novel_info.title,
        user: PixivNovelUser {
            id: novel_info.user.id,
            name: novel_info.user.name,
            account: novel_info.user.account,
        },
        tags: novel_info
            .tags
            .into_iter()
            .map(|t| PixivNovelTag { name: t.name })
            .collect(),
        caption: novel_info.caption,
        create_date: novel_info.create_date,
        text_length: novel_info.text_length.try_into().unwrap_or(0),
        series_id,
        series_title,
    })
}

#[allow(dead_code)]
pub async fn get_novel_text(novel_id: &str, refresh_token: &str) -> Result<String, Box<dyn Error>> {
    let api = AppPixivAPI::new_from_refresh_token(refresh_token.to_string());
    let id_u64: u64 = novel_id.parse()?;

    // Webview用のAPIを使用して本文を取得
    let webview_novel = api.webview_novel(id_u64, true).await?;

    Ok(webview_novel.text)
}

/// 本文とアセット（表紙、挿絵）の全データを取得する
pub async fn get_novel_text_and_assets(
    novel_id: &str,
    refresh_token: &str,
) -> Result<(String, String, serde_json::Value, serde_json::Value), Box<dyn Error>> {
    let api = AppPixivAPI::new_from_refresh_token(refresh_token.to_string());
    let id_u64: u64 = novel_id.parse()?;

    // Webview用のAPIを使用して詳細メタデータを一挙に取得
    let webview_novel = api.webview_novel(id_u64, true).await?;

    Ok((
        webview_novel.text,
        webview_novel.cover_url,
        webview_novel.illusts,
        webview_novel.images,
    ))
}

pub fn extract_novel_id(url: &str) -> Option<String> {
    let re = Regex::new(r"novel/show\.php\?id=(\d+)|novels/(\d+)").unwrap();
    if let Some(cap) = re.captures(url) {
        return cap.get(1).or(cap.get(2)).map(|m| m.as_str().to_string());
    }
    None
}
