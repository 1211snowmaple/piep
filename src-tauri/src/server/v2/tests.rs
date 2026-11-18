use super::*;
use crate::library::{Capture, CapturedMedia, EntityKind};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("piep-v2-http-{}", uuid::Uuid::now_v7())))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct TestAuthority {
    library: Id,
    subject: Id,
    active: AtomicBool,
}
impl Authority for TestAuthority {
    fn grant(&self, bearer: &str, library: Id) -> Option<Grant> {
        let device = match bearer {
            "fixture" => "phone",
            "second" => "tablet",
            _ => return None,
        };
        (library == self.library && self.active.load(Ordering::SeqCst)).then(|| Grant {
            device: device.into(),
            subject: self.subject,
        })
    }
}
struct Server {
    base: String,
    client: reqwest::Client,
    task: tokio::task::JoinHandle<()>,
    authority: Arc<TestAuthority>,
    library: Arc<Library>,
    context: Arc<Context>,
    temp: Temp,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    async fn start() -> Self {
        let temp = Temp::new();
        let library = Arc::new(Library::create(&temp.0).unwrap());
        let authority = Arc::new(TestAuthority {
            library: Id::new(library.id).unwrap(),
            subject: Id::new(library.id).unwrap(),
            active: AtomicBool::new(true),
        });
        let context = Arc::new(
            Context::new(
                library.clone(),
                authority.clone(),
                Id::new(uuid::Uuid::now_v7()).unwrap(),
            )
            .unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let application = router(context.clone());
        let task = tokio::spawn(async move { axum::serve(listener, application).await.unwrap() });
        Self {
            base: format!("http://{address}"),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            task,
            authority,
            library,
            context,
            temp,
        }
    }
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!("{}{path}", self.base))
            .bearer_auth("fixture")
    }
    fn resource(&self, entity: uuid::Uuid, snapshot: uuid::Uuid) -> String {
        format!(
            "/api/v2/libraries/{}/entities/{entity}/views/snapshot/{snapshot}/resources/media",
            self.library.id
        )
    }
}
fn capture(key: &str, bytes: Option<&[u8]>) -> Capture {
    Capture {
        request_id: uuid::Uuid::now_v7(),
        acquired_at: Some(123),
        provider: "pixiv".into(),
        external_key: key.into(),
        kind: EntityKind::Work,
        route: "offline-test".into(),
        raw: br#"{"text":"body"}"#.to_vec().into(),
        metadata: BTreeMap::from([("title".into(), json!(key))]),
        field_states: BTreeMap::new(),
        body: Some("body".into()),
        media: vec![CapturedMedia {
            slot: "cover".into(),
            role: "cover".into(),
            source_key: Some(format!("image:{key}")),
            source_url: None,
            quality: "original".into(),
            original_name: "cover.png".into(),
            mime_type: "image/png".into(),
            bytes: bytes.map(|b| b.to_vec().into()),
        }],
        attachments: vec![],
    }
}

#[tokio::test]
async fn one_device_cannot_take_every_reader_and_a_short_wait_comes_before_refusing() {
    let server = Server::start().await;
    let saved = save(&server.library, capture("one", Some(b"image")));
    let path = format!(
        "{}/manifest?slot=cover",
        server.resource(saved.entity_id, saved.snapshot_id)
    );
    let mut held = Vec::new();
    for _ in 0..READERS_PER_DEVICE {
        held.push(read_permit(&server.context, "phone").await.unwrap());
    }
    // 一台が自分の枠を使い切っても、ほかの端末は読める。
    let other = server
        .client
        .get(format!("{}{path}", server.base))
        .bearer_auth("second")
        .send()
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::OK);
    let refused = server.get(&path).send().await.unwrap();
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.headers()[header::RETRY_AFTER], "1");
    // 待っているあいだに空けば、断らずに読める。
    let waiting = tokio::spawn({
        let request = server.get(&path);
        async move { request.send().await.unwrap().status() }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    held.pop();
    assert_eq!(waiting.await.unwrap(), StatusCode::OK);
    drop(held);
    let response = server.get(&path).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["x-piep-subject-id"].to_str().unwrap(),
        server.authority.subject.get().to_string()
    );
    assert_eq!(
        response.headers()["x-piep-library-id"].to_str().unwrap(),
        server.library.id.to_string()
    );
    assert_eq!(
        response.headers()["x-piep-sync-epoch"].to_str().unwrap(),
        server.context.sync_epoch.get().to_string()
    );
}

