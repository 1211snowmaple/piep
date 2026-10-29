use crate::pixiv_api::aapi::AppPixivAPI;
use crate::pixiv_api::error::PixivError;
use crate::pixiv_api::web::{NovelBodyWeb, WebPixivAPI};
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_illustrations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_detail: Option<serde_json::Value>,
}

pub async fn get_novel_detail(
    novel_id: &str,
    refresh_token: &str,
) -> Result<PixivNovelDetail, Box<dyn Error>> {
    Ok(get_novel_detail_with_web(novel_id, refresh_token).await?.0)
}

async fn get_novel_detail_with_web(
    novel_id: &str,
    refresh_token: &str,
) -> Result<(PixivNovelDetail, Option<NovelBodyWeb>), Box<dyn Error>> {
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

    let mut detail = PixivNovelDetail {
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
    };
    // The App API can return a readable novel with its caption/tags masked.
    // An inaccessible illustration says nothing about these independent fields.
    let web = if detail.caption.trim().is_empty() || detail.tags.is_empty() {
        match super::session::pixiv_web()?.novel(novel_id).await {
            Ok(body) => {
                supplement_novel_detail(&mut detail, &body.metadata);
                Some(body)
            }
            Err(error @ PixivError::RateLimited { .. }) => return Err(error.into()),
            Err(error) => {
                // Optional public details must not block a body that the
                // authenticated App API is allowed to read.
                log::warn!("pixiv {novel_id} の補足情報を確認できません: {error}");
                None
            }
        }
    } else {
        None
    };
    Ok((detail, web))
}

fn supplement_novel_detail(detail: &mut PixivNovelDetail, web: &serde_json::Value) {
    if detail.caption.trim().is_empty() {
        if let Some(caption) = web.get("description").and_then(serde_json::Value::as_str) {
            detail.caption = caption.to_string();
        }
    }
    if detail.tags.is_empty() {
        if let Some(tags) = web
            .pointer("/tags/tags")
            .and_then(serde_json::Value::as_array)
        {
            detail.tags = tags
                .iter()
                .filter_map(|tag| {
                    tag.get("tag")
                        .and_then(serde_json::Value::as_str)
                        .filter(|tag| !tag.trim().is_empty())
                        .map(|name| PixivNovelTag {
                            name: name.to_string(),
                        })
                })
                .collect();
        }
    }
    if detail.series_id.is_none() {
        if let Some(id) = web
            .pointer("/seriesNavData/seriesId")
            .filter(|id| id.is_number() || id.is_string())
        {
            detail.series_id = Some(
                id.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| id.to_string()),
            );
            detail.series_title = web
                .pointer("/seriesNavData/title")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
        }
    }
}

pub async fn get_novel_content(
    novel_id: &str,
    refresh_token: &str,
) -> Result<PixivNovelContent, Box<dyn Error>> {
    let (detail, web) = get_novel_detail_with_web(novel_id, refresh_token).await?;
    let web_detail = web.as_ref().map(|body| body.metadata.clone());
    let (text, webview_cover, mut illusts, images) =
        get_novel_text_and_assets(novel_id, refresh_token, web).await?;
    strip_placeholder_illustration_urls(&mut illusts);
    // A successful webview can still omit individual references. Fill only
    // unresolved IDs, without repeating an authenticated Web attempt.
    let unresolved = missing_illustrations(&text, &illusts)
        .into_iter()
        .filter(|reference| {
            let id = reference
                .trim_start_matches("[pixivimage:")
                .trim_end_matches(']')
                .split('-')
                .next()
                .unwrap_or("");
            illusts
                .get(id)
                .and_then(|item| item.get("webChecked"))
                .and_then(serde_json::Value::as_bool)
                != Some(true)
        })
        .collect::<Vec<_>>();
    if !unresolved.is_empty() {
        let api = super::session::pixiv(refresh_token.to_string())?;
        let recovered = resolve_web_novel_illusts(&unresolved.join(""), &api).await?;
        merge_recovered_illustrations(&mut illusts, &recovered);
    }
    let cover_url = recover_novel_cover(
        novel_id,
        best_novel_cover(webview_cover, detail.cover_url.as_deref()),
    )
    .await;
    let missing_illustrations = missing_illustrations(&text, &illusts);
    Ok(PixivNovelContent {
        detail,
        text,
        cover_url,
        illusts: Some(illusts),
        images: Some(images),
        missing_illustrations,
        web_detail,
    })
}

