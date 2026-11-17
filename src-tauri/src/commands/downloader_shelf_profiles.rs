//! 作品に載っているプロフィールを控える。旧 DB と旧プロフィールのフォルダーには書かない。
use crate::library::work_profiles::{work_profile_seeds, WorkProfileSeed};
use crate::library::{Library, LibraryId, StagedAsset};
use serde_json::Value;
use std::{path::Path, sync::Arc};
static ICON_DOWNLOADS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

async fn icon(seed: &WorkProfileSeed, staging: &Path) -> Result<Vec<StagedAsset>, String> {
    let Some(url) = seed
        .data
        .get("iconUrl")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return Ok(vec![]);
    };
    let _permit = ICON_DOWNLOADS.acquire().await.map_err(|e| e.to_string())?;
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    tokio::fs::create_dir_all(staging)
        .await
        .map_err(|e| e.to_string())?;
    let ext = crate::downloader::asset_downloader::extract_extension(url, "jpg");
    let name = format!("icon.{ext}");
    let path = staging.join(&name);
    let response = client
        .get(url)
        .header(
            "referer",
            if seed.provider == "fanbox" {
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
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    crate::downloader::asset_downloader::save_response_atomically(
        response,
        &path,
        crate::downloader::asset_downloader::MAX_PROFILE_IMAGE_BYTES,
        true,
    )
    .await?;
    Ok(vec![StagedAsset {
        role: "icon".into(),
        name,
        path,
        url: Some(url.into()),
        mime_type: mime,
    }])
}

/// 控えが作れなくても作品の保存は成功として返す。詳しい取得の失敗とは分けて記録する。
pub(super) async fn save(library: Arc<Library>, provider: &str, data: &Value) -> bool {
    let seeds = match work_profile_seeds(provider, data) {
        Ok(seeds) => seeds,
        Err(error) => {
            log::warn!("作品からプロフィールの控えを作れません: {error}");
            return false;
        }
    };
    let mut changed = false;
    for seed in seeds {
        let kind = if seed.kind == crate::library::EntityKind::Person {
            "person"
        } else {
            "series"
        };
        let guard = super::profile_save_mutex(kind, &seed.provider, &seed.key);
        let _serialized = guard.lock().await;
        let request = LibraryId::now_v7();
        let begun = {
            let (library, seed, work) = (Arc::clone(&library), seed.clone(), data.clone());
            super::shelf::blocking(move || {
                library
                    .begin_work_profile_seed(request, &seed, &work)
                    .map_err(|e| e.to_string())
            })
            .await
        };
        let ticket = match begun {
            Ok(Some(ticket)) => ticket,
            Ok(None) => continue,
            Err(error) => {
                log::warn!("プロフィールの控えを始められません: {error}");
                continue;
            }
        };
        let staging = std::env::temp_dir().join(format!("piep-profile-seed-{}", request.simple()));
        let images = match icon(&seed, &staging).await {
            Ok(images) => images,
            Err(error) => {
                log::warn!("プロフィールの控えの画像を取得できません: {error}");
                vec![]
            }
        };
        let completed = {
            let library = Arc::clone(&library);
            super::shelf::blocking(move || {
                library
                    .complete_work_profile_seed(&ticket, &seed, images)
                    .map_err(|e| e.to_string())
            })
            .await
        };
        match completed {
            Ok(_) => changed = true,
            Err(error) => log::warn!("プロフィールの控えを保存できません: {error}"),
        }
        if let Err(error) = tokio::fs::remove_dir_all(&staging).await {
            if error.kind() != std::io::ErrorKind::NotFound {
                log::warn!("プロフィールの作業フォルダーを消せません: {error}");
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{EntityKind, ResourceReference, ResourceScope, WorkSave};
    use serde_json::json;
    use std::{fs, path::PathBuf};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(
                std::env::temp_dir().join(format!("piep-profile-seed-app-{}", LibraryId::now_v7())),
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    async fn server(body: Vec<u8>, mime: &'static str) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/icon.png", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) =
                tokio::time::timeout(std::time::Duration::from_secs(10), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
            let mut request = [0u8; 2048];
            let count = socket.read(&mut request).await.unwrap();
            let text = String::from_utf8_lossy(&request[..count]).to_lowercase();
            assert!(text.contains("referer: https://www.pixiv.net/"));
            assert!(!text.contains("cookie:"));
            let headers=format!("HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
            socket.write_all(headers.as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
        });
        (url, task)
    }
    fn data(url: &str) -> Value {
        json!({"detail":{"title":"作品","user":{"id":77,"name":"作者","profile_image_url":url},"series_id":88,"series_title":"連作"},"text":"本文"})
    }

    #[tokio::test]
    async fn the_real_save_adapter_publishes_profiles_and_images_once_without_legacy_files() {
        let fixture = Fixture::new();
        let library = Arc::new(Library::create(&fixture.0.join("shelf")).unwrap());
        library.switch_over().unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1, 1)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let png = png.into_inner();
        let (url, server) = server(png.clone(), "image/png").await;
        let data = data(&url);
        library
            .save_work(WorkSave {
                request: LibraryId::now_v7(),
                provider: "pixiv",
                key: "1",
                route: "test",
                data: &data,
                assets: vec![],
            })
            .unwrap();
        assert!(save(library.clone(), "pixiv", &data).await);
        server.await.unwrap();
        // サーバーは終了済み。同じ作品を保存しても画像へ再接続せず、版も増やさない。
        assert!(!save(library.clone(), "pixiv", &data).await);
        let person = library
            .entity_head(EntityKind::Person, "pixiv", "77")
            .unwrap()
            .unwrap();
        assert_eq!(person.name, "作者");
        assert_eq!(person.version_number, Some(1));
        assert_eq!(person.work_count, 1);
        assert!(person.icon.is_some());
        assert_eq!(person.last_checked_at, None);
        assert_eq!(person.last_fetched_at, None);
        let series = library
            .entity_head(EntityKind::Series, "pixiv", "88")
            .unwrap()
            .unwrap();
        assert_eq!(series.name, "連作");
        assert_eq!(series.version_number, Some(1));
        assert!(series.cover.is_none());
        assert_eq!(series.is_concluded, None);
        assert_eq!(series.published_content_count, None);
        let mut bytes = Vec::new();
        library
            .read_resource(
                person.id,
                ResourceScope::Snapshot(person.snapshot.unwrap()),
                &ResourceReference::Media {
                    slot: "icon".into(),
                },
                None,
                &mut bytes,
            )
            .unwrap();
        assert_eq!(bytes, png);
        assert!(!fixture.0.join("profiles").exists());
        assert!(!fixture.0.join("series").exists());
        assert!(!fixture.0.join("piep.db").exists());
        library.verify().unwrap();
    }

    #[tokio::test]
    async fn an_invalid_icon_is_left_missing_without_rejecting_the_profile_or_saved_work() {
        let fixture = Fixture::new();
        let library = Arc::new(Library::create(&fixture.0.join("shelf")).unwrap());
        let (url, server) = server(b"<html>error</html>".to_vec(), "text/html").await;
        let data = data(&url);
        library
            .save_work(WorkSave {
                request: LibraryId::now_v7(),
                provider: "pixiv",
                key: "1",
                route: "test",
                data: &data,
                assets: vec![],
            })
            .unwrap();
        assert!(save(library.clone(), "pixiv", &data).await);
        server.await.unwrap();
        let person = library
            .entity_head(EntityKind::Person, "pixiv", "77")
            .unwrap()
            .unwrap();
        assert!(person.icon.is_none());
        assert!(person.snapshot.is_some());
        assert_eq!(library.works(5).unwrap().len(), 1);
        assert!(library
            .entity_by_key(EntityKind::Person, "pixiv", "77")
            .unwrap()
            .is_some());
        library.verify().unwrap();
    }
}
