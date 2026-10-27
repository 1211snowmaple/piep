use crate::pixiv_api::aapi::AppPixivAPI;
use crate::pixiv_api::error::PixivError;
use crate::pixiv_api::web::WebPixivAPI;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
    let api = super::session::pixiv(refresh_token.to_string())?;
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
    let api = super::session::pixiv(refresh_token.to_string())?;
    let id_u64: u64 = novel_id.parse()?;

    // Webview用のAPIを使用して詳細メタデータを一挙に取得
    match api.webview_novel(id_u64, true).await {
        Ok(webview_novel) => Ok((
            webview_novel.text,
            webview_novel.cover_url,
            webview_novel.illusts,
            webview_novel.images,
        )),
        // 切り出しに失敗したときは、web の口へ降りる。
        //
        // この経路は HTML から正規表現で JSON を抜いているので、pixiv が
        // viewer を作り直した日に、ここだけが静かに壊れる。取得制限では
        // 降りない - 待つべき相手を二度叩くことになる。
        Err(PixivError::UnintelligibleResponse { .. }) => {
            log::warn!("pixiv webview から本文を切り出せません（{novel_id}）。web の口を試します");
            novel_text_from_web(novel_id, &WebPixivAPI::new()?, Some(&api)).await
        }
        // 詳細 API で作品を確認した後なので、この 404 は作品の消失を
        // 意味しない。実際に R-18 の本文だけ webview が 404 を返し、
        // /ajax/novel/{id} は本文を返すことがある。
        Err(PixivError::NotFound { .. }) => {
            log::warn!("pixiv webview の本文取得が 404 でした（{novel_id}）。web の口を試します");
            novel_text_after_webview_not_found(novel_id, &WebPixivAPI::new()?, Some(&api)).await
        }
        Err(error) => Err(error.into()),
    }
}

/// 退避路。web の `/ajax/novel/{id}` から本文を読む。
///
/// 本文の中身は webview 経路と完全に一致するので、保存する原本は変わらない。
/// web の本文だけでは `[pixivimage:]` の URL が分からないため、参照先の
/// イラストを App API で個別取得する。参照先も読めない場合は、挿絵を欠いた
/// 作品を完全な保存として扱わず、該当 ID を明示して失敗させる。
async fn novel_text_after_webview_not_found(
    novel_id: &str,
    web_api: &WebPixivAPI,
    app_api: Option<&AppPixivAPI>,
) -> Result<(String, String, serde_json::Value, serde_json::Value), Box<dyn Error>> {
    match novel_text_from_web(novel_id, web_api, app_api).await {
        Err(error) if matches!(error.downcast_ref::<PixivError>(), Some(PixivError::NotFound { .. })) => {
            Err(format!(
                "作品 {novel_id} の詳細は取得できましたが、本文の取得先が両方とも 404 を返しました。pixivへの接続を確認して再試行してください"
            ).into())
        }
        result => result,
    }
}

async fn novel_text_from_web(
    novel_id: &str,
    web_api: &WebPixivAPI,
    app_api: Option<&AppPixivAPI>,
) -> Result<(String, String, serde_json::Value, serde_json::Value), Box<dyn Error>> {
    let body = web_api.novel(novel_id).await?;
    let illusts = if body.references_unresolved_illusts() {
        let api = app_api.ok_or("挿絵を取得するためのpixiv接続がありません")?;
        resolve_web_novel_illusts(&body.text, api).await?
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    };
    Ok((
        body.text,
        body.cover_url.unwrap_or_default(),
        illusts,
        body.images,
    ))
}

async fn resolve_web_novel_illusts(
    text: &str,
    api: &AppPixivAPI,
) -> Result<serde_json::Value, Box<dyn Error>> {
    let references = referenced_illust_pages(text)?;
    let mut illusts = serde_json::Map::new();
    for (id, required_page) in references {
        let detail = match api.illust_detail(id.parse()?, true).await {
            Ok(detail) => detail,
            Err(PixivError::NotFound { .. }) => {
                return Err(format!(
                    "本文は取得できましたが、参照先の挿絵 {id} が 404 を返しました"
                )
                .into());
            }
            Err(error) => return Err(error.into()),
        };
        illusts.insert(
            id.clone(),
            illust_pages(&id, required_page, &detail.illust)?,
        );
    }
    Ok(serde_json::Value::Object(illusts))
}