fn merge_recovered_illustrations(existing: &mut serde_json::Value, recovered: &serde_json::Value) {
    if !existing.is_object() {
        *existing = serde_json::json!({});
    }
    let map = existing.as_object_mut().unwrap();
    let Some(recovered) = recovered.as_object() else {
        return;
    };
    for (id, value) in recovered {
        let mut value = value.clone();
        if let Some(old) = map.get(id).cloned() {
            if value.get("visible").and_then(serde_json::Value::as_bool) == Some(false) {
                let known_page = old
                    .get("page")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(1) as usize;
                let has_saved_image = illust_entry_has_page(&old, known_page)
                    || old
                        .get("pages")
                        .and_then(serde_json::Value::as_array)
                        .is_some_and(|pages| {
                            (1..=pages.len()).any(|page| illust_entry_has_page(&old, page))
                        });
                if has_saved_image {
                    value = old;
                    value["webChecked"] = serde_json::Value::Bool(true);
                }
            } else if let Some(page) = old.get("page").and_then(serde_json::Value::as_u64) {
                if illust_entry_has_page(&old, page as usize)
                    && !illust_entry_has_page(&value, page as usize)
                {
                    map.insert(format!("{id}-{page}"), old);
                }
            } else if let (Some(old_pages), Some(new_pages)) = (
                old.get("pages").and_then(serde_json::Value::as_array),
                value
                    .get_mut("pages")
                    .and_then(serde_json::Value::as_array_mut),
            ) {
                for (index, old_page) in old_pages.iter().enumerate() {
                    let available = |page: &serde_json::Value| {
                        ["original", "regular"].iter().any(|key| {
                            page.get("urls")
                                .and_then(|urls| urls.get(key))
                                .and_then(serde_json::Value::as_str)
                                .is_some_and(|url| !url.is_empty())
                        })
                    };
                    if available(old_page)
                        && new_pages.get(index).is_none_or(|page| !available(page))
                    {
                        new_pages.resize(new_pages.len().max(index + 1), serde_json::json!({}));
                        new_pages[index] = old_page.clone();
                    }
                }
            }
        }
        map.retain(|key, _| {
            let page = key
                .strip_prefix(&format!("{id}-"))
                .and_then(|page| page.parse::<usize>().ok());
            page.is_none_or(|page| !illust_entry_has_page(&value, page))
        });
        map.insert(id.clone(), value);
    }
}

