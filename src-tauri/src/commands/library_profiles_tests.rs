use super::*;
use serde_json::json;
#[test]
fn person_normalization_distinguishes_unknown_fields_and_explicit_empty_values() {
    let p=pixiv_person("77",&json!({"user":{"id":77,"name":"作者","comment":null,"profile_image_urls":{"medium":null}},"profile":{"background_image_url":null,"total_novels":4}})).unwrap();
    assert_eq!(p.data["description"], "");
    assert!(p.media_complete);
    assert!(p.data["links"].is_null());
    assert_eq!(p.data["stats"]["totalNovels"], 4);
    assert!(p.data["stats"]["totalNovelSeries"].is_null());
    let missing = pixiv_person("77", &json!({"user":{"id":77,"name":"作者"}})).unwrap();
    assert!(missing.data["description"].is_null());
    assert!(!missing.media_complete);
    assert!(pixiv_person("77", &json!({"user":{"id":78,"name":"別人"}})).is_err());
    let fanbox=fanbox_person("creator",&json!({"body":{"creatorId":"creator","user":{"name":"作者","userId":"123","iconUrl":null},"coverImageUrl":null,"description":"紹介","profileLinks":[],"profileItems":[]}})).unwrap();
    assert_eq!(fanbox.data["sourceKey"], "creator");
    assert!(fanbox.media_complete);
    assert!(fanbox_person(
        "123",
        &json!({"body":{"creatorId":"creator","user":{"name":"作者"}}})
    )
    .is_err());
}
#[test]
fn a_series_uses_its_own_cover_and_preserves_unknown_counts_when_only_one_endpoint_answers() {
    let app = json!({"novels":[{"series":{"title":"シリーズ"},"image_urls":{"large":"https://example.invalid/work-cover.png"}}]});
    let p = pixiv_series("88", None, Some(&app)).unwrap();
    assert_eq!(p.data["title"], "シリーズ");
    assert_eq!(p.data["sampleNovelCount"], 1);
    assert!(p.data["publishedContentCount"].is_null());
    assert!(p.data["coverUrl"].is_null());
    assert!(!p.media_complete);
    let web = json!({"error":false,"body":{"id":"88","title":"シリーズの題","cover":null,"publishedContentCount":12,"isConcluded":true}});
    let p = pixiv_series("88", Some(&web), None).unwrap();
    assert_eq!(p.data["publishedContentCount"], 12);
    assert_eq!(p.data["isConcluded"], true);
    assert!(p.data["sampleNovelCount"].is_null());
    assert!(p.media_complete);
    assert!(pixiv_series("99", Some(&web), None).is_err());
    assert!(pixiv_series("88", None, None).is_err());
    assert!(pixiv_series("88", Some(&json!({"bad":"shape"})), Some(&app)).is_ok());
    assert!(pixiv_series(
        "88",
        None,
        Some(&json!({"novels":[{"series":{"id":99,"title":"別"}}]}))
    )
    .is_err());
}

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("piep-profile-coordinator-{}", LibraryId::now_v7())))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn state(root: &std::path::Path, library: Arc<Library>) -> Arc<AppState> {
    std::fs::create_dir_all(root.join("runtime")).unwrap();
    Arc::new(AppState::new(Arc::new(
        crate::commands::shelf::Shelf::with_test_library(library),
    )))
}
fn params(force: bool) -> RefreshEntityProfileParams {
    RefreshEntityProfileParams {
        entity_type: "person".into(),
        source: "pixiv".into(),
        source_key: "77".into(),
        force: Some(force),
        refresh_token: None,
        cookie: None,
        user_agent: None,
    }
}

#[tokio::test]
async fn the_refresh_coordinator_saves_to_the_shelf_and_uses_its_freshness_cache() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let fixture = Fixture::new();
    let library = Arc::new(Library::create(&fixture.0.join("shelf")).unwrap());
    let state = state(&fixture.0, library.clone());
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(1, 1)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let png = png.into_inner();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/icon.png", listener.local_addr().unwrap());
    let expected = png.clone();
    let server = tokio::spawn(async move {
        let (mut socket, _) =
            tokio::time::timeout(std::time::Duration::from_secs(10), listener.accept())
                .await
                .unwrap()
                .unwrap();
        let mut request = [0u8; 2048];
        let count = socket.read(&mut request).await.unwrap();
        let text = String::from_utf8_lossy(&request[..count]).to_lowercase();
        assert!(!text.contains("cookie:"));
        assert!(!text.contains("authorization:"));
        // MIME の申告でなく、検証した画像の形式を保存する。
        let header=format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",png.len());
        socket.write_all(header.as_bytes()).await.unwrap();
        socket.write_all(&png).await.unwrap();
    });
    let head=refresh_with(state.clone(),library.clone(),params(true),move|lib,request,_|async move {
        let raw=format!(" \n{{\"user\":{{\"id\":77,\"name\":\"直接取得した作者\",\"comment\":\"紹介\",\"profile_image_urls\":{{\"medium\":{}}}}},\"profile\":{{\"background_image_url\":null}},\"future\":9007199254740993}}\n",serde_json::to_string(&url).unwrap());
        pixiv_person("77",&received(lib,request,"pixiv-user-detail",raw).await?)
    }).await.unwrap();
    server.await.unwrap();
    assert_eq!(head["name"], "直接取得した作者");
    assert!(head["last_checked_at"].is_number());
    assert!(head["last_fetched_at"].is_number());
    let person = library
        .entity_head(EntityKind::Person, "pixiv", "77")
        .unwrap()
        .unwrap();
    let info = library
        .resource_info(
            person.id,
            crate::library::ResourceScope::Snapshot(person.snapshot.unwrap()),
            &crate::library::ResourceReference::Media {
                slot: "icon".into(),
            },
        )
        .unwrap();
    assert_eq!(info.mime_type.as_deref(), Some("image/png"));
    let mut bytes = Vec::new();
    library
        .read_resource(
            person.id,
            crate::library::ResourceScope::Snapshot(person.snapshot.unwrap()),
            &crate::library::ResourceReference::Media {
                slot: "icon".into(),
            },
            None,
            &mut bytes,
        )
        .unwrap();
    assert_eq!(bytes, expected);
    let cached = refresh_with(
        state.clone(),
        library.clone(),
        params(false),
        |_, _, _| async { panic!("fresh profile must not fetch") },
    )
    .await
    .unwrap();
    assert_eq!(cached, head);
    assert!(!fixture.0.join("runtime/profiles").exists());
    library.verify().unwrap();
    let old_snapshot = person.snapshot;
    assert!(refresh_with(
        state,
        library.clone(),
        params(true),
        |lib, request, _| async move {
            received(lib, request, "invalid-response", "{\"bad\":true}".into()).await?;
            Err("invalid response".into())
        }
    )
    .await
    .is_err());
    let unchanged = library
        .entity_head(EntityKind::Person, "pixiv", "77")
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.snapshot, old_snapshot);
    assert_eq!(unchanged.last_checked_at, person.last_checked_at);
    library.verify().unwrap();
}
