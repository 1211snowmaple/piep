//! サーバーモードを通しで確かめる。本物の HTTP で話し、本物のファイルと DB を動かす。

use super::desktop::{DesktopServer, EmbeddedSettings};
use super::http::{
    admin_router, combined_router, reader_router, AdminAsset, AdminWeb, ConnectionInfo,
};
use super::standalone::{assemble, command_admin_pair, command_init, ServerPaths, StandaloneParts};
use super::store::{random_token, Role, ServerStore};
use super::sync_receive::{BindEntry, CommitRequest, ManifestEntry, SyncError};
use super::sync_send::{
    admin_post, pair_admin, prepare_snapshot, send_prepared, RemoteServer, SendHandle,
};
use super::API_VERSION;
use crate::database::queries::EntityProfileFreshness;
use crate::database::{Database, NewDownload, WorkCollectionInput, WorkCollectionMemberInput};
use crate::AppState;
use reqwest::{header, StatusCode};
use sha2::{Digest, Sha256};
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub(super) struct TestRoot(pub(super) PathBuf);

impl TestRoot {
    pub(super) fn new(label: &str) -> Self {
        Self(std::env::temp_dir().join(format!("piep_server_{label}_{}", random_token(9))))
    }

    pub(super) fn created(label: &str) -> Self {
        let root = Self::new(label);
        fs::create_dir_all(&root.0).expect("test root");
        root
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        crate::database::tantivy_index::release_runtime(&self.0.join("app-data").join("downloads"));
        crate::database::tantivy_index::release_runtime(
            &self.0.join("server").join("library").join("downloads"),
        );
        for attempt in 0..6 {
            if fs::remove_dir_all(&self.0).is_ok() || !self.0.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5 << attempt));
        }
    }
}

struct Fixture {
    app_data: PathBuf,
    work_id: i64,
    collection_id: String,
    state: Arc<AppState>,
}

fn write_image(path: &Path, color: [u8; 3]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("image directory");
    image::RgbImage::from_pixel(4, 4, image::Rgb(color))
        .save(path)
        .expect("test image");
}

fn add_work(
    db: &Database,
    storage: &Path,
    source_id: &str,
    title: &str,
    body: &str,
) -> (i64, PathBuf) {
    let work_dir = storage.join("pixiv").join(source_id).join("v1");
    fs::create_dir_all(&work_dir).expect("work directory");
    let work_file = work_dir.join("original.json");
    let cover_file = work_dir.join("cover.png");
    write_image(&cover_file, [40, 120, 180]);
    fs::write(&work_file, serde_json::json!({ "text": body }).to_string()).expect("work body");
    // 実データには `\\?\` つきのパスが多い。同じ形で試す。
    let cover = cover_file
        .canonicalize()
        .expect("cover path")
        .to_string_lossy()
        .into_owned();
    let id = db
        .upsert_download(&NewDownload {
            source: "pixiv".into(),
            source_id: source_id.into(),
            title: title.into(),
            author_name: "試験作者".into(),
            author_id: "test-author".into(),
            content_type: "novel".into(),
            tags: vec!["試験".into()],
            excerpt: None,
            cover_path: Some(cover),
            json_path: work_file.to_string_lossy().into_owned(),
            original_json_path: Some(work_file.to_string_lossy().into_owned()),
            asset_count: 0,
            file_size_bytes: 0,
            downloaded_at: "2026-10-10T00:00:00Z".into(),
            source_created_at: None,
            content_hash: Some(format!("{source_id}-content")),
            text_length: body.chars().count() as i64,
            source_updated_at: None,
            watch_updates: false,
            current_version: 1,
            favorite: false,
        })
        .expect("work registration");
    db.upsert_download_person(id, "pixiv", "test-author", "author", "試験作者")
        .expect("author");
    db.reindex_download(id).expect("work index");
    (id, work_file)
}

fn fixture(root: &Path) -> Fixture {
    let app_data = root.join("app-data");
    let storage = app_data.join("downloads");
    fs::create_dir_all(&storage).expect("storage");
    let db = Database::open(&app_data.join("piep.db"), &storage).expect("database");
    let (work_id, _) = add_work(
        &db,
        &storage,
        "server-test",
        "サーバー接続試験",
        "第一ページ [newpage] 第二ページの本文",
    );
    db.upsert_download_person(work_id, "pixiv", "other-author", "illustrator", "別作者")
        .expect("second author");
    db.upsert_download_series(work_id, "pixiv", "test-series", "試験シリーズ", Some(2))
        .expect("series");
    db.reindex_download(work_id).expect("work index");

    let icon = app_data.join("profiles/pixiv/test-author/v1/icon.png");
    write_image(&icon, [180, 80, 120]);
    let profile_json = icon.with_file_name("profile.json");
    fs::write(&profile_json, "{}").expect("profile");
    db.upsert_person_profile(
        "pixiv",
        "test-author",
        "試験作者",
        icon.to_str(),
        None,
        None,
        None,
        "test-profile-content",
        profile_json.to_str().unwrap(),
        1,
        0,
        EntityProfileFreshness::RemoteChecked,
    )
    .expect("author profile");
    db.upsert_download_person(work_id, "fanbox", "banner-only", "author", "背景だけの作者")
        .unwrap();
    let banner = app_data.join("profiles/fanbox/banner-only/v1/cover.png");
    write_image(&banner, [30, 80, 100]);
    db.upsert_person_profile(
        "fanbox",
        "banner-only",
        "背景だけの作者",
        None,
        banner.to_str(),
        None,
        None,
        "banner-hash",
        profile_json.to_str().unwrap(),
        1,
        0,
        EntityProfileFreshness::RemoteChecked,
    )
    .unwrap();
    let series_cover = app_data.join("series/pixiv/test-series/v1/cover.png");
    write_image(&series_cover, [80, 180, 120]);
    let series_json = series_cover.with_file_name("series.json");
    fs::write(&series_json, "{}").expect("series profile file");
    db.upsert_series_profile(
        "pixiv",
        "test-series",
        "試験シリーズ",
        None,
        series_cover.to_str(),
        "test-series-content",
        series_json.to_str().unwrap(),
        1,
        0,
        EntityProfileFreshness::RemoteChecked,
        None,
        None,
    )
    .expect("series profile");
    let collection_cover = app_data.join("collection-covers/cover.png");
    write_image(&collection_cover, [120, 80, 180]);
    let collection = db
        .upsert_work_collection(&WorkCollectionInput {
            name: "試験コレクション".into(),
            collection_kind: "ordered".into(),
            cover_mode: Some("file".into()),
            cover_image_path: Some(collection_cover.to_string_lossy().into_owned()),
            ..Default::default()
        })
        .expect("collection");
    db.add_work_collection_members(
        &collection.summary.id,
        &[WorkCollectionMemberInput {
            source: "pixiv".into(),
            source_id: "server-test".into(),
            title_snapshot: None,
            author_snapshot: None,
            position: None,
            member_role: None,
            added_by: None,
            pinned: None,
            note: None,
        }],
    )
    .expect("collection member");
    Fixture {
        app_data,
        work_id,
        collection_id: collection.summary.id,
        state: Arc::new(AppState::new(db)),
    }
}