fn strip_placeholder_illustration_urls(illusts: &mut serde_json::Value) {
    fn strip(urls: Option<&mut serde_json::Value>) {
        if let Some(urls) = urls.and_then(serde_json::Value::as_object_mut) {
            urls.retain(|_, url| !url.as_str().is_some_and(is_placeholder_cover));
        }
    }
    if let Some(entries) = illusts.as_object_mut() {
        for entry in entries.values_mut() {
            strip(
                entry
                    .get_mut("illust")
                    .and_then(|illust| illust.get_mut("images")),
            );
            if let Some(pages) = entry
                .get_mut("pages")
                .and_then(serde_json::Value::as_array_mut)
            {
                for page in pages {
                    strip(page.get_mut("urls"));
                }
            }
        }
    }
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

/// Prefer an original/master cover returned by the webview endpoint. If that
/// endpoint only exposes a transformed thumbnail, use the App API `large` URL.
pub fn best_novel_cover(webview_url: String, app_large_url: Option<&str>) -> String {
    let webview_url = if is_placeholder_cover(&webview_url) {
        ""
    } else {
        webview_url.as_str()
    };
    let app_large_url = app_large_url.filter(|url| !is_placeholder_cover(url));
    let webview_is_master = webview_url.contains("img-original")
        || webview_url.contains("novel-cover-original")
        || !webview_url.contains("/c/");
    if webview_is_master && !webview_url.trim().is_empty() {
        webview_url.to_string()
    } else {
        app_large_url
            .filter(|url| !url.trim().is_empty())
            .unwrap_or(webview_url)
            .to_string()
    }
}

/// Pixiv returns this age-gate icon as a cover URL for some R-18 novels.
/// It is valid image data, so asset validation alone cannot reject it.
pub fn is_placeholder_cover(url: &str) -> bool {
    crate::source_assets::is_pixiv_placeholder_image(url)
}

/// Fetch the real web cover only when both regular API responses were gated.
pub async fn recover_novel_cover(novel_id: &str, selected: String) -> Option<String> {
    if !selected.is_empty() {
        return Some(selected);
    }
    let web = super::session::pixiv_web().ok()?;
    match web.novel(novel_id).await {
        Ok(novel) => novel.cover_url.filter(|url| !is_placeholder_cover(url)),
        Err(error) => {
            log::warn!("pixiv {novel_id} の実際の表紙を取得できません: {error}");
            None
        }
    }
}

/// 本文とアセット（表紙、挿絵）の全データを取得する
pub async fn get_novel_text_and_assets(
    novel_id: &str,
    refresh_token: &str,
    cached_web: Option<NovelBodyWeb>,
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
            match cached_web {
                Some(body) => novel_text_from_web_body(body, Some(&api)).await,
                None => {
                    let web = super::session::pixiv_web()?;
                    novel_text_from_web(novel_id, &web, Some(&api)).await
                }
            }
        }
        // 詳細 API で作品を確認した後なので、この 404 は作品の消失を
        // 意味しない。実際に R-18 の本文だけ webview が 404 を返し、
        // /ajax/novel/{id} は本文を返すことがある。
        Err(PixivError::NotFound { .. }) => {
            log::warn!("pixiv webview の本文取得が 404 でした（{novel_id}）。web の口を試します");
            match cached_web {
                Some(body) => novel_text_from_web_body(body, Some(&api)).await,
                None => {
                    let web = super::session::pixiv_web()?;
                    novel_text_after_webview_not_found(novel_id, &web, Some(&api)).await
                }
            }
        }
        Err(error) => Err(error.into()),
    }
}

/// 退避路。web の `/ajax/novel/{id}` から本文を読む。
///
/// 本文の中身は webview 経路と完全に一致するので、保存する原本は変わらない。
/// web の本文だけでは `[pixivimage:]` の URL が分からないため、参照先の
/// イラストを App API で個別取得する。公開元で閲覧できない挿絵は
/// 欠落として記録し、本文まで失わないようにする。
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
    novel_text_from_web_body(body, app_api).await
}

async fn novel_text_from_web_body(
    body: NovelBodyWeb,
    app_api: Option<&AppPixivAPI>,
) -> Result<(String, String, serde_json::Value, serde_json::Value), Box<dyn Error>> {
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
        let response = api.illust_detail(id.parse()?, true).await;
        // Space consecutive illustration requests even when one returns 404.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let value = match response {
            Ok(detail) if detail.illust.visible => illust_pages(&id, required_page, &detail.illust)
                .ok()
                .filter(|value| illust_entry_has_page(value, required_page)),
            Ok(_) | Err(PixivError::NotFound { .. }) => None,
            Err(error) => return Err(error.into()),
        };
        let value = match value {
            Some(value) => value,
            None => {
                let web = super::session::pixiv_web()?;
                recover_illustration_from_web(&id, &web).await?
            }
        };
        illusts.insert(id, value);
    }
    Ok(serde_json::Value::Object(illusts))
}

