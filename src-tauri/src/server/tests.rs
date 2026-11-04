//! サーバーモードを通しで確かめる。本物の HTTP で話し、本物のファイルと DB を動かす。

use super::desktop::{DesktopServer, EmbeddedSettings};
use super::http::{combined_router, reader_router};
use super::standalone::{assemble, ServerPaths, StandaloneParts};
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

    for path in [
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
    assert_eq!(metadata["people"].as_array().unwrap().len(), 2);
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
