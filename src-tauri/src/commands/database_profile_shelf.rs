//! 正本に切り替えた棚のプロフィール更新。旧 DB と旧ファイルの配置へは戻らない。
use super::RefreshEntityProfileParams;
use crate::{
    library::{CaptureSource, EntityKind, Library, LibraryId, ProfileSave, PROFILE_REFRESH_ROUTE},
    AppState,
};
use serde_json::{json, Value};
use std::sync::Arc;

struct Profile {
    data: Value,
    media_complete: bool,
}
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())?
}
fn text<'a>(value: &'a Value, path: &str) -> Option<&'a str> {
    value.pointer(path).and_then(Value::as_str)
}
fn optional_text(value: &Value, path: &str) -> Result<Value, String> {
    match value.pointer(path) {
        None => Ok(Value::Null),
        Some(Value::Null) => Ok(json!("")),
        Some(v) if v.is_string() => Ok(v.clone()),
        _ => Err("プロフィールの文字列の型が不正です".into()),
    }
}
fn url_known(value: &Value, path: &str) -> bool {
    value
        .pointer(path)
        .is_some_and(|v| v.is_null() || v.is_string())
}
fn pixiv_person(key: &str, raw: &Value) -> Result<Profile, String> {
    let expected = key
        .parse::<u64>()
        .map_err(|_| "pixiv の作者 ID が不正です")?;
    if raw.pointer("/user/id").and_then(Value::as_u64) != Some(expected) {
        return Err("取得した作者の ID が一致しません".into());
    }
    let name = text(raw, "/user/name")
        .filter(|v| !v.trim().is_empty())
        .ok_or("取得した作者名がありません")?;
    let links = if ["webpage", "twitter_url", "twitter_account", "pawoo_url"]
        .iter()
        .all(|k| raw.get("profile").is_some_and(|p| p.get(*k).is_some()))
    {
        for path in [
            "/profile/webpage",
            "/profile/twitter_url",
            "/profile/twitter_account",
            "/profile/pawoo_url",
        ] {
            optional_text(raw, path)?;
        }
        Some(super::collect_profile_links(
            [
                format!("https://www.pixiv.net/users/{key}"),
                text(raw, "/profile/webpage").unwrap_or_default().into(),
                text(raw, "/profile/twitter_url").unwrap_or_default().into(),
                text(raw, "/profile/twitter_account")
                    .filter(|v| !v.is_empty())
                    .map(|v| format!("https://x.com/{}", v.trim_start_matches('@')))
                    .unwrap_or_default(),
                text(raw, "/profile/pawoo_url").unwrap_or_default().into(),
            ],
            text(raw, "/user/comment"),
        ))
    } else {
        None
    };
    Ok(Profile {
        data: json!({"source":"pixiv","sourceKey":key,"displayName":name,"account":optional_text(raw,"/user/account")?,"description":optional_text(raw,"/user/comment")?,"iconUrl":raw.pointer("/user/profile_image_urls/medium"),"coverUrl":raw.pointer("/profile/background_image_url"),"links":links,"stats":{"totalIllusts":raw.pointer("/profile/total_illusts").and_then(Value::as_u64),"totalManga":raw.pointer("/profile/total_manga").and_then(Value::as_u64),"totalNovels":raw.pointer("/profile/total_novels").and_then(Value::as_u64),"totalNovelSeries":raw.pointer("/profile/total_novel_series").and_then(Value::as_u64)}}),
        media_complete: url_known(raw, "/user/profile_image_urls/medium")
            && url_known(raw, "/profile/background_image_url"),
    })
}
fn fanbox_person(key: &str, raw: &Value) -> Result<Profile, String> {
    let body = raw
        .get("body")
        .filter(|v| v.is_object())
        .ok_or("FANBOX の作者の応答に本体がありません")?;
    if text(body, "/creatorId") != Some(key) {
        return Err("取得したクリエイター ID が一致しません".into());
    }
    let name = text(body, "/user/name")
        .filter(|v| !v.trim().is_empty())
        .ok_or("取得した作者名がありません")?;
    let links = body
        .get("profileLinks")
        .map(|v| {
            let list = v
                .as_array()
                .ok_or("プロフィールのリンクが配列ではありません")?;
            let list = list
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .ok_or("プロフィールのリンクが文字列ではありません")
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok::<_, String>(super::collect_profile_links(
                std::iter::once(format!("https://www.fanbox.cc/@{key}")).chain(list),
                text(body, "/description"),
            ))
        })
        .transpose()?;
    Ok(Profile {
        data: json!({"source":"fanbox","sourceKey":key,"displayName":name,"description":optional_text(body,"/description")?,"iconUrl":body.pointer("/user/iconUrl"),"coverUrl":body.get("coverImageUrl"),"links":links,"profileItems":body.get("profileItems")}),
        media_complete: url_known(body, "/user/iconUrl") && url_known(body, "/coverImageUrl"),
    })
}
fn pixiv_series(key: &str, web: Option<&Value>, app: Option<&Value>) -> Result<Profile, String> {
    if web.is_none() && app.is_none() {
        return Err("pixiv のシリーズ情報を取得できませんでした".into());
    }
    let web_info = match web
        .map(crate::pixiv_api::web::parse_novel_series)
        .transpose()
    {
        Ok(info) => info,
        Err(_) if app.is_some() => None,
        Err(error) => return Err(error.to_string()),
    };
    if web_info
        .as_ref()
        .and_then(|v| v.id.as_deref())
        .is_some_and(|v| v != key)
    {
        return Err("取得したシリーズの ID が一致しません".into());
    }
    let novels = app.and_then(|v| v.get("novels")).and_then(Value::as_array);
    for novel in novels.into_iter().flatten() {
        if let Some(id) = novel.pointer("/series/id") {
            let id = id
                .as_u64()
                .or_else(|| id.as_str().and_then(|s| s.parse::<u64>().ok()));
            if id.is_none() || id != key.parse::<u64>().ok() {
                return Err("取得した構成作品が別のシリーズを指しています".into());
            }
        }
    }
    let app_title = novels.and_then(|n| {
        n.iter().find_map(|v| {
            text(v, "/series/title")
                .or_else(|| text(v, "/seriesTitle"))
                .or_else(|| text(v, "/series_title"))
        })
    });
    let title = web_info
        .as_ref()
        .and_then(|v| v.title.as_deref())
        .or(app_title);
    let caption = web_info
        .as_ref()
        .and_then(|v| v.caption.as_deref())
        .map(|s| json!(s))
        .or_else(|| {
            app.and_then(|v| {
                v.pointer("/novel_series_detail/caption")
                    .or_else(|| v.pointer("/novelSeriesDetail/caption"))
            })
            .cloned()
        });
    let app_cover = app.and_then(|v| {
        text(v, "/novel_series_detail/cover_image_urls/large")
            .or_else(|| text(v, "/novelSeriesDetail/coverImageUrls/large"))
    });
    let cover = web_info
        .as_ref()
        .and_then(|v| v.cover_url.as_deref())
        .or(app_cover);
    let known_cover = web.is_some_and(|v| v.pointer("/body/cover").is_some_and(Value::is_null))
        || cover.is_some();
    if title.is_none()
        && web_info
            .as_ref()
            .is_none_or(|v| v.is_concluded.is_none() && v.published_content_count.is_none())
    {
        return Err("取得したシリーズの項目を解釈できません".into());
    }
    Ok(Profile {
        data: json!({"source":"pixiv","sourceKey":key,"title":title,"description":caption,"coverUrl":cover,"sampleNovelCount":novels.map(Vec::len),"isConcluded":web_info.as_ref().and_then(|v|v.is_concluded),"publishedContentCount":web_info.as_ref().and_then(|v|v.published_content_count)}),
        media_complete: known_cover,
    })
}
async fn received(
    library: Arc<Library>,
    request: LibraryId,
    slot: &'static str,
    raw: String,
) -> Result<Value, String> {
    blocking(move || {
        library
            .record_acquisition_payload(
                request,
                slot,
                &CaptureSource::Bytes(raw.as_bytes().to_vec()),
            )
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).map_err(|e| e.to_string())
    })
    .await
}
async fn fetch(
    library: Arc<Library>,
    request: LibraryId,
    params: &RefreshEntityProfileParams,
) -> Result<Profile, String> {
    match (params.entity_type.as_str(), params.source.as_str()) {
        ("person", "pixiv") => {
            let token = params
                .refresh_token
                .clone()
                .filter(|v| !v.trim().is_empty())
                .ok_or("設定で pixiv に接続してください")?;
            let id = params
                .source_key
                .parse::<u64>()
                .map_err(|_| "pixiv の作者 ID が不正です")?;
            let api = crate::downloader::session::pixiv(token).map_err(|e| e.to_string())?;
            let raw = api
                .user_detail_raw(id, None, true)
                .await
                .map_err(|e| e.to_string())?;
            pixiv_person(
                &params.source_key,
                &received(library, request, "pixiv-user-detail", raw).await?,
            )
        }
        ("person", "fanbox") => {
            let cookie = params
                .cookie
                .clone()
                .filter(|v| !v.trim().is_empty())
                .ok_or("設定で FANBOX に接続してください")?;
            let api = crate::downloader::session::fanbox(
                cookie,
                params
                    .user_agent
                    .clone()
                    .unwrap_or_else(|| "Mozilla/5.0".into()),
            )
            .map_err(|e| e.to_string())?;
            let raw = api
                .get_creator_raw(&params.source_key)
                .await
                .map_err(|e| e.to_string())?;
            fanbox_person(
                &params.source_key,
                &received(library, request, "fanbox-creator", raw).await?,
            )
        }
        ("series", "pixiv") => {
            let mut web = None;
            let mut app = None;
            let mut failure = None;
            match crate::pixiv_api::web::WebPixivAPI::new() {
                Ok(api) => match api.novel_series_raw(&params.source_key).await {
                    Ok(raw) => {
                        web = Some(
                            received(library.clone(), request, "pixiv-series-web", raw).await?,
                        );
                    }
                    Err(error) => failure = Some(error.to_string()),
                },
                Err(error) => failure = Some(error.to_string()),
            }
            if let Some(token) = params
                .refresh_token
                .as_deref()
                .filter(|v| !v.trim().is_empty())
            {
                let id = params
                    .source_key
                    .parse::<u64>()
                    .map_err(|_| "pixiv のシリーズ ID が不正です")?;
                let api = crate::downloader::session::pixiv(token.to_owned())
                    .map_err(|e| e.to_string())?;
                match api.novel_series_raw(id, None, None, true).await {
                    Ok(raw) => {
                        app = Some(received(library, request, "pixiv-series-app", raw).await?)
                    }
                    Err(error) => failure = Some(error.to_string()),
                }
            }
            if web.is_none() && app.is_none() {
                return Err(failure.unwrap_or_else(|| "pixiv のシリーズを取得できません".into()));
            }
            pixiv_series(&params.source_key, web.as_ref(), app.as_ref())
        }
        _ => Err("この取得元・種類のプロフィール更新には対応していません".into()),
    }
}