async fn recover_illustration_from_web(
    id: &str,
    web: &WebPixivAPI,
) -> Result<serde_json::Value, Box<dyn Error>> {
    Ok(match web.illustration_pages(id).await {
        Ok(Some(value)) => value,
        Ok(None) | Err(PixivError::NotFound { .. }) => {
            let mut unavailable = unavailable_illust(id, "取得用の接続でも閲覧できません");
            unavailable["webChecked"] = serde_json::Value::Bool(true);
            unavailable
        }
        Err(error) => return Err(error.into()),
    })
}

fn unavailable_illust(id: &str, reason: &str) -> serde_json::Value {
    serde_json::json!({"illustId": id, "visible": false, "reason": reason, "pages": []})
}

/// The source may return a body while some illustration references are hidden.
/// Keep the exact references so a later repair can be distinguished from a
/// complete archive without relying on the text hash.
pub fn missing_illustrations(text: &str, illusts: &serde_json::Value) -> Vec<String> {
    let re = Regex::new(r"\[pixivimage:([^\]\r\n]*)\]").unwrap();
    let numeric = Regex::new(r"^(\d+)(?:-(\d+))?$").unwrap();
    let mut missing = Vec::new();
    for captures in re.captures_iter(text) {
        let spec = captures.get(0).unwrap().as_str();
        let Some(parts) = numeric.captures(&captures[1]) else {
            if !missing.iter().any(|value| value == spec) {
                missing.push(spec.to_string());
            }
            continue;
        };
        let id = &parts[1];
        let page = parts.get(2).and_then(|m| m.as_str().parse::<usize>().ok());
        let unavailable = page == Some(0)
            || (parts.get(2).is_some() && page.is_none())
            || [
                parts.get(2).map(|_| format!("{id}-{}", page.unwrap_or(0))),
                Some(id.to_string()),
            ]
            .into_iter()
            .flatten()
            .filter_map(|key| illusts.get(&key))
            .all(|entry| !illust_entry_has_page(entry, page.unwrap_or(1)));
        if unavailable && !missing.iter().any(|value| value == spec) {
            missing.push(spec.to_string());
        }
    }
    missing
}

fn illust_entry_has_page(entry: &serde_json::Value, page: usize) -> bool {
    if entry.get("visible").and_then(|value| value.as_bool()) == Some(false) {
        return false;
    }
    let webview_image = entry.get("illust").and_then(|illust| illust.get("images"));
    let webview_page = entry.get("page").and_then(|value| value.as_u64());
    if webview_page == Some(page as u64)
        && ["original", "medium", "small"].iter().any(|size| {
            webview_image
                .and_then(|images| images.get(*size))
                .and_then(|url| url.as_str())
                .is_some_and(|url| !url.is_empty() && !is_placeholder_cover(url))
        })
    {
        return true;
    }
    entry
        .get("pages")
        .and_then(|value| value.as_array())
        .and_then(|pages| pages.get(page.saturating_sub(1)))
        .and_then(|page| page.get("urls"))
        .is_some_and(|urls| {
            ["original", "regular"].iter().any(|size| {
                urls.get(*size)
                    .and_then(|url| url.as_str())
                    .is_some_and(|url| !url.is_empty() && !is_placeholder_cover(url))
            })
        })
}

