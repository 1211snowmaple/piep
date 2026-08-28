use crate::pixiv_api::aapi::AppPixivAPI;
use crate::pixiv_api::error::PixivError;
use crate::pixiv_api::web::WebPixivAPI;
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
    /// Highest resolution cover URL exposed by the App API.
    pub cover_url: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PixivNovelUser {
    pub id: u64,
    pub name: String,
    pub account: String,
    pub profile_image_url: Option<String>,
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
    let cover_url = non_empty(novel_info.image_urls.large.clone());
    let profile_image_url = non_empty(novel_info.user.profile_image_urls.medium.clone());

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
            profile_image_url,
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
        cover_url,
    })
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

/// Prefer an original/master cover returned by the webview endpoint. If that
/// endpoint only exposes a transformed thumbnail, use the App API `large` URL.
pub fn best_novel_cover(webview_url: String, app_large_url: Option<&str>) -> String {
    let webview_is_master = webview_url.contains("img-original")
        || webview_url.contains("novel-cover-original")
        || !webview_url.contains("/c/");
    if webview_is_master && !webview_url.trim().is_empty() {
        webview_url
    } else {
        app_large_url
            .filter(|url| !url.trim().is_empty())
            .unwrap_or(&webview_url)
            .to_string()
    }
}

/// 本文とアセット（表紙、挿絵）の全データを取得する
pub async fn get_novel_text_and_assets(
    novel_id: &str,
    refresh_token: &str,
) -> Result<(String, String, serde_json::Value, serde_json::Value), Box<dyn Error>> {
    let api = AppPixivAPI::new_from_refresh_token(refresh_token.to_string());
    let id_u64: u64 = novel_id.parse()?;

    // Webview用のAPIを使用して詳細メタデータを一挙に取得
    match api.webview_novel(id_u64, true).await {
        Ok(webview_novel) => Ok((
            webview_novel.text,
            webview_novel.cover_url,
            webview_novel.illusts,
            webview_novel.images,
        )),
        // 切り出しに失敗したときだけ、web の口へ降りる。
        //
        // この経路は HTML から正規表現で JSON を抜いているので、pixiv が
        // viewer を作り直した日に、ここだけが静かに壊れる。取得制限や 404 は
        // 別の話なので、そちらでは降りない - 相手を二度叩くだけになる。
        Err(PixivError::UnintelligibleResponse { .. }) => {
            log::warn!("pixiv webview から本文を切り出せません（{novel_id}）。web の口を試します");
            novel_text_from_web(novel_id).await
        }
        Err(error) => Err(error.into()),
    }
}

/// 退避路。web の `/ajax/novel/{id}` から本文を読む。
///
/// 本文の中身は webview 経路と完全に一致するので、保存する原本は変わらない。
/// ただし **web は `[pixivimage:]` を解決しない**。挿絵を参照している作品で
/// ここへ降りたときは、欠けたまま保存せずに失敗させる。0.3% の作品のために
/// 残りを諦める必要はないが、欠けたものを黙って完全なふりで保存してはいけない。
async fn novel_text_from_web(
    novel_id: &str,
) -> Result<(String, String, serde_json::Value, serde_json::Value), Box<dyn Error>> {
    let body = WebPixivAPI::new()?.novel(novel_id).await?;
    if body.references_unresolved_illusts() {
        return Err(format!(
            "作品 {novel_id} は本文に挿絵を参照していますが、退避路では挿絵を取得できません"
        )
        .into());
    }
    Ok((
        body.text,
        body.cover_url.unwrap_or_default(),
        // 参照している挿絵は無い、と分かっている。空を返すのは推測ではない。
        serde_json::Value::Object(serde_json::Map::new()),
        body.images,
    ))
}

pub fn extract_novel_id(url: &str) -> Option<String> {
    let re = Regex::new(r"novel/show\.php\?id=(\d+)|novels/(\d+)").unwrap();
    if let Some(cap) = re.captures(url) {
        return cap.get(1).or(cap.get(2)).map(|m| m.as_str().to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::best_novel_cover;

    #[test]
    fn prefers_app_large_over_transformed_webview_thumbnail() {
        let selected = best_novel_cover(
            "https://i.pximg.net/c/600x600/novel-cover-master/example.jpg".into(),
            Some("https://i.pximg.net/c/1200x1200/novel-cover-master/example.jpg"),
        );
        assert!(selected.contains("1200x1200"));
    }

    #[test]
    fn keeps_original_webview_cover() {
        let selected = best_novel_cover(
            "https://i.pximg.net/novel-cover-original/example.jpg".into(),
            Some("https://i.pximg.net/c/1200x1200/example.jpg"),
        );
        assert!(selected.contains("novel-cover-original"));
    }
}
