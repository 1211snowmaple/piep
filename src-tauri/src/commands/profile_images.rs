//! プロフィール画像の取得。控えと直接更新で、容量・画像の検証と同時取得の制限を揃える。
use crate::library::StagedAsset;
use std::path::Path;
static DOWNLOADS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

pub(crate) async fn download(
    provider: &str,
    slot: &str,
    url: &str,
    staging: &Path,
) -> Result<StagedAsset, String> {
    if !matches!(slot, "icon" | "cover") {
        return Err("プロフィール画像の用途が不正です".into());
    }
    let _permit = DOWNLOADS.acquire().await.map_err(|e| e.to_string())?;
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    tokio::fs::create_dir_all(staging)
        .await
        .map_err(|e| e.to_string())?;
    let ext = crate::downloader::asset_downloader::extract_extension(url, "jpg");
    let name = format!("{slot}.{ext}");
    let path = staging.join(&name);
    let response = client
        .get(url)
        .header(
            "referer",
            if provider == "fanbox" {
                "https://www.fanbox.cc/"
            } else {
                "https://www.pixiv.net/"
            },
        )
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;
    crate::downloader::asset_downloader::save_response_atomically(
        response,
        &path,
        crate::downloader::asset_downloader::MAX_PROFILE_IMAGE_BYTES,
        true,
    )
    .await?;
    let mime = {
        let path = path.clone();
        tauri::async_runtime::spawn_blocking(move || {
            image::ImageReader::open(path)
                .and_then(|r| r.with_guessed_format())
                .map_err(|e| e.to_string())?
                .format()
                .map(|f| f.to_mime_type().to_owned())
                .ok_or_else(|| "プロフィール画像の形式が不明です".to_owned())
        })
        .await
        .map_err(|e| e.to_string())??
    };
    Ok(StagedAsset {
        role: slot.into(),
        name,
        path,
        url: Some(url.into()),
        mime_type: Some(mime),
    })
}