/// 読み手の API を一通り叩く。埋め込みでも単独でも同じ結果になるはずのもの。
async fn assert_read_api(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    work_id: i64,
    collection_id: &str,
    private_root: &Path,
) {
    let private_root = private_root.to_string_lossy().replace('\\', "\\\\");
    let search = client
        .post(format!("{url}/api/v1/works/search"))
        .bearer_auth(token)
        .json(&serde_json::json!({ "text": "サーバー接続試験", "limit": 30 }))
        .send()
        .await
        .expect("search");
    assert_eq!(search.status(), StatusCode::OK);
    assert_eq!(
        search.headers().get(header::CACHE_CONTROL).unwrap(),
        "private, no-store"
    );
    let results: serde_json::Value = search.json().await.expect("search JSON");
    assert_eq!(results["items"][0]["id"], work_id);
    assert_eq!(results["items"][0]["authorId"], "test-author");
    assert_eq!(results["items"][0]["hasCover"], true);
    assert!(!results.to_string().contains(&private_root));

    let authors: serde_json::Value = client
        .get(format!(
            "{url}/api/v1/entities/people?text=%E8%A9%A6%E9%A8%93"
        ))
        .bearer_auth(token)
        .send()
        .await
        .expect("authors")
        .json()
        .await
        .expect("authors JSON");
    assert_eq!(authors["items"][0]["sourceKey"], "test-author");
    assert_eq!(authors["items"][0]["hasCover"], true);
    assert!(!authors.to_string().contains(&private_root));

    let banner_person: serde_json::Value = client
        .get(format!(
            "{url}/api/v1/entities/people/detail?source=fanbox&key=banner-only"
        ))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(banner_person["hasCover"], false);
    assert_eq!(banner_person["hasBanner"], true);
    assert_eq!(
        client
            .get(format!(
                "{url}/api/v1/entities/people/cover?source=fanbox&key=banner-only"
            ))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        client
            .get(format!(
                "{url}/api/v1/entities/people/cover?source=fanbox&key=banner-only&variant=banner"
            ))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    for endpoint in [
        "/api/v1/search/suggest?text=試験",
        "/api/v1/works/lookup?source=pixiv&key=server-test",
    ] {
        assert_eq!(
            client
                .get(format!("{url}{endpoint}"))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let value: serde_json::Value = client
            .get(format!("{url}{endpoint}"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(!value.to_string().contains(&private_root));
        if endpoint.contains("suggest") {
            assert!(!value["items"].as_array().unwrap().is_empty());
        } else {
            assert_eq!(value["id"], work_id);
        }
    }
    let missing: serde_json::Value = client
        .get(format!(
            "{url}/api/v1/works/lookup?source=pixiv&key=missing"
        ))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(missing.is_null());

    let sorted: serde_json::Value = client
        .get(format!("{url}/api/v1/entities/people?sort=name&order=asc"))
        .bearer_auth(token)
        .send()
        .await
        .expect("sorted authors")
        .json()
        .await
        .expect("sorted authors JSON");
    assert!(sorted["total"].as_i64().unwrap() >= 1);
    let bad_sort = client
        .get(format!("{url}/api/v1/entities/people?sort=json_path"))
        .bearer_auth(token)
        .send()
        .await
        .expect("bad sort");
    assert_eq!(bad_sort.status(), StatusCode::BAD_REQUEST);

    let person: serde_json::Value = client
        .get(format!(
            "{url}/api/v1/entities/people/detail?source=pixiv&key=test-author"
        ))
        .bearer_auth(token)
        .send()
        .await
        .expect("person detail")
        .json()
        .await
        .expect("person detail JSON");
    assert_eq!(person["sourceKey"], "test-author");
    assert_eq!(person["tags"][0]["name"], "試験");
    assert!(person["seriesCount"].is_number());
    assert!(!person.to_string().contains(&private_root));

    let series_page: serde_json::Value = client
        .get(format!(
            "{url}/api/v1/entities/people/series?source=pixiv&key=test-author&sort=name&order=asc"
        ))
        .bearer_auth(token)
        .send()
        .await
        .expect("person series")
        .json()
        .await
        .expect("person series JSON");
    assert!(series_page["items"].is_array());
    assert!(!series_page.to_string().contains(&private_root));
    let series_of_series = client
        .get(format!(
            "{url}/api/v1/entities/series/series?source=pixiv&key=test-series"
        ))
        .bearer_auth(token)
        .send()
        .await
        .expect("series of series");
    assert_eq!(series_of_series.status(), StatusCode::NOT_FOUND);

    let series_detail: serde_json::Value = client
        .get(format!(
            "{url}/api/v1/entities/series/detail?source=pixiv&key=test-series"
        ))
        .bearer_auth(token)
        .send()
        .await
        .expect("series detail")
        .json()
        .await
        .expect("series detail JSON");
    assert_eq!(series_detail["sourceKey"], "test-series");
    assert!(!series_detail.to_string().contains(&private_root));

    for path in [
        format!("works/{work_id}/cover?size=large"),
        "entities/people/cover?source=pixiv&key=test-author".to_string(),
        "entities/series/cover?source=pixiv&key=test-series".to_string(),
        format!("works/{work_id}/cover"),
        format!("collections/{collection_id}/cover/file"),
        format!("collections/{collection_id}/cover/tile-0"),
    ] {
        let image = client
            .get(format!("{url}/api/v1/{path}"))
            .bearer_auth(token)
            .send()
            .await
            .expect("image");
        assert_eq!(image.status(), StatusCode::OK, "{path}");
        assert_eq!(
            image.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/webp",
            "{path}"
        );
        assert!(
            !image.bytes().await.expect("image bytes").is_empty(),
            "{path}"
        );
    }
    let invalid_slot = client
        .get(format!(
            "{url}/api/v1/collections/{collection_id}/cover/tile-4"
        ))
        .bearer_auth(token)
        .send()
        .await
        .expect("invalid slot");
    assert_eq!(invalid_slot.status(), StatusCode::NOT_FOUND);

    let collections: serde_json::Value = client
        .get(format!("{url}/api/v1/collections"))
        .bearer_auth(token)
        .send()
        .await
        .expect("collections")
        .json()
        .await
        .expect("collections JSON");
    assert_eq!(collections[0]["id"], collection_id);
    assert_eq!(collections[0]["hasFileCover"], true);
    assert!(!collections.to_string().contains(&private_root));
    let detail: serde_json::Value = client
        .get(format!("{url}/api/v1/collections/{collection_id}"))
        .bearer_auth(token)
        .send()
        .await
        .expect("collection")
        .json()
        .await
        .expect("collection JSON");
    assert_eq!(detail["members"].as_array().unwrap().len(), 1);
    let memberships: serde_json::Value = client
        .get(format!("{url}/api/v1/works/{work_id}/collections"))
        .bearer_auth(token)
        .send()
        .await
        .expect("memberships")
        .json()
        .await
        .expect("memberships JSON");
    assert_eq!(memberships[0]["id"], collection_id);
    assert_eq!(memberships[0]["position"], 0);

    let metadata: serde_json::Value = client
        .get(format!("{url}/api/v1/works/{work_id}"))
        .bearer_auth(token)
        .send()
        .await
        .expect("metadata")
        .json()
        .await
        .expect("metadata JSON");
    assert_eq!(metadata["work"]["title"], "サーバー接続試験");
    assert_eq!(metadata["people"].as_array().unwrap().len(), 3);
    assert_eq!(metadata["series"][0]["key"], "test-series");
    assert!(!metadata.to_string().contains(&private_root));

    let page: serde_json::Value = client
        .get(format!("{url}/api/v1/works/{work_id}/pages/1"))
        .bearer_auth(token)
        .send()
        .await
        .expect("page")
        .json()
        .await
        .expect("page JSON");
    assert_eq!(page["pageCount"], 2);
    assert!(page["html"].as_str().unwrap().contains("第二ページ"));
    assert!(!page.to_string().contains(&private_root));
}

async fn assert_reading_api(client: &reqwest::Client, url: &str, token: &str) {
    let put = |page: i64, position: f64, device_time: i64| {
        client
            .put(format!("{url}/api/v1/reading/pixiv/server-test"))
            .bearer_auth(token)
            .json(&serde_json::json!({ "page": page, "position": position, "deviceTime": device_time }))
            .send()
    };
    let stored = put(2, 0.5, 2_000).await.expect("reading put");
    assert_eq!(stored.status(), StatusCode::OK);
    let stored: serde_json::Value = stored.json().await.expect("reading JSON");
    assert_eq!(stored["page"], 2);
    // 古い時刻の書き込みは、新しい位置を巻き戻さない。
    let older: serde_json::Value = put(1, 0.1, 1_000)
        .await
        .expect("older put")
        .json()
        .await
        .expect("older JSON");
    assert_eq!(older["page"], 2);
    assert_eq!(
        put(1, 1.5, 3_000).await.expect("bad put").status(),
        StatusCode::BAD_REQUEST
    );
    let listed: serde_json::Value = client
        .get(format!("{url}/api/v1/reading?since=0"))
        .bearer_auth(token)
        .send()
        .await
        .expect("reading list")
        .json()
        .await
        .expect("reading list JSON");
    assert_eq!(listed["items"][0]["sourceId"], "server-test");
    assert_eq!(listed["items"][0]["page"], 2);
    let cursor = listed["cursor"].as_i64().expect("cursor");
    let empty: serde_json::Value = client
        .get(format!("{url}/api/v1/reading?since={cursor}"))
        .bearer_auth(token)
        .send()
        .await
        .expect("reading empty")
        .json()
        .await
        .expect("reading empty JSON");
    assert_eq!(empty["items"].as_array().unwrap().len(), 0);
}

async fn pair_with(client: &reqwest::Client, url: &str, secret: &str) -> serde_json::Value {
    let paired = client
        .post(format!("{url}/api/v1/pair"))
        .json(&serde_json::json!({ "secret": secret, "deviceName": "test Android" }))
        .send()
        .await
        .expect("pair");
    assert_eq!(paired.status(), StatusCode::OK);
    paired.json().await.expect("pair JSON")
}

#[tokio::test]
async fn embedded_https_requires_pairing_and_revocation_takes_effect() {
    let root = TestRoot::new("embedded");
    let fixture = fixture(&root.0);
    let server = DesktopServer::new(&fixture.app_data).expect("server");
    server
        .start_embedded(
            Arc::clone(&fixture.state),
            EmbeddedSettings {
                ip: Some("127.0.0.1".into()),
                ..Default::default()
            },
            0,
        )
        .await
        .expect("start");
    let status = server.status().await;
    let url = status
        .embedded_url
        .clone()
        .expect("url")
        .trim_end_matches('/')
        .to_string();

    let probe = reqwest::Client::builder()
        .tls_backend_rustls()
        .tls_danger_accept_invalid_certs(true)
        .tls_info(true)
        .no_proxy()
        .build()
        .expect("probe client");
    let mut hello = None;
    for _ in 0..40 {
        match probe.get(format!("{url}/api/v1/hello")).send().await {
            Ok(response) => {
                hello = Some(response);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    let hello = hello.expect("listener became ready");
    let certificate = hello
        .extensions()
        .get::<reqwest::tls::TlsInfo>()
        .and_then(|info| info.peer_certificate())
        .expect("certificate")
        .to_vec();
    let fingerprint = Sha256::digest(&certificate)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(Some(fingerprint), status.embedded_fingerprint);
    let body: serde_json::Value = hello.json().await.expect("hello JSON");
    assert_eq!(body["apiVersion"], API_VERSION);
    assert_eq!(body["serverName"], "piep");

    let client = reqwest::Client::builder()
        .tls_backend_rustls()
        .tls_certs_only([reqwest::Certificate::from_der(&certificate).expect("certificate")])
        .tls_danger_accept_invalid_hostnames(true)
        .https_only(true)
        .no_proxy()
        .build()
        .expect("pinned client");

    let unauthorized = client
        .get(format!("{url}/api/v1/works/{}", fixture.work_id))
        .send()
        .await
        .expect("request");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let pairing = server.create_reader_pairing().await.expect("pairing");
    let payload: serde_json::Value = serde_json::from_str(&pairing.payload).expect("payload");
    assert_eq!(payload["version"], 2);
    assert_eq!(payload["role"], "reader");
    let credentials = pair_with(&client, &url, payload["secret"].as_str().expect("secret")).await;
    let token = credentials["token"].as_str().expect("token");
    let device_id = credentials["deviceId"].as_str().expect("device id");

    // 一回限りの鍵は、使ったら終わり。
    let again = client
        .post(format!("{url}/api/v1/pair"))
        .json(&serde_json::json!({ "secret": payload["secret"], "deviceName": "another" }))
        .send()
        .await
        .expect("second pair");
    assert_eq!(again.status(), StatusCode::UNAUTHORIZED);

    assert_read_api(
        &client,
        &url,
        token,
        fixture.work_id,
        &fixture.collection_id,
        &fixture.app_data,
    )
    .await;
    assert_reading_api(&client, &url, token).await;

    // 内蔵サーバーに送信の口は無い。読み手の鍵では管理の API も使えない。
    let admin = client
        .get(format!("{url}/api/v1/admin/status"))
        .bearer_auth(token)
        .send()
        .await
        .expect("admin");
    assert_eq!(admin.status(), StatusCode::FORBIDDEN);
    let unknown = client
        .get(format!("{url}/api/v1/works/{}", fixture.work_id + 1000))
        .bearer_auth(token)
        .send()
        .await
        .expect("unknown work");
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    server.revoke_device(device_id).await.expect("revoke");
    let revoked = client
        .get(format!("{url}/api/v1/works/{}", fixture.work_id))
        .bearer_auth(token)
        .send()
        .await
        .expect("revoked");
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);

    server.stop_embedded().await.expect("stop");
    assert!(probe
        .get(format!("{url}/api/v1/hello"))
        .send()
        .await
        .is_err());
    assert!(!server.status().await.embedded.enabled);
}

fn spawn_plain(router: axum::Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        axum_server::from_tcp(listener)
            .expect("listener")
            .serve(router.into_make_service())
            .await
            .expect("serve");
    });
    format!("http://{address}")
}

fn standalone(root: &Path) -> (StandaloneParts, ServerPaths) {
    let paths = ServerPaths::new(&root.join("server")).expect("paths");
    fs::create_dir_all(paths.state()).expect("state");
    (assemble(&paths, None, None).expect("assemble"), paths)
}

#[tokio::test]
async fn protected_admin_routes_reject_before_receiving_the_body() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let root = TestRoot::created("admin-body-auth");
    let (parts, _) = standalone(&root.0);
    let secret = parts
        .context
        .store
        .issue_pairing(Role::Reader, Duration::from_secs(60))
        .unwrap();
    let reader = parts
        .context
        .store
        .consume_pairing(&secret, "reader")
        .unwrap();
    let url = spawn_plain(combined_router(Arc::clone(&parts.context)));
    for (method, path) in [
        ("POST", "/api/v1/admin/pairings"),
        ("POST", "/api/v1/admin/sync/sessions/unused/manifest"),
        ("POST", "/api/v1/admin/sync/sessions/unused/bind"),
        ("POST", "/api/v1/admin/sync/sessions/unused/commit"),
        ("POST", "/api/v1/admin/favorites/ack"),
        (
            "PUT",
            "/api/v1/admin/sync/sessions/unused/blobs/unused?offset=0&total=4194304",
        ),
        ("PUT", "/api/v1/admin/settings/server-name"),
    ] {
        for (token, status) in [
            (None, 401),
            (Some("wrong"), 401),
            (Some(reader.token.as_str()), 403),
        ] {
            let address = url.strip_prefix("http://").unwrap();
            let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
            let authorization = token
                .map(|token| format!("Authorization: Bearer {token}\r\n"))
                .unwrap_or_default();
            // 宣言した本文を一切送らない。extractor が先なら応答は返ってこない。
            let headers = format!("{method} {path} HTTP/1.1\r\nHost: {address}\r\n{authorization}Content-Type: application/json\r\nContent-Length: 4194304\r\nConnection: close\r\n\r\n");
            stream.write_all(headers.as_bytes()).await.unwrap();
            let mut response = [0; 4096];
            let read = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut response))
                .await
                .expect("authorization must not wait for the body")
                .unwrap();
            let response = String::from_utf8_lossy(&response[..read]);
            assert!(
                response.starts_with(&format!("HTTP/1.1 {status} ")),
                "{path}: {response}"
            );
        }
    }
}

#[tokio::test]
async fn favorite_routes_keep_device_and_admin_authorization_separate() {
    let root = TestRoot::created("favorite-authorization");
    let (parts, _) = standalone(&root.0);
    let store = &parts.context.store;
    let reader = store
        .consume_pairing(
            &store
                .issue_pairing(Role::Reader, Duration::from_secs(60))
                .unwrap(),
            "reader",
        )
        .unwrap();
    let admin = store
        .consume_pairing(
            &store
                .issue_pairing(Role::Admin, Duration::from_secs(60))
                .unwrap(),
            "admin",
        )
        .unwrap();
    let url = spawn_plain(combined_router(Arc::clone(&parts.context)));
    let reader_url = spawn_plain(reader_router(Arc::clone(&parts.context)));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for token in [None, Some("wrong")] {
        let request = client
            .put(format!("{url}/api/v1/favorites/pixiv/test"))
            .json(&serde_json::json!({ "favorite": true, "deviceTime": 1 }));
        let request = match token {
            Some(token) => request.bearer_auth(token),
            None => request,
        };
        assert_eq!(
            request.send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    for (token, status) in [
        (None, StatusCode::UNAUTHORIZED),
        (Some(reader.token.as_str()), StatusCode::FORBIDDEN),
        (Some(admin.token.as_str()), StatusCode::OK),
    ] {
        let request = client.get(format!("{url}/api/v1/admin/favorites?since=0"));
        let request = match token {
            Some(token) => request.bearer_auth(token),
            None => request,
        };
        assert_eq!(request.send().await.unwrap().status(), status);
        let request = client
            .post(format!("{url}/api/v1/admin/favorites/ack"))
            .json(&serde_json::json!({ "cursor": 0 }));
        let request = match token {
            Some(token) => request.bearer_auth(token),
            None => request,
        };
        assert_eq!(request.send().await.unwrap().status(), status);
    }
    assert_eq!(
        client
            .get(format!("{reader_url}/api/v1/admin/favorites"))
            .bearer_auth(&admin.token)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND,
    );
    store.revoke_device(&reader.device_id).unwrap();
    assert_eq!(
        put_favorite(&client, &url, &reader.token, "test", true, 2)
            .await
            .status(),
        StatusCode::UNAUTHORIZED,
    );
}

#[test]
fn entity_series_cursor_is_rejected_after_reopening_the_library() {
    let root = TestRoot::created("entity-series-reopen");
    let first = fixture(&root.0);
    let (second_id, _) = add_work(
        &first.state.db,
        &first.app_data.join("downloads"),
        "second-series-work",
        "別作品",
        "本文",
    );
    first
        .state
        .db
        .upsert_download_series(second_id, "pixiv", "second-series", "別シリーズ", Some(1))
        .unwrap();
    let page = first
        .state
        .db
        .list_entity_series_paged(
            "pixiv",
            "test-author",
            None,
            Some("work_count"),
            Some("desc"),
            1,
            None,
        )
        .unwrap();
    let cursor = page.next_cursor.expect("more than one series");
    let old_generation = first.state.db.library_generation().unwrap();
    let app_data = first.app_data.clone();
    drop(first);

    let reopened = Database::open(&app_data.join("piep.db"), &app_data.join("downloads")).unwrap();
    assert_eq!(
        old_generation,
        reopened.library_generation().unwrap(),
        "data_version alone cannot identify a reopened database"
    );
    assert!(reopened
        .list_entity_series_paged(
            "pixiv",
            "test-author",
            None,
            Some("work_count"),
            Some("desc"),
            1,
            Some(&cursor)
        )
        .unwrap_err()
        .contains("Library changed"));
    let fresh = reopened
        .list_entity_series_paged(
            "pixiv",
            "test-author",
            None,
            Some("work_count"),
            Some("desc"),
            1,
            None,
        )
        .unwrap();
    assert_eq!(fresh.total, 2);
    assert!(reopened
        .list_entity_series_paged(
            "pixiv",
            "test-author",
            None,
            Some("work_count"),
            Some("desc"),
            1,
            fresh.next_cursor.as_deref()
        )
        .is_ok());
}

#[tokio::test]
async fn an_existing_admin_is_not_replaced_without_asking() {
    let root = TestRoot::created("admin-guard");
    let url = Some("https://server.example/".to_string());
    let first: serde_json::Value =
        serde_json::from_str(&command_init(&root.0, url.clone()).await.expect("init"))
            .expect("payload");
    let paths = ServerPaths::new(&root.0).expect("paths");
    let store = ServerStore::open(&paths.store()).expect("store");
    let admin = store
        .consume_pairing(first["secret"].as_str().expect("secret"), "今の管理者")
        .expect("pair");

    let refused = command_admin_pair(&root.0, url.clone(), false).await;
    assert!(refused.expect_err("refused").contains("--replace"));
    assert!(command_init(&root.0, url.clone()).await.is_err());

    let replacing = command_admin_pair(&root.0, url, true)
        .await
        .expect("replace");
    assert!(replacing.contains("secret"));
    assert!(
        store.authorize(&admin.token).is_some(),
        "発行しただけでは締め出さない"
    );
}

fn admin_secret(paths: &ServerPaths) -> String {
    ServerStore::open(&paths.store())
        .expect("store")
        .issue_pairing(Role::Admin, Duration::from_secs(60))
        .expect("pairing")
}

async fn send(
    state: &Arc<AppState>,
    app_data: &Path,
    outgoing: &Path,
    remote: &RemoteServer,
    token: &str,
) -> super::sync_receive::CommitOutcome {
    let snapshot = outgoing.join("piep-snapshot.db");
    let (report, files, skipped) = prepare_snapshot(state, app_data, &snapshot)
        .await
        .expect("snapshot");
    assert_eq!(report.outside, 0, "app data の外を指すパスがあります");
    assert_eq!(skipped, 0);
    send_prepared(remote, token, &snapshot, files, &SendHandle::new())
        .await
        .expect("send")
}

/// 索引の追いつきを待つ。
async fn wait_for_search(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    text: &str,
    expected: i64,
) -> serde_json::Value {
    let mut last = serde_json::Value::Null;
    for _ in 0..600 {
        let response = client
            .post(format!("{url}/api/v1/works/search"))
            .bearer_auth(token)
            .json(&serde_json::json!({ "text": text, "limit": 30 }))
            .send()
            .await
            .expect("search");
        assert_eq!(response.status(), StatusCode::OK);
        last = response.json().await.expect("search JSON");
        if last["items"]
            .as_array()
            .is_some_and(|items| items.len() as i64 == expected)
        {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("索引が追いつきません: {last}");
}

#[tokio::test]
async fn a_library_sent_from_the_desktop_can_be_read_from_the_standalone_server() {
    let root = TestRoot::new("send");
    let fixture = fixture(&root.0);
    let (parts, paths) = standalone(&root.0);
    let url = spawn_plain(combined_router(Arc::clone(&parts.context)));
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("client");
    let remote = RemoteServer {
        url: format!("{url}/"),
        fingerprint: None,
    };
    let outgoing = root.0.join("outgoing");

    // 届く前は「準備中」と答える。読み手の登録はできるが本棚は無い。
    let token = pair_admin(&remote, &admin_secret(&paths), "test desktop")
        .await
        .expect("pair admin");
    let empty = client
        .post(format!("{url}/api/v1/works/search"))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "text": "x", "limit": 1 }))
        .send()
        .await
        .expect("search before send");
    assert_eq!(empty.status(), StatusCode::SERVICE_UNAVAILABLE);

    let first = send(
        &fixture.state,
        &fixture.app_data,
        &outgoing,
        &remote,
        &token,
    )
    .await;
    assert!(!first.unchanged);
    assert_eq!(first.generation, 1);
    assert_eq!(first.works, 1);
    assert!(
        first.files_added >= 7,
        "files_added = {}",
        first.files_added
    );
    assert_eq!(first.missing_referenced, 0);

    // 携帯の登録文字列は管理者が発行する。
    let pairing = admin_post(
        &remote,
        &token,
        "pairings",
        &serde_json::json!({ "url": "https://piep.example.test/" }),
    )
    .await
    .expect("reader pairing");
    let payload: serde_json::Value =
        serde_json::from_str(pairing["payload"].as_str().expect("payload")).expect("payload JSON");
    assert_eq!(payload["role"], "reader");
    assert_eq!(payload["url"], "https://piep.example.test/");
    let credentials = pair_with(&client, &url, payload["secret"].as_str().expect("secret")).await;
    let reader = credentials["token"]
        .as_str()
        .expect("reader token")
        .to_string();

    let results = wait_for_search(&client, &url, &reader, "サーバー接続試験", 1).await;
    let work_id = results["items"][0]["id"].as_i64().expect("id");
    assert_read_api(
        &client,
        &url,
        &reader,
        work_id,
        &fixture.collection_id,
        &root.0,
    )
    .await;
    assert_reading_api(&client, &url, &reader).await;

    // 何も変えなければ、何も動かさない。
    let second = send(
        &fixture.state,
        &fixture.app_data,
        &outgoing,
        &remote,
        &token,
    )
    .await;
    assert!(second.unchanged, "{second:?}");
    assert_eq!(second.generation, 1);

    // 1 作品足すと、そのファイルだけが増える。
    let storage = fixture.app_data.join("downloads");
    let (_, new_body) = add_work(
        &fixture.state.db,
        &storage,
        "server-test-2",
        "二つ目の作品",
        "追加された本文",
    );
    let third = send(
        &fixture.state,
        &fixture.app_data,
        &outgoing,
        &remote,
        &token,
    )
    .await;
    assert_eq!(third.generation, 2);
    assert_eq!(third.works, 2);
    assert_eq!(third.files_added, 2, "{third:?}");
    assert_eq!(third.files_removed, 0);
    wait_for_search(&client, &url, &reader, "二つ目の作品", 1).await;
    // 入れ替えのあとも、鍵と読書位置は生きている。
    let listed: serde_json::Value = client
        .get(format!("{url}/api/v1/reading?since=0"))
        .bearer_auth(&reader)
        .send()
        .await
        .expect("reading")
        .json()
        .await
        .expect("reading JSON");
    assert_eq!(listed["items"][0]["sourceId"], "server-test");
    assert!(paths.library().join("piep.db.previous").exists());

    // 手元で消えたファイルは、捨てずに library-trash へ退ける。
    fs::remove_file(&new_body).expect("remove local body");
    let fourth = send(
        &fixture.state,
        &fixture.app_data,
        &outgoing,
        &remote,
        &token,
    )
    .await;
    assert_eq!(fourth.files_removed, 1, "{fourth:?}");
    assert!(!paths
        .library()
        .join("downloads/pixiv/server-test-2/v1/original.json")
        .exists());
    assert_eq!(
        fourth.missing_referenced, 2,
        "json_path と original_json_path の 2 つ"
    );
    let trashed = fs::read_dir(paths.trash())
        .expect("trash")
        .flatten()
        .next()
        .expect("trash batch")
        .path();
    assert!(trashed
        .join("downloads/pixiv/server-test-2/v1/original.json")
        .exists());

    // 管理の状態。パスや鍵は含まない。
    let status: serde_json::Value = client
        .get(format!("{url}/api/v1/admin/status"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("status")
        .json()
        .await
        .expect("status JSON");
    assert_eq!(status["sync"]["generation"], fourth.generation);
    assert_eq!(status["works"], 2);
    assert!(!status.to_string().contains(&token));
    assert!(!status.to_string().contains("secret"));
    parts.receiver.stop_index_catch_up();
}

#[tokio::test]
async fn the_public_side_refuses_what_it_should() {
    let root = TestRoot::new("public");
    let (parts, paths) = standalone(&root.0);
    let url = spawn_plain(combined_router(Arc::clone(&parts.context)));
    let reader_url = spawn_plain(reader_router(Arc::clone(&parts.context)));
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("client");
    let remote = RemoteServer {
        url: url.clone(),
        fingerprint: None,
    };
    let token = pair_admin(&remote, &admin_secret(&paths), "test desktop")
        .await
        .expect("pair admin");

    // 無認証で答えるのは hello と pair だけ。
    assert_eq!(
        client
            .get(format!("{url}/api/v1/hello"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    for path in [
        "works/1",
        "works/1/pages/0",
        "works/1/outline",
        "works/1/assets/1",
        "works/1/cover",
        "works/1/collections",
        "entities/people",
        "entities/people/cover?source=pixiv&key=a",
        "entities/people/detail?source=pixiv&key=a",
        "entities/people/series?source=pixiv&key=a",
        "collections",
        "collections/a",
        "collections/a/cover/file",
        "reading",
        "admin/status",
        "admin/devices",
    ] {
        let response = client
            .get(format!("{url}/api/v1/{path}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        let wrong = client
            .get(format!("{url}/api/v1/{path}"))
            .bearer_auth("wrong")
            .send()
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED, "{path}");

        // 更新を頼めるのは管理者だけ。手で置いたサーバーは自分では更新できない。
        let update_url = format!("{url}/api/v1/admin/update");
        assert_eq!(
            client.post(&update_url).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let refused = client
            .post(&update_url)
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::CONFLICT);
    }
    assert_eq!(
        client
            .get(format!("{url}/secret"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );

    // 管理の口を分けると、読み手の口には管理の API が存在しない。
    let split = client
        .get(format!("{reader_url}/api/v1/admin/status"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(split.status(), StatusCode::NOT_FOUND);
    let body: serde_json::Value = split.json().await.unwrap();
    assert_eq!(body["code"], "not_found");
    assert_eq!(
        client
            .get(format!("{reader_url}/api/v1/hello"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    // 要求の大きさの上限。
    let huge = "x".repeat(70 * 1024);
    let search = client
        .post(format!("{url}/api/v1/works/search"))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "text": huge }))
        .send()
        .await
        .unwrap();
    assert_eq!(search.status(), StatusCode::PAYLOAD_TOO_LARGE);

    // 経路が付けた利用者名は、認証にも記録にも使わない。
    let forged = client
        .get(format!("{url}/api/v1/collections"))
        .header("Tailscale-User-Login", "someone@example.test")
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), StatusCode::UNAUTHORIZED);
    let listed = client
        .get(format!("{url}/api/v1/admin/devices"))
        .bearer_auth(&token)
        .header("Tailscale-User-Login", "someone@example.test")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!listed.contains("someone@example.test"));

    // 読み手の鍵では、送信も端末の管理もできない。
    let secret = ServerStore::open(&paths.store())
        .unwrap()
        .issue_pairing(Role::Reader, Duration::from_secs(60))
        .unwrap();
    let reader = pair_with(&client, &url, &secret).await;
    let reader_token = reader["token"].as_str().unwrap();
    let open = client
        .post(format!("{url}/api/v1/admin/sync/sessions"))
        .bearer_auth(reader_token)
        .send()
        .await
        .unwrap();
    assert_eq!(open.status(), StatusCode::FORBIDDEN);

    // 送信の一覧に、置き場の外を指すパスを混ぜても通らない。
    let opened: serde_json::Value = client
        .post(format!("{url}/api/v1/admin/sync/sessions"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session = opened["sessionId"].as_str().unwrap();
    for hostile in [
        "../outside.txt",
        "downloads/../../outside.txt",
        "C:/Windows/win.ini",
        "\\\\server\\share\\a",
        "state/state.db",
        "downloads/a/../../state/state.db",
    ] {
        let response = client
            .post(format!(
                "{url}/api/v1/admin/sync/sessions/{session}/manifest"
            ))
            .bearer_auth(&token)
            .json(&serde_json::json!({ "files": [{ "path": hostile, "size": 1, "mtimeMs": 0 }] }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{hostile}");
    }
    let bad_sha = client
        .get(format!(
            "{url}/api/v1/admin/sync/sessions/{session}/blobs/..%2F..%2Fstate"
        ))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(bad_sha.status(), StatusCode::BAD_REQUEST);

    // 登録の試行は 1 分に 10 回まで。止めた試行では鍵を使い切らない。
    let secret = ServerStore::open(&paths.store())
        .unwrap()
        .issue_pairing(Role::Reader, Duration::from_secs(60))
        .unwrap();
    for _ in 0..10 {
        client
            .post(format!("{url}/api/v1/pair"))
            .json(&serde_json::json!({ "secret": "wrong", "deviceName": "x" }))
            .send()
            .await
            .unwrap();
    }
    let limited = client
        .post(format!("{url}/api/v1/pair"))
        .json(&serde_json::json!({ "secret": secret, "deviceName": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(limited.status(), StatusCode::UNAUTHORIZED);
    assert!(ServerStore::open(&paths.store())
        .unwrap()
        .consume_pairing(&secret, "x")
        .is_ok());
}

#[test]
fn an_interrupted_send_can_resume_and_a_bad_one_changes_nothing() {
    let root = TestRoot::new("resume");
    let (parts, _paths) = standalone(&root.0);
    let receiver = &parts.receiver;
    let content = b"resumable file content".to_vec();
    let sha = Sha256::digest(&content)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let entry = ManifestEntry {
        path: "downloads/pixiv/1/a.bin".into(),
        size: content.len() as u64,
        mtime_ms: 5,
    };

    let opened = receiver.open_session().expect("open");
    assert!(!opened.resumed);
    assert_eq!(
        receiver
            .add_manifest(&opened.session_id, std::slice::from_ref(&entry))
            .expect("manifest"),
        vec![entry.path.clone()]
    );
    let state = receiver
        .write_blob(
            &opened.session_id,
            &sha,
            0,
            content.len() as u64,
            &content[..4],
        )
        .expect("chunk");
    assert_eq!((state.received, state.complete), (4, false));

    // つなぎ直すと同じ送信の続きになり、受け取った分から再開できる。
    let resumed = receiver.open_session().expect("reopen");
    assert!(resumed.resumed);
    assert_eq!(resumed.session_id, opened.session_id);
    let state = receiver
        .blob_state(&resumed.session_id, &sha)
        .expect("state");
    assert_eq!(state.received, 4);
    // 位置が合わないかたまりは書かず、今の長さを返す。
    let mismatch = receiver
        .write_blob(
            &resumed.session_id,
            &sha,
            9,
            content.len() as u64,
            &content[9..],
        )
        .expect("mismatch");
    assert_eq!(mismatch.received, 4);
    receiver
        .add_manifest(&resumed.session_id, std::slice::from_ref(&entry))
        .expect("manifest again");
    let state = receiver
        .write_blob(
            &resumed.session_id,
            &sha,
            4,
            content.len() as u64,
            &content[4..],
        )
        .expect("rest");
    assert!(state.complete);

    // 中身がハッシュと違えば受け取らない。
    let other = "0".repeat(64);
    assert!(matches!(
        receiver.write_blob(&resumed.session_id, &other, 0, 3, b"abc"),
        Err(SyncError::Conflict(_))
    ));

    // DB が上がっていなければ確定しない。本棚にも何も起きない。
    receiver
        .bind_files(
            &resumed.session_id,
            &[BindEntry {
                path: entry.path.clone(),
                sha256: sha.clone(),
            }],
        )
        .expect("bind");
    let refused = receiver.commit(
        &resumed.session_id,
        &CommitRequest {
            database_sha256: "1".repeat(64),
        },
    );
    assert!(matches!(refused, Err(SyncError::Conflict(_))));
    assert!(parts.db.current().is_none());
    assert!(!root
        .0
        .join("server/library/downloads/pixiv/1/a.bin")
        .exists());

    // 紐づいていないパスは結びつけられない。
    assert!(matches!(
        receiver.bind_files(
            &resumed.session_id,
            &[BindEntry {
                path: "downloads/other".into(),
                sha256: sha
            }]
        ),
        Err(SyncError::Invalid(_))
    ));

    receiver.abort_session(&resumed.session_id).expect("abort");
    assert!(!root
        .0
        .join("server/incoming")
        .join(&resumed.session_id)
        .exists());
    assert!(matches!(
        receiver.blob_state(&resumed.session_id, &"0".repeat(64)),
        Err(SyncError::NotFound)
    ));
}

#[test]
fn an_uploaded_invalid_database_does_not_overwrite_live_files() {
    let root = TestRoot::new("invalid_snapshot");
    let (parts, paths) = standalone(&root.0);
    let receiver = &parts.receiver;
    let relative = "downloads/pixiv/1/original.json";
    let live = paths.library().join(relative);
    fs::create_dir_all(live.parent().unwrap()).unwrap();
    fs::write(&live, b"previous content").unwrap();
    let content = b"replacement content";
    let invalid_database = b"not a SQLite database";
    let hash = |bytes: &[u8]| {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let sha = hash(content);
    let database_sha = hash(invalid_database);
    let opened = receiver.open_session().unwrap();
    let session = &opened.session_id;
    receiver
        .add_manifest(
            session,
            &[ManifestEntry {
                path: relative.into(),
                size: content.len() as u64,
                mtime_ms: 1,
            }],
        )
        .unwrap();
    for (sha, bytes) in [
        (&sha, content.as_slice()),
        (&database_sha, invalid_database.as_slice()),
    ] {
        receiver
            .write_blob(session, sha, 0, bytes.len() as u64, bytes)
            .unwrap();
    }
    receiver
        .bind_files(
            session,
            &[BindEntry {
                path: relative.into(),
                sha256: sha.clone(),
            }],
        )
        .unwrap();
    let request = CommitRequest {
        database_sha256: database_sha.clone(),
    };
    assert!(matches!(
        receiver.commit(session, &request),
        Err(SyncError::Internal(_))
    ));
    assert_eq!(fs::read(&live).unwrap(), b"previous content");
    assert_eq!(receiver.status().generation, 0);
    assert!(receiver.blob_state(session, &sha).unwrap().complete);
    assert!(
        receiver
            .blob_state(session, &database_sha)
            .unwrap()
            .complete
    );
    // 再試行でも、検証を通らない写しが本棚へ触ることはない。
    assert!(matches!(
        receiver.commit(session, &request),
        Err(SyncError::Internal(_))
    ));
    assert_eq!(fs::read(&live).unwrap(), b"previous content");
}

#[test]
#[cfg(windows)]
fn a_database_install_failure_restores_files_and_keeps_uploaded_blobs() {
    use std::os::windows::fs::OpenOptionsExt;
    for failure in ["database", "file"] {
        let root = TestRoot::new("failed_commit");
        let (parts, paths) = standalone(&root.0);
        let db = Database::open(&paths.database(), &paths.storage()).unwrap();
        let (_, original) = add_work(&db, &paths.storage(), "rollback", "元の作品", "元の本文");
        let old_content = fs::read(&original).unwrap();
        let incoming = root.0.join("snapshot.db");
        db.write_vacuumed_copy(&incoming).unwrap();
        super::snapshot::prepare_for_send(&incoming, &paths.library()).unwrap();
        drop(db);
        parts.db.open_existing().unwrap();
        let receiver = &parts.receiver;
        let session = receiver.open_session().unwrap().session_id;
        let relative = original
            .strip_prefix(paths.library())
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let content = r#"{"text":"新しい本文"}"#.as_bytes();
        let database = fs::read(&incoming).unwrap();
        let hash = |bytes: &[u8]| {
            Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let sha = hash(content);
        let database_sha = hash(&database);
        receiver
            .add_manifest(
                &session,
                &[ManifestEntry {
                    path: relative.clone(),
                    size: content.len() as u64,
                    mtime_ms: 1,
                }],
            )
            .unwrap();
        for (sha, bytes) in [(&sha, content), (&database_sha, database.as_slice())] {
            receiver
                .write_blob(&session, sha, 0, bytes.len() as u64, bytes)
                .unwrap();
        }
        receiver
            .bind_files(
                &session,
                &[BindEntry {
                    path: relative,
                    sha256: sha.clone(),
                }],
            )
            .unwrap();
        // DB の控えを作れない状態にする。実データを触らず、入れ替え時のI/O失敗を再現する。
        let blocker = paths.database().with_extension("db.previous");
        let file_lock = if failure == "file" {
            Some(
                fs::OpenOptions::new()
                    .read(true)
                    .share_mode(0)
                    .open(&original)
                    .unwrap(),
            )
        } else {
            fs::create_dir(&blocker).unwrap();
            None
        };
        let request = CommitRequest {
            database_sha256: database_sha,
        };
        assert!(receiver.commit(&session, &request).is_err());
        drop(file_lock);
        assert_eq!(fs::read(&original).unwrap(), old_content);
        assert_eq!(receiver.status().generation, 0);
        assert_eq!(
            parts
                .db
                .current()
                .unwrap()
                .get_stats()
                .unwrap()
                .total_downloads,
            1
        );
        assert!(receiver.blob_state(&session, &sha).unwrap().complete);
        if failure == "database" {
            fs::remove_dir(blocker).unwrap();
        }
        let result = receiver.commit(&session, &request).unwrap();
        assert_eq!(result.generation, 1);
        assert_eq!(fs::read(&original).unwrap(), content);
        receiver.stop_index_catch_up();
    }
}

/// 動かしたかたまりは失敗すると元へ戻り、確定は写さずに済ませる。写すと、本棚を
/// 閉じている間の時間と要る空き容量が、本棚の大きさに比例する。
#[test]
#[cfg(windows)]
fn committing_moves_blobs_into_place_and_a_failure_moves_them_back() {
    use std::os::windows::fs::FileTimesExt;
    use std::time::SystemTime;

    let hash = |bytes: &[u8]| {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let unique = b"one path holds this".as_slice();
    let shared = b"two paths hold this".as_slice();
    let files = [
        ("downloads/pixiv/9/unique.bin", unique),
        ("downloads/pixiv/9/shared-a.bin", shared),
        ("downloads/pixiv/9/shared-b.bin", shared),
    ];
    for fail_first in [true, false] {
        let root = TestRoot::new("moved_blobs");
        let (parts, paths) = standalone(&root.0);
        let db = Database::open(&paths.database(), &paths.storage()).unwrap();
        let incoming = root.0.join("snapshot.db");
        db.write_vacuumed_copy(&incoming).unwrap();
        super::snapshot::prepare_for_send(&incoming, &paths.library()).unwrap();
        drop(db);
        parts.db.open_existing().unwrap();
        let receiver = &parts.receiver;
        let session = receiver.open_session().unwrap().session_id;
        let database = fs::read(&incoming).unwrap();
        let manifest = files.map(|(path, bytes)| ManifestEntry {
            path: path.into(),
            size: bytes.len() as u64,
            mtime_ms: 1,
        });
        receiver.add_manifest(&session, &manifest).unwrap();
        for bytes in [unique, shared, database.as_slice()] {
            receiver
                .write_blob(&session, &hash(bytes), 0, bytes.len() as u64, bytes)
                .unwrap();
        }
        let binds = files.map(|(path, bytes)| BindEntry {
            path: path.into(),
            sha256: hash(bytes),
        });
        receiver.bind_files(&session, &binds).unwrap();
        let request = CommitRequest {
            database_sha256: hash(&database),
        };
        let library = paths.library();

        if fail_first {
            // DB の控えを作れない状態で確定する。ファイルを置いたあとで失敗する。
            let blocker = paths.database().with_extension("db.previous");
            fs::create_dir(&blocker).unwrap();
            assert!(receiver.commit(&session, &request).is_err());
            for (path, _) in files {
                assert!(!library.join(path).exists(), "置いたものを残さない: {path}");
            }
            for bytes in [unique, shared] {
                assert!(
                    receiver
                        .blob_state(&session, &hash(bytes))
                        .unwrap()
                        .complete
                );
            }
            fs::remove_dir(blocker).unwrap();
            assert_eq!(receiver.commit(&session, &request).unwrap().generation, 1);
        } else {
            // 作成日時は移動では変わらず、写すと変わる。かたまりの日時を古くしておく。
            // 失敗の回では測らない。同じ名前を置き直すと NTFS が前の日時を引き継ぐ。
            let long_ago = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
            fs::OpenOptions::new()
                .write(true)
                .open(walk_for(&root.0, &hash(unique)))
                .unwrap()
                .set_times(fs::FileTimes::new().set_created(long_ago))
                .unwrap();
            assert_eq!(receiver.commit(&session, &request).unwrap().generation, 1);
            let created = fs::metadata(library.join(files[0].0))
                .unwrap()
                .created()
                .unwrap();
            assert_eq!(created, long_ago, "かたまりを写さず動かす");
        }
        for (path, bytes) in files {
            assert_eq!(fs::read(library.join(path)).unwrap(), bytes);
        }
        receiver.stop_index_catch_up();
    }
}

fn walk_for(directory: &Path, name: &str) -> PathBuf {
    let mut pending = vec![directory.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap().flatten() {
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
            } else if entry.file_name().to_string_lossy() == name {
                return entry.path();
            }
        }
    }
    panic!("{name} が見つからない");
}

#[test]
fn requests_for_an_old_session_cannot_cross_the_commit_boundary() {
    let root = TestRoot::new("commit_race");
    let (parts, paths) = standalone(&root.0);
    drop(Database::open(&paths.database(), &paths.storage()).unwrap());
    parts.db.open_existing().unwrap();
    // 読んでいる要求を持ち、確定を「読み終わるのを待つ」ところで止める。
    let reader = parts.db.current().unwrap();
    let incoming = root.0.join("snapshot.db");
    reader.write_vacuumed_copy(&incoming).unwrap();
    super::snapshot::prepare_for_send(&incoming, &paths.library()).unwrap();
    let bytes = fs::read(incoming).unwrap();
    let sha = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let session = parts.receiver.open_session().unwrap().session_id;
    parts
        .receiver
        .write_blob(&session, &sha, 0, bytes.len() as u64, &bytes)
        .unwrap();
    let receiver = Arc::clone(&parts.receiver);
    let commit_session = session.clone();
    let commit_sha = sha.clone();
    let commit = std::thread::spawn(move || {
        receiver.commit(
            &commit_session,
            &CommitRequest {
                database_sha256: commit_sha,
            },
        )
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while parts.db.current().is_some() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let draining = parts.db.current().is_none();
    let barrier = Arc::new(std::sync::Barrier::new(5));
    let (sent, received) = std::sync::mpsc::channel();
    let writers: Vec<_> = (0..4)
        .map(|operation| {
            let receiver = Arc::clone(&parts.receiver);
            let barrier = Arc::clone(&barrier);
            let sent = sent.clone();
            let session = session.clone();
            let sha = sha.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let result = match operation {
                    0 => receiver.add_manifest(&session, &[]).map(|_| ()),
                    1 => receiver.blob_state(&session, &sha).map(|_| ()),
                    2 => receiver.write_blob(&session, &sha, 0, 0, &[]).map(|_| ()),
                    _ => receiver.bind_files(&session, &[]),
                };
                sent.send(()).unwrap();
                matches!(result, Err(SyncError::NotFound))
            })
        })
        .collect();
    barrier.wait();
    let premature = received.recv_timeout(Duration::from_millis(100)).is_ok();
    drop(reader);
    let committed = commit.join().unwrap();
    let refused: Vec<_> = writers
        .into_iter()
        .map(|writer| writer.join().unwrap())
        .collect();
    parts.receiver.stop_index_catch_up();
    assert!(draining, "読み手を待つところまで確定が進んだ");
    assert!(!premature, "確定の途中に古い送信を変更しない");
    committed.unwrap();
    assert!(
        refused.into_iter().all(|refused| refused),
        "確定後の古い送信は拒否する"
    );
}

#[test]
fn a_missing_library_database_is_requested_again() {
    let root = TestRoot::new("missing_database");
    let (parts, _paths) = standalone(&root.0);
    let incoming = root.0.join("snapshot.db");
    let conn = rusqlite::Connection::open(&incoming).unwrap();
    crate::database::schema::initialize(&conn).unwrap();
    drop(conn);
    let bytes = fs::read(&incoming).unwrap();
    let sha = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    // 本棚だけが失われ、前回の受信記録が残っている状態。
    parts
        .context
        .store
        .set_meta("database_sha256", &sha)
        .unwrap();
    let session = parts.receiver.open_session().unwrap();
    assert!(
        session.database_sha256.is_none(),
        "存在しないDBを受信済みと答えない"
    );
    parts
        .receiver
        .write_blob(&session.session_id, &sha, 0, bytes.len() as u64, &bytes)
        .unwrap();
    let result = parts
        .receiver
        .commit(
            &session.session_id,
            &CommitRequest {
                database_sha256: sha,
            },
        )
        .unwrap();
    assert!(!result.unchanged);
    assert!(parts.db.current().is_some());
    parts.receiver.stop_index_catch_up();
}

#[test]
fn opening_the_store_removes_the_recorded_login_name() {
    let root = TestRoot::created("last_user");
    let path = root.0.join("state.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE devices (
                 id TEXT PRIMARY KEY, name TEXT NOT NULL, role TEXT NOT NULL,
                 token_hash BLOB NOT NULL, created_at TEXT NOT NULL,
                 last_seen_at TEXT, last_user TEXT
             );
             INSERT INTO devices VALUES ('a', 'PC', 'admin', x'00', '2026-11-04T00:00:00Z', NULL, 'someone@example.test');",
        )
        .unwrap();
    }
    let store = ServerStore::open(&path).unwrap();
    assert_eq!(store.list_devices().unwrap()[0].name, "PC");
    drop(store);
    let columns: i64 = rusqlite::Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('devices') WHERE name = 'last_user'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(columns, 0);
}

// ---- 管理画面 ----

static TEST_ASSETS: &[AdminAsset] = &[
    AdminAsset {
        path: "index.html",
        mime: "text/html; charset=utf-8",
        bytes: b"<!doctype html><title>piep</title>",
    },
    AdminAsset {
        path: "assets/app.js",
        mime: "text/javascript; charset=utf-8",
        bytes: b"console.log(1)",
    },
];

const GOOD_PASSWORD: &str = "夏の終わりに海辺で読む本を探す";

struct AdminSite {
    _root: TestRoot,
    parts: StandaloneParts,
    paths: ServerPaths,
    url: String,
    client: reqwest::Client,
}

impl AdminSite {
    fn new(label: &str) -> Self {
        let root = TestRoot::new(label);
        let (parts, paths) = standalone(&root.0);
        parts.context.set_admin_web(AdminWeb::new(
            TEST_ASSETS,
            paths.clone(),
            ConnectionInfo {
                listen: "127.0.0.1:1".into(),
                admin_listen: Some("127.0.0.1:2".into()),
                public_url: None,
                local_admin: None,
                tls: "none",
            },
        ));
        let url = spawn_plain(admin_router(Arc::clone(&parts.context)));
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client");
        Self {
            _root: root,
            parts,
            paths,
            url,
            client,
        }
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{path}", self.url))
            .header("sec-fetch-site", "same-origin")
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.get(format!("{}{path}", self.url))
    }

    fn setup_ticket(&self) -> String {
        self.parts
            .context
            .store
            .issue_admin_ticket(
                crate::server::admin_store::TicketKind::Setup,
                Duration::from_secs(60),
            )
            .expect("ticket")
    }

    fn login_ticket(&self) -> String {
        self.parts
            .context
            .store
            .issue_admin_ticket(
                crate::server::admin_store::TicketKind::Login,
                Duration::from_secs(60),
            )
            .expect("ticket")
    }

    async fn set_up(&self) -> String {
        let response = self
            .post("/api/v1/admin/web/setup")
            .json(&serde_json::json!({
                "ticket": self.setup_ticket(),
                "username": "snow",
                "password": GOOD_PASSWORD,
            }))
            .send()
            .await
            .expect("setup");
        assert_eq!(response.status(), StatusCode::OK);
        cookie_of(&response)
    }

    async fn login(&self, username: &str, password: &str) -> reqwest::Response {
        self.post("/api/v1/admin/web/login")
            .json(&serde_json::json!({ "username": username, "password": password }))
            .send()
            .await
            .expect("login")
    }
}

/// `Set-Cookie` から、送り返す部分だけを取り出す。
fn cookie_of(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(header::SET_COOKIE)
        .expect("cookie")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn the_first_admin_needs_a_ticket_that_works_once() {
    let site = AdminSite::new("admin-first");
    let attempt = |ticket: String| {
        site.post("/api/v1/admin/web/setup")
            .json(&serde_json::json!({
                "ticket": ticket,
                "username": "snow",
                "password": GOOD_PASSWORD,
            }))
            .send()
    };
    let refused = attempt("でたらめ".into()).await.unwrap();
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    assert!(site.parts.context.store.admin_account().unwrap().is_none());

    // 期限切れ。
    let expired = site
        .parts
        .context
        .store
        .issue_admin_ticket(
            crate::server::admin_store::TicketKind::Setup,
            Duration::ZERO,
        )
        .unwrap();
    assert_eq!(
        attempt(expired).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );

    // ログインの券ではアカウントを作れない。
    assert_eq!(
        attempt(site.login_ticket()).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );

    // 決まりに合わないパスワードでは、券を使い切らない。
    let ticket = site.setup_ticket();
    let weak = site
        .post("/api/v1/admin/web/setup")
        .json(&serde_json::json!({
            "ticket": ticket,
            "username": "snow",
            "password": "aaaaaaaaaaaaaaaaaaaa",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(weak.status(), StatusCode::BAD_REQUEST);

    let first = attempt(ticket.clone()).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert!(site.parts.context.store.admin_account().unwrap().is_some());
    assert_eq!(
        attempt(ticket).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn the_session_cookie_has_the_designed_attributes_and_ends_at_logout() {
    let site = AdminSite::new("admin-cookie");
    let response = site
        .post("/api/v1/admin/web/setup")
        .json(&serde_json::json!({
            "ticket": site.setup_ticket(),
            "username": "snow",
            "password": GOOD_PASSWORD,
        }))
        .send()
        .await
        .unwrap();
    let raw = response
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(raw.starts_with("__Host-piep_admin="), "{raw}");
    for attribute in ["Secure", "HttpOnly", "SameSite=Strict", "Path=/"] {
        assert!(raw.contains(attribute), "{raw}");
    }
    let cookie = raw.split(';').next().unwrap().to_string();

    let status = site
        .get("/api/v1/admin/status")
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    assert_eq!(
        site.get("/api/v1/admin/status")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let logout = site
        .post("/api/v1/admin/web/logout")
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        logout.headers().get("clear-site-data").unwrap(),
        "\"cookies\""
    );
    let after = site
        .get("/api/v1/admin/status")
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(after.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_errors_look_the_same_and_the_wait_does_not_lock_out_tickets() {
    let site = AdminSite::new("admin-login");
    site.set_up().await;
    let wrong_password = site.login("snow", "違うパスワードをここに入れる").await;
    let wrong_name = site.login("someone", GOOD_PASSWORD).await;
    assert_eq!(wrong_password.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(wrong_name.status(), StatusCode::UNAUTHORIZED);
    let first: serde_json::Value = wrong_password.json().await.unwrap();
    let second: serde_json::Value = wrong_name.json().await.unwrap();
    assert_eq!(first, second);

    for _ in 0..3 {
        site.login("snow", "違うパスワードをここに入れる").await;
    }
    // 5 回目の失敗で待ちに入る。正しいパスワードでも、待っている間は通らない。
    let waiting = site.login("snow", GOOD_PASSWORD).await;
    assert_eq!(waiting.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(waiting.headers().contains_key(header::RETRY_AFTER));

    // 券は数えないので、持ち主は入れる。
    let ticket = site
        .post("/api/v1/admin/web/ticket")
        .json(&serde_json::json!({ "ticket": site.login_ticket() }))
        .send()
        .await
        .unwrap();
    assert_eq!(ticket.status(), StatusCode::OK);
}

#[tokio::test]
async fn requests_from_other_sites_and_unknown_hosts_are_refused() {
    let site = AdminSite::new("admin-csrf");
    let cookie = site.set_up().await;
    let url = format!("{}/api/v1/admin/web/logout", site.url);
    let send = |pairs: Vec<(&'static str, &'static str)>| {
        let mut request = site.client.post(&url).header(header::COOKIE, &cookie);
        for (name, value) in pairs {
            request = request.header(name, value);
        }
        request.send()
    };
    for pairs in [
        vec![("sec-fetch-site", "cross-site")],
        vec![("sec-fetch-site", "same-site")],
        vec![("origin", "null")],
        vec![("origin", "https://evil.example")],
        vec![],
    ] {
        let response = send(pairs.clone()).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{pairs:?}");
    }
    // 届いていないので、ログインは続いている。
    let status = site
        .get("/api/v1/admin/status")
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);

    // 知らない名前の Host は読むことも断る。券が通った名前は覚える。
    let unknown = site
        .get("/admin/")
        .header(header::HOST, "attacker.example")
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::FORBIDDEN);
    site.post("/api/v1/admin/web/ticket")
        .header(header::HOST, "piep.example.net")
        .header(header::ORIGIN, "https://piep.example.net")
        .json(&serde_json::json!({ "ticket": site.login_ticket() }))
        .send()
        .await
        .unwrap();
    // 上は Sec-Fetch-Site を付けている分だけ通り、名前は未知なので断られる。
    let known_before = site.parts.context.store.admin_hosts();
    assert!(known_before.is_empty());
    site.parts
        .context
        .store
        .allow_admin_host("piep.example.net")
        .unwrap();
    let known = site
        .get("/admin/")
        .header(header::HOST, "piep.example.net:443")
        .send()
        .await
        .unwrap();
    assert_eq!(known.status(), StatusCode::OK);

    // 鍵の要求は Cookie を使わないので、この守りを通らない。
    let token = pair_admin(
        &RemoteServer {
            url: format!("{}/", site.url),
            fingerprint: None,
        },
        &admin_secret(&site.paths),
        "test desktop",
    )
    .await
    .unwrap();
    let bearer = site
        .client
        .get(format!("{}/api/v1/admin/status", site.url))
        .bearer_auth(&token)
        .header(header::HOST, "attacker.example")
        .send()
        .await
        .unwrap();
    assert_eq!(bearer.status(), StatusCode::OK);
}

/// 管理画面のない口（公開の口）からは、リンクを出さずに開ける場所を教える。
#[tokio::test]
async fn a_link_is_not_issued_where_the_page_is_not_served() {
    let site = AdminSite::new("admin-link-elsewhere");
    site.set_up().await;
    let token = pair_admin(
        &RemoteServer {
            url: format!("{}/", site.url),
            fingerprint: None,
        },
        &admin_secret(&site.paths),
        "test desktop",
    )
    .await
    .unwrap();
    let public = spawn_plain(combined_router(Arc::clone(&site.parts.context)));
    let refused = site
        .client
        .post(format!("{public}/api/v1/admin/tickets"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let error: serde_json::Value = refused.json().await.unwrap();
    assert_eq!(error["code"], "admin_web_local");
    assert!(error["message"]
        .as_str()
        .unwrap()
        .contains("127.0.0.1:38778"));
    // 画面のある口では、今までどおり出す。
    let issued = site
        .client
        .post(format!("{}/api/v1/admin/tickets", site.url))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(issued.status(), StatusCode::OK);
}

/// デスクトップが接続先の名前でリンクを発行すると、その名前でページを開ける。
/// 127.0.0.1 は名前を問わないので、名前を付けて試す。
#[tokio::test]
async fn the_name_the_desktop_issued_a_link_for_opens_the_page() {
    let site = AdminSite::new("admin-host-learn");
    site.set_up().await;
    let token = pair_admin(
        &RemoteServer {
            url: format!("{}/", site.url),
            fingerprint: None,
        },
        &admin_secret(&site.paths),
        "test desktop",
    )
    .await
    .unwrap();
    let page = |host: &'static str| {
        site.get("/admin/")
            .header(header::HOST, host)
            .header("sec-fetch-dest", "document")
            .send()
    };
    // 覚える前は、ブラウザには理由と次の手を書いたページを返す。
    let before = page("piep.example.net").await.unwrap();
    assert_eq!(before.status(), StatusCode::FORBIDDEN);
    assert!(before.headers()[header::CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    let body = before.text().await.unwrap();
    assert!(body.contains("管理画面を開く"), "{body}");
    assert!(body.contains("data:image/svg+xml;base64,"), "{body}");
    assert!(body.contains("piep.example.net"), "{body}");
    // Host は相手が自由に書けるので、ページには文字として入れる。
    let hostile = site
        .get("/admin/")
        .header(header::HOST, "a<script>.example")
        .header("sec-fetch-dest", "document")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!hostile.contains("<script>"), "{hostile}");
    // 部品や API には、今までどおり JSON で答える。
    let api = site
        .get("/api/v1/admin/web/state")
        .header(header::HOST, "piep.example.net")
        .send()
        .await
        .unwrap();
    assert_eq!(api.status(), StatusCode::FORBIDDEN);
    let error: serde_json::Value = api.json().await.unwrap();
    assert_eq!(error["code"], "host_rejected");

    let issued = site
        .client
        .post(format!("{}/api/v1/admin/tickets", site.url))
        .bearer_auth(&token)
        .header(header::HOST, "piep.example.net")
        .send()
        .await
        .unwrap();
    assert_eq!(issued.status(), StatusCode::OK);
    let issued: serde_json::Value = issued.json().await.unwrap();
    let ticket = issued["path"]
        .as_str()
        .unwrap()
        .strip_prefix("/admin/#ticket=")
        .unwrap()
        .to_string();

    // 発行のあとは、同じ名前でページを開き、券で入れる。
    let after = page("piep.example.net").await.unwrap();
    assert_eq!(after.status(), StatusCode::OK);
    let entered = site
        .post("/api/v1/admin/web/ticket")
        .header(header::HOST, "piep.example.net")
        .json(&serde_json::json!({ "ticket": ticket }))
        .send()
        .await
        .unwrap();
    assert_eq!(entered.status(), StatusCode::OK);
    // 覚えるのは発行に使った名前だけで、ほかの名前は断ったまま。
    assert_eq!(
        page("other.example.net").await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn tickets_come_only_from_the_key_and_important_actions_ask_again() {
    let site = AdminSite::new("admin-ticket");
    let token = pair_admin(
        &RemoteServer {
            url: format!("{}/", site.url),
            fingerprint: None,
        },
        &admin_secret(&site.paths),
        "test desktop",
    )
    .await
    .unwrap();
    let issue = || {
        site.client
            .post(format!("{}/api/v1/admin/tickets", site.url))
            .bearer_auth(&token)
            .send()
    };
    // アカウントがなければ準備の券になる。
    let first: serde_json::Value = issue().await.unwrap().json().await.unwrap();
    assert_eq!(first["kind"], "setup");
    site.set_up().await;
    let second: serde_json::Value = issue().await.unwrap().json().await.unwrap();
    assert_eq!(second["kind"], "login");
    assert_eq!(second["expiresInSeconds"], 60);
    let ticket = second["path"]
        .as_str()
        .unwrap()
        .strip_prefix("/admin/#ticket=")
        .unwrap()
        .to_string();

    // 券で入る。券は 2 度使えない。
    let entered = site
        .post("/api/v1/admin/web/ticket")
        .json(&serde_json::json!({ "ticket": ticket }))
        .send()
        .await
        .unwrap();
    assert_eq!(entered.status(), StatusCode::OK);
    let cookie = cookie_of(&entered);
    let again = site
        .post("/api/v1/admin/web/ticket")
        .json(&serde_json::json!({ "ticket": ticket }))
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::UNAUTHORIZED);

    // ブラウザからは券を作れない。
    let by_cookie = site
        .post("/api/v1/admin/tickets")
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(by_cookie.status(), StatusCode::FORBIDDEN);

    // 券で入っただけでは、パスワードの変更を聞き直す。
    let change = |password: &'static str| {
        site.client
            .put(format!("{}/api/v1/admin/password", site.url))
            .header("sec-fetch-site", "same-origin")
            .header(header::COOKIE, &cookie)
            .json(&serde_json::json!({ "password": password }))
            .send()
    };
    let asked = change("秋の夕暮れに図書館で借りた本を読む").await.unwrap();
    assert_eq!(asked.status(), StatusCode::FORBIDDEN);
    let body: serde_json::Value = asked.json().await.unwrap();
    assert_eq!(body["code"], "reauth_required");
    let wrong = site
        .post("/api/v1/admin/web/reauth")
        .header(header::COOKIE, &cookie)
        .json(&serde_json::json!({ "password": "違うパスワードをここに入れる" }))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    let right = site
        .post("/api/v1/admin/web/reauth")
        .header(header::COOKIE, &cookie)
        .json(&serde_json::json!({ "password": GOOD_PASSWORD }))
        .send()
        .await
        .unwrap();
    assert_eq!(right.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        change("秋の夕暮れに図書館で借りた本を読む")
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        site.login("snow", GOOD_PASSWORD).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn pages_carry_the_security_headers_and_secrets_stay_out_of_the_record() {
    let site = AdminSite::new("admin-headers");
    let page = site.get("/admin/").send().await.unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let headers = page.headers();
    let csp = headers
        .get(header::CONTENT_SECURITY_POLICY)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(csp.contains("default-src 'none'"));
    assert!(csp.contains("frame-ancestors 'none'"));
    assert_eq!(headers.get(header::REFERRER_POLICY).unwrap(), "no-referrer");
    assert_eq!(
        headers.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
        "nosniff"
    );
    assert_eq!(
        headers.get(header::CACHE_CONTROL).unwrap(),
        "private, no-store"
    );
    assert!(!headers.contains_key(header::STRICT_TRANSPORT_SECURITY));

    let script = site.get("/admin/assets/app.js").send().await.unwrap();
    assert_eq!(script.status(), StatusCode::OK);
    assert_eq!(
        site.get("/admin/assets/none.js")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );

    let ticket = site.setup_ticket();
    let cookie = {
        let response = site
            .post("/api/v1/admin/web/setup")
            .json(&serde_json::json!({
                "ticket": ticket,
                "username": "snow",
                "password": GOOD_PASSWORD,
            }))
            .send()
            .await
            .unwrap();
        cookie_of(&response)
    };
    site.login("snow", "違うパスワードをここに入れる").await;
    let audit: serde_json::Value = site
        .get("/api/v1/admin/audit")
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let text = audit.to_string();
    assert!(text.contains("管理者の作成"));
    assert!(!text.contains(GOOD_PASSWORD));
    assert!(!text.contains("違うパスワード"));
    assert!(!text.contains(&ticket));
    assert!(!text.contains(cookie.split('=').nth(1).unwrap()));

    // パスワードはハッシュだけが残る。
    let account = site.parts.context.store.admin_account().unwrap().unwrap();
    assert!(account.password_hash.starts_with("$argon2id$"));
}

#[tokio::test]
async fn the_admin_screen_is_not_on_the_reader_port_or_a_public_only_server() {
    let site = AdminSite::new("admin-ports");
    let cookie = site.set_up().await;
    let reader = spawn_plain(reader_router(Arc::clone(&site.parts.context)));
    let client = &site.client;
    for path in [
        "/admin",
        "/admin/",
        "/api/v1/admin/status",
        "/api/v1/admin/web/state",
    ] {
        let response = client
            .get(format!("{reader}{path}"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }

    // 管理の口を分けず public_url だけのときは AdminWeb を渡さない。Cookie も見ない。
    let root = TestRoot::new("admin-public");
    let (parts, _paths) = standalone(&root.0);
    let public = spawn_plain(combined_router(Arc::clone(&parts.context)));
    let page = client.get(format!("{public}/admin/")).send().await.unwrap();
    assert_eq!(page.status(), StatusCode::NOT_FOUND);
    let by_cookie = client
        .get(format!("{public}/api/v1/admin/status"))
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(by_cookie.status(), StatusCode::UNAUTHORIZED);

    // 止めると、画面も Cookie も効かなくなる。
    site.parts
        .context
        .store
        .set_admin_web_disabled(true)
        .unwrap();
    assert_eq!(
        site.get("/admin/").send().await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    let stopped = site
        .get("/api/v1/admin/status")
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(stopped.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn about_is_for_registered_devices_and_hello_stays_silent_about_the_version() {
    let site = AdminSite::new("admin-about");
    let hello: serde_json::Value = site
        .get("/api/v1/hello")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(hello.get("version").is_none());
    assert_eq!(
        site.get("/api/v1/about").send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let token = pair_admin(
        &RemoteServer {
            url: format!("{}/", site.url),
            fingerprint: None,
        },
        &admin_secret(&site.paths),
        "test desktop",
    )
    .await
    .unwrap();
    let about: serde_json::Value = site
        .get("/api/v1/about")
        .bearer_auth(&token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(about["version"], super::update::VERSION);
    assert!(about["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability == "admin-web"));
}

#[tokio::test]
async fn text_is_compressed_only_for_a_client_that_asks() {
    let root = TestRoot::new("compression");
    let (parts, _paths) = standalone(&root.0);
    let url = spawn_plain(combined_router(Arc::clone(&parts.context)));
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("client");
    let plain = client
        .get(format!("{url}/api/v1/hello"))
        .send()
        .await
        .unwrap();
    assert_eq!(plain.status(), StatusCode::OK);
    assert!(plain.headers().get("content-encoding").is_none());
    let packed = client
        .get(format!("{url}/api/v1/hello"))
        .header("accept-encoding", "gzip")
        .send()
        .await
        .unwrap();
    assert_eq!(packed.status(), StatusCode::OK);
    assert_eq!(packed.headers()["content-encoding"], "gzip");
    // 圧縮しても、本文を保存させない指示は変わらない。
    assert_eq!(packed.headers()["cache-control"], "private, no-store");
}

async fn put_favorite(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    source_id: &str,
    favorite: bool,
    device_time: i64,
) -> reqwest::Response {
    client
        .put(format!("{url}/api/v1/favorites/pixiv/{source_id}"))
        .bearer_auth(token)
        .json(&serde_json::json!({ "favorite": favorite, "deviceTime": device_time }))
        .send()
        .await
        .expect("favorite put")
}

async fn favorite_on_server(client: &reqwest::Client, url: &str, token: &str) -> bool {
    let found = wait_for_search(client, url, token, "サーバー接続試験", 1).await;
    found["items"][0]["favorite"]
        .as_bool()
        .expect("favorite flag")
}

#[tokio::test]
async fn a_favorite_from_the_phone_reaches_the_desktop_and_survives_a_send() {
    let root = TestRoot::new("favorite-standalone");
    let fixture = fixture(&root.0);
    let (parts, paths) = standalone(&root.0);
    let url = spawn_plain(combined_router(Arc::clone(&parts.context)));
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("client");
    let remote = RemoteServer {
        url: format!("{url}/"),
        fingerprint: None,
    };
    let outgoing = root.0.join("outgoing");
    let token = pair_admin(&remote, &admin_secret(&paths), "test desktop")
        .await
        .expect("pair admin");
    send(
        &fixture.state,
        &fixture.app_data,
        &outgoing,
        &remote,
        &token,
    )
    .await;
    let pairing = admin_post(
        &remote,
        &token,
        "pairings",
        &serde_json::json!({ "url": "https://piep.example.test/" }),
    )
    .await
    .expect("reader pairing");
    let payload: serde_json::Value =
        serde_json::from_str(pairing["payload"].as_str().expect("payload")).expect("payload JSON");
    let credentials = pair_with(&client, &url, payload["secret"].as_str().expect("secret")).await;
    let reader = credentials["token"]
        .as_str()
        .expect("reader token")
        .to_string();

    let about: serde_json::Value = client
        .get(format!("{url}/api/v1/about"))
        .bearer_auth(&reader)
        .send()
        .await
        .expect("about")
        .json()
        .await
        .expect("about JSON");
    assert!(about["capabilities"]
        .as_array()
        .expect("capabilities")
        .contains(&serde_json::json!("favorite-write")));

    // 無い作品は記録しない。
    let missing = put_favorite(&client, &url, &reader, "no-such-work", true, 1_000).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    // 携帯は管理の口を読めない。
    let refused = client
        .get(format!("{url}/api/v1/admin/favorites?since=0"))
        .bearer_auth(&reader)
        .send()
        .await
        .expect("reader admin");
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);

    let stored = put_favorite(&client, &url, &reader, "server-test", true, 2_000).await;
    assert_eq!(stored.status(), StatusCode::OK);
    let stored: serde_json::Value = stored.json().await.expect("favorite JSON");
    assert_eq!(stored["favorite"], true);
    assert_eq!(stored["sourceId"], "server-test");
    // 古い時刻の変更は負け、今の値が返る。
    let stale: serde_json::Value =
        put_favorite(&client, &url, &reader, "server-test", false, 1_500)
            .await
            .json()
            .await
            .expect("stale JSON");
    assert_eq!(stale["favorite"], true);
    assert!(favorite_on_server(&client, &url, &reader).await);

    // デスクトップ版が取り込む前に別の変更を送っても、送られてきた本棚へ当て直す。
    let storage = fixture.app_data.join("downloads");
    add_work(
        &fixture.state.db,
        &storage,
        "server-test-2",
        "二つ目の作品",
        "追加の本文",
    );
    let replaced = send(
        &fixture.state,
        &fixture.app_data,
        &outgoing,
        &remote,
        &token,
    )
    .await;
    assert!(!replaced.unchanged);
    assert!(favorite_on_server(&client, &url, &reader).await);

    // デスクトップ版の取り込み。送信の前に本棚へ書き、取り込んだ番号を知らせる。
    let changes = super::sync_send::admin_get_if_present(&remote, &token, "favorites?since=0")
        .await
        .expect("favorites")
        .expect("this server has the favorites route");
    assert_eq!(changes["items"].as_array().expect("items").len(), 1);
    let cursor = changes["cursor"].as_i64().expect("cursor");
    fixture
        .state
        .db
        .set_favorite_by_source("pixiv", "server-test", true)
        .expect("apply")
        .expect("work exists");
    admin_post(
        &remote,
        &token,
        "favorites/ack",
        &serde_json::json!({ "cursor": cursor }),
    )
    .await
    .expect("ack");
    let after: serde_json::Value =
        super::sync_send::admin_get_if_present(&remote, &token, "favorites?since=0")
            .await
            .expect("favorites after ack")
            .expect("route");
    assert!(after["items"].as_array().expect("items").is_empty());

    // 取り込んだあとは、送った本棚がそのまま正しい。
    let third = send(
        &fixture.state,
        &fixture.app_data,
        &outgoing,
        &remote,
        &token,
    )
    .await;
    assert!(!third.unchanged);
    assert!(favorite_on_server(&client, &url, &reader).await);
    parts.receiver.stop_index_catch_up();
}

#[tokio::test]
async fn the_embedded_server_writes_a_favorite_to_the_desktop_library_and_says_so() {
    let root = TestRoot::new("favorite-embedded");
    let fixture = fixture(&root.0);
    let store = Arc::new(ServerStore::open(&root.0.join("state.db")).expect("store"));
    let context = super::http::ServerContext::new(
        super::library::Library::App(Arc::clone(&fixture.state)),
        Arc::clone(&store),
        fixture.app_data.clone(),
        None,
        None,
        None,
    );
    let told = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = Arc::clone(&told);
    context.set_favorites_listener(Arc::new(move || {
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    let url = spawn_plain(combined_router(context));
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("client");
    let secret = store
        .issue_pairing(Role::Reader, Duration::from_secs(60))
        .expect("pairing");
    let credentials = pair_with(&client, &url, &secret).await;
    let reader = credentials["token"].as_str().expect("token").to_string();

    let response = put_favorite(&client, &url, &reader, "server-test", true, 1_000).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        fixture
            .state
            .db
            .get_download(fixture.work_id)
            .expect("work")
            .favorite
    );
    assert_eq!(told.load(std::sync::atomic::Ordering::SeqCst), 1);
    // 内蔵サーバーは送信で入れ替わらないので、記録は残さない。
    assert!(store
        .favorite_changes_since(0, 10)
        .expect("changes")
        .is_empty());
    // 無い作品は 404 で、画面へも知らせない。
    let missing = put_favorite(&client, &url, &reader, "no-such-work", true, 2_000).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(told.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unauthorized_bodies_are_rejected_before_they_are_received() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let root = TestRoot::new("body_auth");
    let (parts, _) = standalone(&root.0);
    let secret = parts
        .context
        .store
        .issue_pairing(Role::Reader, Duration::from_secs(60))
        .unwrap();
    let reader = parts
        .context
        .store
        .consume_pairing(&secret, "reader")
        .unwrap();
    let url = spawn_plain(combined_router(Arc::clone(&parts.context)));
    for (method, path, token, expected) in [
        ("POST", "/api/v1/works/search", None, 401),
        ("PUT", "/api/v1/reading/pixiv/1", None, 401),
        ("POST", "/api/v1/admin/sync/sessions/s/manifest", None, 401),
        (
            "PUT",
            "/api/v1/admin/sync/sessions/s/blobs/b?offset=0&total=1",
            None,
            401,
        ),
        (
            "PUT",
            "/api/v1/admin/sync/sessions/s/blobs/b?offset=0&total=1",
            Some(reader.token.as_str()),
            403,
        ),
    ] {
        let mut socket = tokio::net::TcpStream::connect(url.trim_start_matches("http://"))
            .await
            .unwrap();
        let authorization = token.map_or(String::new(), |token| {
            format!("Authorization: Bearer {token}\r\n")
        });
        let request = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\n{authorization}Content-Type: application/json\r\nContent-Length: 8388608\r\n\r\n");
        socket.write_all(request.as_bytes()).await.unwrap();
        // 本文は送らない。認証が抽出後なら、タイムアウトまで応答できない。
        let mut response = [0_u8; 1024];
        let received = tokio::time::timeout(Duration::from_secs(3), socket.read(&mut response))
            .await
            .expect("bodyを待たず認証を拒否する")
            .unwrap();
        assert!(
            String::from_utf8_lossy(&response[..received])
                .starts_with(&format!("HTTP/1.1 {expected}")),
            "{path}: {}",
            String::from_utf8_lossy(&response[..received])
        );
    }
}

#[tokio::test]
async fn pairing_secrets_and_library_bodies_are_not_forwarded_to_redirects() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let forwarded = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&forwarded);
    let destination = spawn_plain(axum::Router::new().fallback(move || {
        let seen = Arc::clone(&seen);
        async move {
            seen.fetch_add(1, Ordering::SeqCst);
            axum::Json(serde_json::json!({ "role": "admin", "token": "forwarded" }))
        }
    }));
    for status in [
        StatusCode::TEMPORARY_REDIRECT,
        StatusCode::PERMANENT_REDIRECT,
    ] {
        let destination = destination.clone();
        let url = spawn_plain(axum::Router::new().fallback(move || {
            let destination = destination.clone();
            async move { (status, [(header::LOCATION, destination)]) }
        }));
        let remote = RemoteServer {
            url,
            fingerprint: None,
        };
        assert!(pair_admin(&remote, "one-time-secret", "desktop")
            .await
            .is_err());
        let client = super::sync_send::build_client(&remote).await.unwrap();
        let response = client
            .put(format!(
                "{}/api/v1/admin/sync/sessions/s/blobs/b",
                remote.url
            ))
            .bearer_auth("admin-token")
            .body("private library bytes")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
    }
    assert_eq!(forwarded.load(Ordering::SeqCst), 0);
}

#[test]
#[cfg(windows)]
fn failures_during_stale_removal_or_recording_restore_the_entire_commit() {
    use std::os::windows::fs::OpenOptionsExt;

    for failure in ["trash", "metadata"] {
        let root = TestRoot::new("late_commit_failure");
        let (parts, paths) = standalone(&root.0);
        let db = Database::open(&paths.database(), &paths.storage()).unwrap();
        let (_, original) = add_work(&db, &paths.storage(), "late", "元の作品", "元の本文");
        let previous = fs::read(&original).unwrap();
        let incoming = root.0.join("snapshot.db");
        db.write_vacuumed_copy(&incoming).unwrap();
        super::snapshot::prepare_for_send(&incoming, &paths.library()).unwrap();
        drop(db);
        parts.db.open_existing().unwrap();
        let receiver = &parts.receiver;
        let store = &parts.context.store;
        let stale_relative = "downloads/pixiv/late/stale.bin";
        let stale = paths.library().join(stale_relative);
        fs::write(&stale, b"keep until committed").unwrap();
        store
            .connection()
            .unwrap()
            .execute(
                "INSERT INTO files(path, size, mtime_ms) VALUES (?1, 20, 1)",
                [stale_relative],
            )
            .unwrap();
        let session = receiver.open_session().unwrap().session_id;
        let relative = original
            .strip_prefix(paths.library())
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let content = br#"{"text":"replacement"}"#;
        let database = fs::read(&incoming).unwrap();
        let hash = |bytes: &[u8]| {
            Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        };
        let sha = hash(content);
        let database_sha = hash(&database);
        receiver
            .add_manifest(
                &session,
                &[ManifestEntry {
                    path: relative.clone(),
                    size: content.len() as u64,
                    mtime_ms: 1,
                }],
            )
            .unwrap();
        for (sha, bytes) in [
            (&sha, content.as_slice()),
            (&database_sha, database.as_slice()),
        ] {
            receiver
                .write_blob(&session, sha, 0, bytes.len() as u64, bytes)
                .unwrap();
        }
        receiver
            .bind_files(
                &session,
                &[BindEntry {
                    path: relative,
                    sha256: sha.clone(),
                }],
            )
            .unwrap();
        let locked = if failure == "trash" {
            Some(
                fs::OpenOptions::new()
                    .read(true)
                    .share_mode(0)
                    .open(&stale)
                    .unwrap(),
            )
        } else {
            store.connection().unwrap().execute_batch(
                "CREATE TRIGGER refuse_file_record BEFORE INSERT ON files BEGIN SELECT RAISE(ABORT, 'test recording failure'); END;"
            ).unwrap();
            None
        };
        let request = CommitRequest {
            database_sha256: database_sha,
        };
        assert!(receiver.commit(&session, &request).is_err(), "{failure}");
        drop(locked);
        receiver.stop_index_catch_up();
        assert_eq!(fs::read(&original).unwrap(), previous, "{failure}");
        assert_eq!(
            fs::read(&stale).unwrap(),
            b"keep until committed",
            "{failure}"
        );
        assert_eq!(receiver.status().generation, 0, "{failure}");
        assert!(
            receiver.blob_state(&session, &sha).unwrap().complete,
            "{failure}"
        );
        assert_eq!(
            parts
                .db
                .current()
                .unwrap()
                .get_stats()
                .unwrap()
                .total_downloads,
            1
        );
        if failure == "metadata" {
            store
                .connection()
                .unwrap()
                .execute_batch("DROP TRIGGER refuse_file_record;")
                .unwrap();
        }
        let outcome = receiver.commit(&session, &request).unwrap();
        receiver.stop_index_catch_up();
        assert_eq!(outcome.generation, 1);
        assert_eq!(outcome.files_removed, 1);
        assert!(!stale.exists());
        assert_eq!(fs::read(&original).unwrap(), content);
    }
}

// 手動の AVD 検証用。通常のテストでは動かさず、本番の本棚には触れない。
// PIEP_ANDROID_QA_DIR に登録文字列を置く。stop ファイルで終了する。
#[tokio::test]
#[ignore]
async fn mobile_reader_emulator_fixture() {
    let output = PathBuf::from(std::env::var("PIEP_ANDROID_QA_DIR").expect("QA output directory"));
    fs::create_dir_all(&output).unwrap();
    let root = TestRoot::new("mobile-reader-qa");
    let fixture = fixture(&root.0);
    let _id = add_mobile_version_fixture(&fixture);
    let server = DesktopServer::new(&fixture.app_data).unwrap();
    server
        .start_embedded(
            Arc::clone(&fixture.state),
            EmbeddedSettings {
                ip: Some("127.0.0.1".into()),
                ..Default::default()
            },
            0,
        )
        .await
        .unwrap();
    let mut payloads = Vec::new();
    for _ in 0..2 {
        payloads.push(server.create_reader_pairing().await.unwrap().payload);
    }
    fs::write(
        output.join("pairings.json"),
        serde_json::to_vec(&payloads).unwrap(),
    )
    .unwrap();
    for _ in 0..2400 {
        if output.join("stop").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    server.stop_embedded().await.unwrap();
}

fn add_mobile_version_fixture(fixture: &Fixture) -> i64 {
    let storage = fixture.app_data.join("downloads");
    let (id, old) = add_work(
        &fixture.state.db,
        &storage,
        "990001",
        "読書操作の確認",
        "[chapter:第一版] 第一版の本文です。",
    );
    fixture
        .state
        .db
        .insert_version(&crate::database::NewVersion {
            download_id: id,
            version: 1,
            content_hash: Some("990001-content".into()),
            text_length: 20,
            json_path: old.to_string_lossy().into_owned(),
            original_json_path: Some(old.to_string_lossy().into_owned()),
            asset_count: 0,
            file_size_bytes: 100,
            created_at: "2026-11-06T00:00:00Z".into(),
            change_summary: None,
        })
        .unwrap();
    let next = storage.join("pixiv/990001/v2/original.json");
    fs::create_dir_all(next.parent().unwrap()).unwrap();
    let body = format!("[chapter:第二版] {}\n[[jumpuri:次の作品 > https://www.pixiv.net/novel/show.php?id=990002]]\n[[jumpuri:作者 > https://www.pixiv.net/users/12345]]", "静かな夜、窓の外を眺めながら本を読んだ。長押しで選択して検索できる。\n".repeat(100));
    fs::write(&next, serde_json::json!({"text": body}).to_string()).unwrap();
    let conn = rusqlite::Connection::open(fixture.app_data.join("piep.db")).unwrap();
    conn.execute("UPDATE downloads SET current_version=2, content_hash='qa-v2', json_path=?1, original_json_path=?1 WHERE id=?2", rusqlite::params![next.to_string_lossy(), id]).unwrap();
    fixture
        .state
        .db
        .insert_version(&crate::database::NewVersion {
            download_id: id,
            version: 2,
            content_hash: Some("qa-v2".into()),
            text_length: body.chars().count() as i64,
            json_path: next.to_string_lossy().into_owned(),
            original_json_path: Some(next.to_string_lossy().into_owned()),
            asset_count: 0,
            file_size_bytes: 100,
            created_at: "2026-11-07T00:00:00Z".into(),
            change_summary: None,
        })
        .unwrap();
    fixture.state.db.reindex_download(id).unwrap();
    add_work(
        &fixture.state.db,
        &storage,
        "990002",
        "リンク先の作品",
        "リンク先の本文です。",
    );
    fixture
        .state
        .db
        .upsert_download_person(id, "pixiv", "12345", "author", "リンク先作者")
        .unwrap();
    id
}

#[tokio::test]
async fn explicit_reader_versions_return_their_own_content_hash_and_outline() {
    let root = TestRoot::new("mobile-versions");
    let fixture = fixture(&root.0);
    let id = add_mobile_version_fixture(&fixture);
    let server = DesktopServer::new(&fixture.app_data).unwrap();
    server
        .start_embedded(
            Arc::clone(&fixture.state),
            EmbeddedSettings {
                ip: Some("127.0.0.1".into()),
                ..Default::default()
            },
            0,
        )
        .await
        .unwrap();
    let url = server
        .status()
        .await
        .embedded_url
        .unwrap()
        .trim_end_matches('/')
        .to_string();
    let client = reqwest::Client::builder()
        .tls_backend_rustls()
        .tls_danger_accept_invalid_certs(true)
        .no_proxy()
        .build()
        .unwrap();
    let pairing: serde_json::Value =
        serde_json::from_str(&server.create_reader_pairing().await.unwrap().payload).unwrap();
    let credentials = pair_with(&client, &url, pairing["secret"].as_str().unwrap()).await;
    let token = credentials["token"].as_str().unwrap();
    for (query, version, hash, heading) in [
        ("?version=1", 1, "990001-content", "第一版"),
        ("?version=2", 2, "qa-v2", "第二版"),
        ("", 2, "qa-v2", "第二版"),
    ] {
        let page: serde_json::Value = client
            .get(format!("{url}/api/v1/works/{id}/pages/0{query}"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(page["revision"]["currentVersion"], version);
        assert_eq!(page["revision"]["contentHash"], hash);
        assert!(page["revision"]["activeEditHash"].is_null());
        assert!(page["html"].as_str().unwrap().contains(heading));
        let outline: serde_json::Value = client
            .get(format!("{url}/api/v1/works/{id}/outline{query}"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(outline[0]["title"], heading);
    }
    server.stop_embedded().await.unwrap();
}