fn referenced_illust_pages(text: &str) -> Result<BTreeMap<String, usize>, Box<dyn Error>> {
    let re = Regex::new(r"\[pixivimage:(\d+)(?:-(\d+))?\]")?;
    let mut references = BTreeMap::new();
    for captures in re.captures_iter(text) {
        let id = captures[1].to_string();
        if id.parse::<u64>().is_err() {
            continue;
        }
        let page = match captures
            .get(2)
            .map_or(Ok(1), |part| part.as_str().parse::<usize>())
        {
            Ok(page) => page,
            Err(_) => continue,
        };
        if page == 0 {
            continue;
        }
        let entry = references.entry(id).or_insert(0);
        *entry = (*entry).max(page);
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

    #[test]
    fn recovered_pages_replace_page_aliases_without_duplicating_or_losing_images() {
        use serde_json::json;
        let mut old = json!({"123-1":{"page":1,"illust":{"images":{"original":"https://example.com/1.jpg"}}},"123-2":{"page":2,"visible":false}});
        let recovered = json!({"123":{"pages":[{"urls":{"original":"https://example.com/1.jpg"}},{"urls":{"original":"https://example.com/2.jpg"}}],"webChecked":true}});
        super::merge_recovered_illustrations(&mut old, &recovered);
        assert_eq!(old.as_object().unwrap().len(), 1);
        assert!(
            super::missing_illustrations("[pixivimage:123-1][pixivimage:123-2]", &old).is_empty()
        );
        let mut targets = Vec::new();
        super::super::asset_downloader::extract_download_targets(
            &json!({"illusts":old}),
            false,
            &mut targets,
        );
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].filename, "123_p0.jpg");
        let mut existing =
            json!({"123":{"pages":[{"urls":{"original":"https://example.com/1.jpg"}}]}});
        super::merge_recovered_illustrations(
            &mut existing,
            &json!({"123":{"pages":[{}, {"urls":{"original":"https://example.com/2.jpg"}}]}}),
        );
        assert!(
            super::missing_illustrations("[pixivimage:123-1][pixivimage:123-2]", &existing)
                .is_empty()
        );
        let mut masked = json!({"123":{"page":1,"illust":{"images":{"medium":"https://s.pximg.net/common/images/limit_unknown_360.png"}}}});
        super::strip_placeholder_illustration_urls(&mut masked);
        super::merge_recovered_illustrations(
            &mut masked,
            &json!({"123":{"visible":false,"webChecked":true,"pages":[]}}),
        );
        let mut targets = Vec::new();
        super::super::asset_downloader::extract_download_targets(
            &json!({"illusts":masked}),
            false,
            &mut targets,
        );
        assert!(targets.is_empty());
    }

    #[tokio::test]
    async fn an_app_hidden_illustration_can_recover_via_the_connected_web_session() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            let count = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..count]).to_lowercase();
            assert!(request.starts_with("get /ajax/illust/101/pages"));
            assert!(request.contains("cookie: session=test"));
            assert!(request.contains("user-agent: saved-download-ua"));
            let body = r#"{"error":false,"body":[{"urls":{"original":"https://example.com/101_p0.jpg"}},{"urls":{"original":"https://example.com/101_p1.jpg"}}]}"#;
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        });
        let api = WebPixivAPI::with_host(&host).unwrap().with_session(
            crate::pixiv_api::web::WebSession::new("session=test", "saved-download-ua").unwrap(),
        );
        let value = super::recover_illustration_from_web("101", &api)
            .await
            .unwrap();
        let illusts = serde_json::json!({"101":value});
        assert!(
            super::missing_illustrations("[pixivimage:101-1][pixivimage:101-2]", &illusts)
                .is_empty()
        );
        assert_eq!(illusts["101"]["pages"].as_array().unwrap().len(), 2);
        server.await.unwrap();
    }

    #[test]
    fn masked_web_illustrations_are_not_treated_as_saved_images() {
        let placeholder = serde_json::json!({"101":{"page":1,"illust":{"images":{"medium":"https://s.pximg.net/common/images/limit_unknown_360.png"}}}});
        assert_eq!(
            super::missing_illustrations("[pixivimage:101]", &placeholder),
            vec!["[pixivimage:101]"]
        );
        let hidden = serde_json::json!({"error":false,"body":[{"urls":{"regular":"https://s.pximg.net/common/images/limit_unknown_360.png"}}]});
        assert!(crate::pixiv_api::web::parse_illustration_pages(&hidden)
            .unwrap()
            .is_none());
        assert!(crate::pixiv_api::web::parse_illustration_pages(
            &serde_json::json!({"error":true,"body":null})
        )
        .unwrap()
        .is_none());
        assert!(crate::pixiv_api::web::parse_illustration_pages(
            &serde_json::json!({"error":false,"body":"unexpected"})
        )
        .is_err());
        let mixed = serde_json::json!({"body":[{"urls":{"original":"https://example.com/1.jpg"}},{"urls":{}},{"urls":{"original":"https://example.com/3.jpg"}}]});
        let pages = crate::pixiv_api::web::parse_illustration_pages(&mixed)
            .unwrap()
            .unwrap();
        let missing = super::missing_illustrations(
            "[pixivimage:101-1][pixivimage:101-2][pixivimage:101-3]",
            &serde_json::json!({"101":pages}),
        );
        assert_eq!(missing, vec!["[pixivimage:101-2]"]);
    }

    #[test]
    fn web_details_fill_masked_caption_tags_and_series_without_losing_body_or_images() {
        use serde_json::json;
        let mut detail: super::PixivNovelDetail = serde_json::from_value(json!({
            "id":101,"title":"作品","user":{"id":1,"name":"作者","account":"author"},
            "tags":[],"caption":"","create_date":"2026-10-01","text_length":12,
            "series_id":null,"series_title":null,"cover_url":null
        }))
        .unwrap();
        let body = crate::pixiv_api::web::parse_novel_body(&json!({"body":{
            "content":"本文[pixivimage:42]","description":"<p>作品の説明</p>",
            "tags":{"tags":[{"tag":"タグ一"},{"tag":"タグ二"}]},
            "seriesNavData":{"seriesId":123,"title":"シリーズ"},
            "textEmbeddedImages":{"image":{"urls":{"original":"https://example.com/image.jpg"}}}
        }}))
        .unwrap();
        super::supplement_novel_detail(&mut detail, &body.metadata);
        assert_eq!(detail.caption, "<p>作品の説明</p>");
        assert_eq!(
            detail
                .tags
                .iter()
                .map(|tag| tag.name.as_str())
                .collect::<Vec<_>>(),
            vec!["タグ一", "タグ二"]
        );
        assert_eq!(detail.series_id.as_deref(), Some("123"));
        assert_eq!(body.text, "本文[pixivimage:42]");
        assert!(body.images.get("image").is_some());
        assert!(body.metadata.get("content").is_none());
        assert!(body.metadata.get("textEmbeddedImages").is_none());
        super::supplement_novel_detail(
            &mut detail,
            &json!({"description":"別の説明","tags":{"tags":[]}}),
        );
        assert_eq!(detail.caption, "<p>作品の説明</p>");
        assert_eq!(detail.tags.len(), 2);
    }

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
        assert!(referenced_illust_pages("[pixivimage:123-0]")
            .unwrap()
            .is_empty());
        assert!(referenced_illust_pages("[pixivimage:abc]")
            .unwrap()
            .is_empty());
        assert_eq!(
            super::missing_illustrations(
                "[pixivimage:abc][pixivimage:123-0]",
                &serde_json::json!({})
            ),
            vec!["[pixivimage:abc]", "[pixivimage:123-0]"]
        );
    }

    #[test]
    fn inaccessible_illustrations_are_recorded_without_losing_text() {
        let illusts = serde_json::json!({
            "123": {"visible": false, "pages": []},
            "456": {"illustId": "456", "pages": [{"urls": {"original": "https://example.com/0.jpg"}}]},
            "789-2": {"visible": true, "page": 2, "illust": {"images": {"medium": "https://example.com/1.jpg"}}}
        });
        let missing = super::missing_illustrations(
            "[pixivimage:123]\n[pixivimage:456]\n[pixivimage:789-2]\n[pixivimage:999-0]",
            &illusts,
        );
        assert_eq!(missing, vec!["[pixivimage:123]", "[pixivimage:999-0]"]);
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

    #[test]
    fn rejects_age_gate_image_as_a_novel_cover() {
        let gate = "https://s.pximg.net/common/images/limit_r18_100.png";
        assert_eq!(best_novel_cover(gate.into(), Some(gate)), "");
        assert_eq!(
            best_novel_cover(
                gate.into(),
                Some("https://i.pximg.net/c/1200x1200/real.jpg")
            ),
            "https://i.pximg.net/c/1200x1200/real.jpg"
        );
    }
}