fn referenced_illust_pages(text: &str) -> Result<BTreeMap<String, usize>, Box<dyn Error>> {
    let re = Regex::new(r"\[pixivimage:(\d+)(?:-(\d+))?\]")?;
    let mut references = BTreeMap::new();
    let mut parsed = 0;
    for captures in re.captures_iter(text) {
        parsed += 1;
        let id = captures[1].to_string();
        let page = captures
            .get(2)
            .map_or(Ok(1), |part| part.as_str().parse::<usize>())?;
        if page == 0 {
            return Err(format!("挿絵 {id} のページ番号が不正です").into());
        }
        let entry = references.entry(id).or_insert(0);
        *entry = (*entry).max(page);
    }
    if parsed != text.matches("[pixivimage:").count() {
        return Err("本文に解釈できない挿絵参照があります".into());
    }
    Ok(references)
}

fn illust_pages(
    id: &str,
    required_page: usize,
    illust: &crate::pixiv_api::models::IllustrationInfo,
) -> Result<serde_json::Value, Box<dyn Error>> {
    let urls: Vec<&str> = if illust.meta_pages.is_empty() {
        vec![illust
            .meta_single_page
            .original_image_url
            .as_deref()
            .filter(|url| !url.is_empty())
            .unwrap_or(&illust.image_urls.large)]
    } else {
        illust
            .meta_pages
            .iter()
            .map(|page| page.image_urls.large.as_str())
            .collect()
    };
    illust_pages_from_urls(id, required_page, urls)
}

fn illust_pages_from_urls(
    id: &str,
    required_page: usize,
    urls: Vec<&str>,
) -> Result<serde_json::Value, Box<dyn Error>> {
    if required_page > urls.len() || urls.iter().any(|url| url.is_empty()) {
        return Err(format!("挿絵 {id} の {required_page} ページ目を取得できません").into());
    }
    let pages: Vec<serde_json::Value> = urls
        .into_iter()
        .take(required_page)
        .map(|url| serde_json::json!({"urls": {"original": url}}))
        .collect();
    Ok(serde_json::json!({"illustId": id, "pages": pages}))
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
    use super::{
        best_novel_cover, illust_pages_from_urls, novel_text_after_webview_not_found,
        referenced_illust_pages,
    };
    use crate::pixiv_api::web::WebPixivAPI;

    async fn one_novel_response(
        status: &'static str,
        body: &'static str,
    ) -> (WebPixivAPI, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let length = stream.read(&mut request).await.unwrap();
            assert!(String::from_utf8_lossy(&request[..length]).starts_with("GET /ajax/novel/101"));
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        (WebPixivAPI::with_host(&host).unwrap(), server)
    }

    #[tokio::test]
    async fn webview_404_can_recover_a_readable_novel_from_web() {
        let (api, server) = one_novel_response(
            "200 OK",
            r#"{"error":false,"body":{"content":"本文","coverUrl":"https://example.com/cover.jpg"}}"#,
        ).await;
        let (text, cover, _, _) = novel_text_after_webview_not_found("101", &api, None)
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(text, "本文");
        assert_eq!(cover, "https://example.com/cover.jpg");
    }

    #[tokio::test]
    async fn two_body_404s_do_not_claim_that_a_confirmed_novel_is_missing() {
        let (api, server) = one_novel_response("404 Not Found", "{}").await;
        let message = novel_text_after_webview_not_found("101", &api, None)
            .await
            .unwrap_err()
            .to_string();
        server.await.unwrap();
        assert!(message.contains("詳細は取得できました"), "{message}");
        assert!(!message.contains("削除された"), "{message}");
    }

    #[test]
    fn referenced_illustrations_keep_the_highest_requested_page() {
        let refs =
            referenced_illust_pages("[pixivimage:123-2]\n[pixivimage:123-1]\n[pixivimage:456]")
                .unwrap();
        assert_eq!(refs.get("123"), Some(&2));
        assert_eq!(refs.get("456"), Some(&1));
        assert!(referenced_illust_pages("[pixivimage:123-0]").is_err());
        assert!(referenced_illust_pages("[pixivimage:abc]").is_err());
    }

    #[test]
    fn illustration_pages_match_the_asset_downloaders_expected_shape() {
        let value = illust_pages_from_urls(
            "123",
            2,
            vec![
                "https://example.com/0.jpg",
                "https://example.com/1.jpg",
                "https://example.com/unreferenced.jpg",
            ],
        )
        .unwrap();
        assert_eq!(value["illustId"], "123");
        assert_eq!(value["pages"].as_array().unwrap().len(), 2);
        assert_eq!(
            value["pages"][1]["urls"]["original"],
            "https://example.com/1.jpg"
        );
        assert!(illust_pages_from_urls(
            "123",
            3,
            vec!["https://example.com/0.jpg", "https://example.com/1.jpg"]
        )
        .is_err());
    }

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