pub(super) async fn refresh(
    state: Arc<AppState>,
    library: Arc<Library>,
    params: RefreshEntityProfileParams,
) -> Result<Value, String> {
    refresh_with(
        state,
        library,
        params,
        |library, request, params| async move { fetch(library, request, &params).await },
    )
    .await
}

async fn refresh_with<F, Fut>(
    state: Arc<AppState>,
    library: Arc<Library>,
    params: RefreshEntityProfileParams,
    fetcher: F,
) -> Result<Value, String>
where
    F: FnOnce(Arc<Library>, LibraryId, RefreshEntityProfileParams) -> Fut + Send,
    Fut: std::future::Future<Output = Result<Profile, String>> + Send,
{
    let kind = match params.entity_type.as_str() {
        "person" => EntityKind::Person,
        "series" => EntityKind::Series,
        _ => return Err("プロフィールの種類が不正です".into()),
    };
    if !matches!(
        (kind, params.source.as_str()),
        (EntityKind::Person, "pixiv" | "fanbox") | (EntityKind::Series, "pixiv")
    ) {
        return Err("この取得元のプロフィールには対応していません".into());
    }
    if params.source_key.trim().is_empty()
        || (params.source == "pixiv" && !params.source_key.bytes().all(|c| c.is_ascii_digit()))
    {
        return Err("プロフィールの ID が不正です".into());
    }
    let lock = super::super::downloader::profile_save_mutex(
        &params.entity_type,
        &params.source,
        &params.source_key,
    );
    let _serialized = lock.lock().await;
    let cached = {
        let (lib, provider, key) = (
            library.clone(),
            params.source.clone(),
            params.source_key.clone(),
        );
        blocking(move || {
            lib.entity_head(kind, &provider, &key)
                .map_err(|e| e.to_string())
        })
        .await?
    };
    if !params.force.unwrap_or(false) {
        if let Some(head) = cached.filter(|h| {
            h.last_checked_at.is_some_and(|v| {
                chrono::Utc::now().timestamp_millis().saturating_sub(v) < 24 * 60 * 60 * 1000
            })
        }) {
            return serde_json::to_value(head).map_err(|e| e.to_string());
        }
    }
    let request = LibraryId::now_v7();
    let (lib, provider, key) = (
        library.clone(),
        params.source.clone(),
        params.source_key.clone(),
    );
    blocking(move || {
        lib.begin_acquisition(request, &provider, kind, &key, PROFILE_REFRESH_ROUTE)
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await?;
    let staging = std::env::temp_dir().join(format!("piep-profile-refresh-{}", request.simple()));
    let result = async {
        let profile = fetcher(library.clone(), request, params.clone()).await?;
        let checked_at = chrono::Utc::now().timestamp_millis();
        let mut images = Vec::new();
        for (slot, field) in [("icon", "iconUrl"), ("cover", "coverUrl")] {
            if let Some(url) = profile
                .data
                .get(field)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                match crate::commands::profile_images::download(&params.source, slot, url, &staging)
                    .await
                {
                    Ok(image) => images.push(image),
                    Err(error) => log::warn!("プロフィール画像を取得できません: {error}"),
                }
            }
        }
        let (lib, provider, key) = (
            library.clone(),
            params.source.clone(),
            params.source_key.clone(),
        );
        blocking(move || {
            lib.save_profile(ProfileSave {
                request,
                provider: &provider,
                key: &key,
                kind,
                data: &profile.data,
                media_complete: profile.media_complete,
                images,
                checked_at,
            })
            .map_err(|e| e.to_string())?;
            lib.entity_head(kind, &provider, &key)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "保存したプロフィールがありません".into())
        })
        .await
    }
    .await;
    if result.is_err() {
        let lib = library.clone();
        let _ = blocking(move || {
            lib.cancel_acquisition(request)
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .await;
    }
    if let Err(error) = tokio::fs::remove_dir_all(&staging).await {
        if error.kind() != std::io::ErrorKind::NotFound {
            log::warn!("プロフィールの作業フォルダーを消せません: {error}");
        }
    }
    let head = result?;
    state.shelf.forget_derived();
    serde_json::to_value(head).map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "database_profile_shelf_tests.rs"]
mod tests;