#[tokio::test]
async fn raw_response_archives_are_not_exposed_by_the_reader_gateway() {
    let server = Server::start().await;
    let saved = save(&server.library, capture("one", Some(b"image")));
    let resources = server
        .library
        .resource_manifest(saved.entity_id, ResourceScope::Snapshot(saved.snapshot_id))
        .unwrap();
    let observation = resources
        .resources
        .iter()
        .find_map(|info| match info.reference {
            ResourceReference::Original { observation } => Some(observation),
            _ => None,
        })
        .unwrap();
    let path=format!("/api/v2/libraries/{}/entities/{}/views/snapshot/{}/resources/original/manifest?observation={observation}",server.library.id,saved.entity_id,saved.snapshot_id);
    assert_eq!(
        server.get(&path).send().await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}

fn save(library: &Library, input: Capture) -> crate::library::CaptureResult {
    let id = input.request_id;
    library
        .begin_acquisition(
            id,
            &input.provider,
            input.kind,
            &input.external_key,
            &input.route,
        )
        .unwrap();
    library.complete_acquisition(id, input).unwrap()
}

#[tokio::test]
async fn identity_requires_bearer_and_offers_the_reader_bundle_but_no_mutation() {
    let server = Server::start().await;
    for path in ["/api/v2/identity", "/api/v2/identity?token=fixture"] {
        assert_eq!(
            server
                .client
                .get(format!("{}{path}", server.base))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let response = server.get("/api/v2/identity").send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let identity = protocol::LibraryIdentity::parse(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(identity.library_id.get(), server.library.id);
    assert_eq!(identity.subject_id, server.authority.subject);
    assert!(identity.capabilities.reader_bundle && !identity.capabilities.mutations);
    assert_eq!(
        server
            .get("/api/v2/identity")
            .header(header::ORIGIN, "https://other.invalid")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn transfer_roundtrips_and_every_read_rechecks_authority_and_ownership() {
    let server = Server::start().await;
    let original = vec![b'x'; 30_000];
    let saved = save(&server.library, capture("one", Some(&original)));
    let foreign = save(&server.library, capture("two", None));
    let resource = server.resource(saved.entity_id, saved.snapshot_id);
    let response = server
        .get(&format!("{resource}/manifest?slot=cover"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(header::ETAG).is_none());
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "private, no-cache, no-transform"
    );
    let bytes = response.bytes().await.unwrap();
    let transfer = protocol::ValidatedTransfer::parse(&bytes).unwrap();
    let path = format!(
        "{resource}/files/{}/chunks/0?slot=cover",
        transfer.file().id
    );
    let response = server
        .get(&path)
        .header(header::IF_NONE_MATCH, "*")
        .send()
        .await
        .unwrap();
    // 取り直しの確認は持たない。中身のハッシュで取るので、同じ URL の中身は変わらない。
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(header::ETAG).is_none());
    assert!(response.headers()[header::CACHE_CONTROL]
        .to_str()
        .unwrap()
        .contains("immutable"));
    let encoded = response.bytes().await.unwrap();
    assert_eq!(
        piep_storage::decode_chunk(&encoded, &transfer.file().chunks[0]).unwrap(),
        original
    );
    let wrong = path.replace(&saved.entity_id.to_string(), &foreign.entity_id.to_string());
    assert_eq!(
        server.get(&wrong).send().await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    let wrong = path.replace(
        &transfer.file().id.to_string(),
        &crate::library::Fingerprint::of(b"other").to_string(),
    );
    assert_eq!(
        server.get(&wrong).send().await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    server.authority.active.store(false, Ordering::SeqCst);
    assert_eq!(
        server.get(&path).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn one_bundle_describes_every_readable_file_and_each_chunk_comes_back_through_it() {
    let server = Server::start().await;
    let original = vec![b'y'; 20_000];
    let saved = save(&server.library, capture("one", Some(&original)));
    let missing = save(&server.library, capture("two", None));
    let view = format!(
        "/api/v2/libraries/{}/entities/{}/views/snapshot/{}",
        server.library.id, saved.entity_id, saved.snapshot_id
    );
    let response = server.get(&format!("{view}/bundle")).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bundle = protocol::ReaderBundle::parse(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(bundle.entity_id.get(), saved.entity_id);
    assert_eq!(bundle.document["format"], "piep-reader-v1");
    // 本文と表紙。原応答は入らない。
    let kinds: Vec<_> = bundle
        .files
        .iter()
        .map(|item| match &item.resource {
            protocol::ResourceReference::Body => "body".to_owned(),
            protocol::ResourceReference::Media { slot } => format!("media:{slot}"),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(kinds, vec!["body", "media:cover"]);
    for item in &bundle.files {
        let (kind, query) = match &item.resource {
            protocol::ResourceReference::Body => ("body", String::new()),
            protocol::ResourceReference::Media { slot } => ("media", format!("?slot={slot}")),
            _ => unreachable!(),
        };
        let native = item.file.to_native();
        let mut restored = Vec::new();
        for (ordinal, chunk) in native.chunks.iter().enumerate() {
            let url = format!(
                "{view}/resources/{kind}/files/{}/chunks/{ordinal}{query}",
                item.file.id.get()
            );
            let encoded = server
                .get(&url)
                .send()
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap();
            restored.extend(piep_storage::decode_chunk(&encoded, chunk).unwrap());
        }
        assert_eq!(crate::library::Fingerprint::of(&restored), native.id);
        if kind == "media" {
            assert_eq!(restored, original);
        }
    }
    // 画像が未保存の作品では、表紙の file は載らない（取りに行かせない）。
    let response = server
        .get(&format!(
            "/api/v2/libraries/{}/entities/{}/views/snapshot/{}/bundle",
            server.library.id, missing.entity_id, missing.snapshot_id
        ))
        .send()
        .await
        .unwrap();
    let bundle = protocol::ReaderBundle::parse(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(bundle.files.len(), 1);
    // ほかの作品の保存版・ほかの棚・知らない view・余計な問い合わせは断る。
    for (path, status) in [
        (
            format!(
                "/api/v2/libraries/{}/entities/{}/views/snapshot/{}/bundle",
                server.library.id, missing.entity_id, saved.snapshot_id
            ),
            StatusCode::NOT_FOUND,
        ),
        (
            view.replace(
                &server.library.id.to_string(),
                &uuid::Uuid::now_v7().to_string(),
            ) + "/bundle",
            StatusCode::NOT_FOUND,
        ),
        (
            view.replace("views/snapshot/", "views/current/") + "/bundle",
            StatusCode::BAD_REQUEST,
        ),
        (format!("{view}/bundle?slot=cover"), StatusCode::BAD_REQUEST),
    ] {
        assert_eq!(
            server.get(&path).send().await.unwrap().status(),
            status,
            "{path}"
        );
    }
}

#[tokio::test]
async fn missing_resources_invalid_parameters_and_wrong_libraries_do_not_fall_back() {
    let server = Server::start().await;
    let saved = save(&server.library, capture("one", None));
    let path = format!(
        "{}/manifest?slot=cover",
        server.resource(saved.entity_id, saved.snapshot_id)
    );
    assert_eq!(
        server.get(&path).send().await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    let invalid = path.replace("views/snapshot/", "views/current/");
    assert_eq!(
        server.get(&invalid).send().await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    let wrong = path.replace(
        &server.library.id.to_string(),
        &uuid::Uuid::now_v7().to_string(),
    );
    assert_eq!(
        server.get(&wrong).send().await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        server
            .get(&(path + "&unexpected=1"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn provider_keys_resolve_to_the_current_reading_view_only_while_on_the_shelf() {
    let server = Server::start().await;
    let saved = save(&server.library, capture("work+one", None));
    let route = format!("/api/v2/libraries/{}/works/lookup", server.library.id);
    let url = format!("{route}?provider=pixiv&key=work%2Bone");
    let response = server
        .get(&url)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(response.headers()[header::VARY], "Authorization");
    let view: protocol::WorkReadingView = response.json().await.unwrap();
    assert_eq!(view.entity_id.get(), saved.entity_id);
    assert_eq!(view.snapshot_id.get(), saved.snapshot_id);
    assert_eq!(view.edit_id, None);
    let bundle_url = format!(
        "/api/v2/libraries/{}/entities/{}/views/snapshot/{}/bundle",
        server.library.id,
        view.entity_id.get(),
        view.snapshot_id.get(),
    );
    let bundle = server
        .get(&bundle_url)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(
        protocol::ReaderBundle::parse(&bundle.bytes().await.unwrap())
            .unwrap()
            .entity_id,
        view.entity_id,
    );

    let edit = server
        .library
        .save_edit(crate::library::EditRequest {
            request_id: uuid::Uuid::now_v7(),
            work: saved.entity_id,
            user: server.library.id,
            base: saved.snapshot_id,
            parent: None,
            expected: 0,
            content: &crate::library::EditContent {
                format: "piep-edit-v1".into(),
                title: None,
                blocks: vec![],
                media: vec![],
                provenance: None,
            },
            activate: true,
        })
        .unwrap();
    let reflected: protocol::WorkReadingView = server
        .get(&url)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reflected.edit_id, Some(Id::new(edit).unwrap()));
    assert_eq!(reflected.snapshot_id, view.snapshot_id);
    let edit_url = format!(
        "/api/v2/libraries/{}/entities/{}/views/edit/{edit}/bundle",
        server.library.id,
        reflected.entity_id.get(),
    );
    let response = server
        .get(&edit_url)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let edit_bundle = protocol::ReaderBundle::parse(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(
        edit_bundle.scope,
        protocol::ResourceScope::Edit(Id::new(edit).unwrap())
    );

    for (path, status) in [
        (url.replace("pixiv", "fanbox"), StatusCode::NOT_FOUND),
        (
            url.replace(
                &server.library.id.to_string(),
                &uuid::Uuid::now_v7().to_string(),
            ),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("{route}?provider=pixiv&key="),
            StatusCode::BAD_REQUEST,
        ),
        (format!("{url}&key=other"), StatusCode::BAD_REQUEST),
        (format!("{url}&token=fixture"), StatusCode::BAD_REQUEST),
    ] {
        assert_eq!(
            server.get(&path).send().await.unwrap().status(),
            status,
            "{path}"
        );
    }
    server
        .library
        .remove_work(uuid::Uuid::now_v7(), saved.entity_id, 1)
        .unwrap();
    assert_eq!(
        server.get(&url).send().await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    server.authority.active.store(false, Ordering::SeqCst);
    assert_eq!(
        server.get(&url).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn corrupt_stored_bytes_are_not_sent_and_do_not_expose_paths() {
    let server = Server::start().await;
    let saved = save(&server.library, capture("one", Some(b"image")));
    let manifest = server
        .library
        .resource_transfer_manifest(
            saved.entity_id,
            ResourceScope::Snapshot(saved.snapshot_id),
            &ResourceReference::Media {
                slot: "cover".into(),
            },
        )
        .unwrap();
    let hash = manifest.file.chunks[0].id.get().to_string();
    let path = server
        .temp
        .0
        .join("objects/v1")
        .join(&hash[..2])
        .join(format!("{hash}.obj"));
    fs::write(path, b"corrupt").unwrap();
    let url = format!(
        "{}/files/{}/chunks/0?slot=cover",
        server.resource(saved.entity_id, saved.snapshot_id),
        manifest.file.id.get()
    );
    let response = server.get(&url).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let text = response.text().await.unwrap();
    assert!(
        !text.contains("objects") && !text.contains(&server.temp.0.to_string_lossy().to_string())
    );
}
