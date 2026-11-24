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
    other_subject: Id,
    active: AtomicBool,
}
impl Authority for TestAuthority {
    fn grant(&self, bearer: &str, library: Id) -> Option<Grant> {
        let (device, subject) = match bearer {
            "fixture" => ("phone", self.subject),
            "second" => ("tablet", self.other_subject),
            _ => return None,
        };
        (library == self.library && self.active.load(Ordering::SeqCst)).then(|| Grant {
            device: device.into(),
            subject,
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
            other_subject: Id::new(uuid::Uuid::now_v7()).unwrap(),
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
        self.get_as(path, "fixture")
    }
    fn get_as(&self, path: &str, token: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!("{}{path}", self.base))
            .bearer_auth(token)
    }
    fn resource(&self, entity: uuid::Uuid, snapshot: uuid::Uuid) -> String {
        format!(
            "/api/libraries/{}/entities/{entity}/views/snapshot/{snapshot}/resources/media",
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
    let path=format!("/api/libraries/{}/entities/{}/views/snapshot/{}/resources/original/manifest?observation={observation}",server.library.id,saved.entity_id,saved.snapshot_id);
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
async fn identity_requires_bearer_and_offers_the_reader_bundle_and_mutations() {
    let server = Server::start().await;
    for path in ["/api/identity", "/api/identity?token=fixture"] {
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
    let response = server.get("/api/identity").send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let identity = protocol::LibraryIdentity::parse(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(identity.library_id.get(), server.library.id);
    assert_eq!(identity.subject_id, server.authority.subject);
    assert!(identity.capabilities.reader_bundle && identity.capabilities.mutations);
    assert_eq!(
        server
            .get("/api/identity")
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
        "/api/libraries/{}/entities/{}/views/snapshot/{}",
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
        let file = item.file.to_content_file();
        let mut restored = Vec::new();
        for (ordinal, chunk) in file.chunks.iter().enumerate() {
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
        assert_eq!(crate::library::Fingerprint::of(&restored), file.id);
        if kind == "media" {
            assert_eq!(restored, original);
        }
    }
    // 画像が未保存の作品では、表紙の file は載らない（取りに行かせない）。
    let response = server
        .get(&format!(
            "/api/libraries/{}/entities/{}/views/snapshot/{}/bundle",
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
                "/api/libraries/{}/entities/{}/views/snapshot/{}/bundle",
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
async fn removing_a_work_hides_its_saved_view_without_hiding_shared_chunks() {
    let server = Server::start().await;
    let cover = png();
    let mut input = capture("removed", Some(&cover));
    input.attachments.push(crate::library::CapturedAttachment {
        slot: "appendix".into(),
        original_name: "appendix.txt".into(),
        mime_type: Some("text/plain".into()),
        bytes: b"appendix".to_vec().into(),
    });
    let removed = save(&server.library, input);
    let kept = save(&server.library, capture("kept", Some(&cover)));
    let view = |saved: &crate::library::CaptureResult| {
        server.shelf(&format!(
            "/entities/{}/views/snapshot/{}",
            saved.entity_id, saved.snapshot_id
        ))
    };
    let removed_view = view(&removed);
    let mut paths = vec![
        format!("{removed_view}/bundle"),
        server.shelf(&format!("/works/{}/cover", removed.entity_id)),
    ];
    let mut descriptors = Vec::new();
    for (kind, query) in [
        ("body", ""),
        ("media", "?slot=cover"),
        ("file", "?slot=appendix"),
    ] {
        let resource = format!("{removed_view}/resources/{kind}");
        let manifest = format!("{resource}/manifest{query}");
        let response = server.get(&manifest).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{manifest}");
        let transfer =
            protocol::ValidatedTransfer::parse(&response.bytes().await.unwrap()).unwrap();
        let chunk = format!("{resource}/files/{}/chunks/0{query}", transfer.file().id);
        paths.extend([manifest, chunk]);
        descriptors.push(transfer);
    }

    let kept_resource = format!("{}/resources/media", view(&kept));
    let response = server
        .get(&format!("{kept_resource}/manifest?slot=cover"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let shared = protocol::ValidatedTransfer::parse(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(shared.file().id, descriptors[1].file().id);
    assert_eq!(
        shared.file().chunks[0].id,
        descriptors[1].file().chunks[0].id
    );
    let kept_chunk = format!(
        "{kept_resource}/files/{}/chunks/0?slot=cover",
        shared.file().id
    );

    for path in paths.iter().chain(std::iter::once(&kept_chunk)) {
        assert_eq!(
            server.get(path).send().await.unwrap().status(),
            StatusCode::OK,
            "{path}"
        );
    }
    server
        .library
        .remove_work(uuid::Uuid::now_v7(), removed.entity_id, 1)
        .unwrap();

    let response = server.get(&kept_chunk).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        piep_storage::decode_chunk(&response.bytes().await.unwrap(), &shared.file().chunks[0])
            .unwrap(),
        cover
    );
    let mut unexpected = Vec::new();
    for path in paths {
        let status = server.get(&path).send().await.unwrap().status();
        if status != StatusCode::NOT_FOUND {
            unexpected.push((path, status));
        }
    }
    assert!(
        unexpected.is_empty(),
        "棚から外した作品の応答が 404 ではない: {unexpected:?}"
    );
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
    let route = format!("/api/libraries/{}/works/lookup", server.library.id);
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
        "/api/libraries/{}/entities/{}/views/snapshot/{}/bundle",
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
        "/api/libraries/{}/entities/{}/views/edit/{edit}/bundle",
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
async fn the_works_list_pages_tied_dates_and_keeps_cursors_with_the_reader_and_epoch() {
    let server = Server::start().await;
    let north = save(&server.library, capture("north", None));
    let south = save(&server.library, capture("south", None));
    let removed = save(&server.library, capture("removed", None));
    server
        .library
        .remove_work(uuid::Uuid::now_v7(), removed.entity_id, 1)
        .unwrap();
    let route = format!("/api/libraries/{}/works", server.library.id);
    let first = format!("{route}?limit=1");
    let response = server
        .get(&first)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    assert_eq!(response.headers()[header::VARY], "Authorization");
    assert_eq!(
        response.headers()["x-piep-sync-epoch"],
        server.context.sync_epoch.get().to_string()
    );
    let page: protocol::ReaderWorkPage = response.json().await.unwrap();
    assert_eq!(page.items.len(), 1);
    let item = &page.items[0];
    assert_eq!(
        item.view.entity_id.get(),
        north.entity_id.max(south.entity_id)
    );
    assert_eq!(item.provider, "pixiv");
    assert_eq!(item.title, item.key);
    let cursor = page.next.unwrap();
    let second = format!("{first}&after={cursor}");
    let last: protocol::ReaderWorkPage = server
        .get(&second)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(last.items.len(), 1);
    assert_eq!(
        last.items[0].view.entity_id.get(),
        north.entity_id.min(south.entity_id)
    );
    assert!(last.next.is_none());
    let stale = second.replace(
        &server.context.sync_epoch.get().to_string(),
        &uuid::Uuid::now_v7().to_string(),
    );
    assert_eq!(
        server.get(&stale).send().await.unwrap().status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        server
            .get_as(&second, "second")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    for path in [
        format!("{route}?limit=0"),
        format!("{route}?limit=51"),
        format!("{first}&after=broken"),
    ] {
        assert_eq!(
            server.get(&path).send().await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
    server.authority.active.store(false, Ordering::SeqCst);
    assert_eq!(
        server.get(&route).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

fn png() -> Vec<u8> {
    png_with_size(8, 12)
}

fn png_with_size(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::RgbImage::from_pixel(width, height, image::Rgb([200, 40, 40]))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

impl Server {
    fn search(&self, body: serde_json::Value) -> reqwest::RequestBuilder {
        self.search_as(body, "fixture")
    }
    fn search_as(&self, body: serde_json::Value, token: &str) -> reqwest::RequestBuilder {
        self.client
            .post(format!(
                "{}/api/libraries/{}/works/search",
                self.base, self.library.id
            ))
            .bearer_auth(token)
            .json(&body)
    }
}

async fn shelf_page(response: reqwest::Response) -> piep_mobile_api::MobileWorkPage {
    response.error_for_status().unwrap().json().await.unwrap()
}

#[tokio::test]
async fn work_search_rejects_invalid_page_sizes_and_false_favorite_filter() {
    let server = Server::start().await;
    for request in [
        json!({"limit": 500}),
        json!({"limit": 0}),
        json!({"favorite": false}),
    ] {
        let response = server.search(request.clone()).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{request}");
    }
}

#[tokio::test]
async fn unfinished_search_index_returns_a_code() {
    let server = Server::start().await;
    server.context.search_ready.store(false, Ordering::Release);
    let response = server.search(json!({"text": "海"})).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["code"], "search_indexing");
}

#[tokio::test]
async fn the_phone_list_sorts_filters_and_pages_the_shelf() {
    let server = Server::start().await;
    let north = save(&server.library, capture("north", None));
    let south = save(&server.library, capture("south", None));
    let removed = save(&server.library, capture("removed", None));
    server
        .library
        .remove_work(uuid::Uuid::now_v7(), removed.entity_id, 1)
        .unwrap();

    // 題の順に一件ずつ。作品は棚の UUID で返る。
    let first = shelf_page(
        server
            .search(json!({"text": "", "limit": 1, "sortBy": "title", "sortOrder": "asc"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(first.total_estimate, Some(2));
    assert_eq!(first.items.len(), 1);
    let item = &first.items[0];
    assert_eq!(item.id, north.entity_id.to_string());
    assert_eq!(item.snapshot_id, north.snapshot_id.to_string());
    assert_eq!(
        (item.source.as_str(), item.source_id.as_str()),
        ("pixiv", "north")
    );
    assert!(!item.has_cover && !item.edited);
    let cursor = first.next_cursor.clone().unwrap();
    let second = shelf_page(
        server
            .search(json!({"text": "", "limit": 1, "sortBy": "title", "sortOrder": "asc", "cursor": cursor}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(second.items[0].id, south.entity_id.to_string());
    assert!(second.next_cursor.is_none());
    // 続きは数え直さない。
    assert_eq!(second.total_estimate, None);

    // 続きの位置は、同じ棚の同じ利用者だけが使える。
    let stale = cursor.replace(
        &server.context.sync_epoch.get().to_string(),
        &uuid::Uuid::now_v7().to_string(),
    );
    for (body, token) in [
        (
            json!({"limit": 1, "sortBy": "title", "sortOrder": "asc", "cursor": stale}),
            "fixture",
        ),
        (
            json!({"limit": 1, "sortBy": "title", "sortOrder": "asc", "cursor": cursor}),
            "second",
        ),
    ] {
        assert_eq!(
            server.search_as(body, token).send().await.unwrap().status(),
            StatusCode::CONFLICT
        );
    }

    // ページ番号で開いたページには続きの位置がない。
    let numbered = shelf_page(
        server
            .search(json!({"offset": 1, "limit": 1, "sortBy": "title", "sortOrder": "asc"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(numbered.items[0].id, south.entity_id.to_string());
    assert!(numbered.next_cursor.is_none());

    // 絞り込みは棚の一覧の意味で効く。
    let none = shelf_page(
        server
            .search(json!({"source": "fanbox"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert!(none.items.is_empty());
    let unknown_person = shelf_page(
        server
            .search(json!({"personSource": "pixiv", "personKey": "nobody"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert!(unknown_person.items.is_empty());

    // 誤った条件は断る（黙って外さない）。
    for body in [
        json!({"sortBy": "relevance"}),
        json!({"source": "elsewhere"}),
        json!({"unknown": true}),
        json!({"cursor": "broken"}),
        json!({"offset": 1, "cursor": "x"}),
    ] {
        assert_eq!(
            server.search(body).send().await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }

    // 語の検索は索引が棚に追いつくまで答えない。追いついたら題で引ける。
    assert_eq!(
        server
            .search(json!({"text": "north"}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    server.library.sync_search_index(&mut |_, _| {}).unwrap();
    server.context.search_ready.store(true, Ordering::SeqCst);
    let found = shelf_page(
        server
            .search(json!({"text": "north"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        found
            .items
            .iter()
            .map(|item| item.id.clone())
            .collect::<Vec<_>>(),
        vec![north.entity_id.to_string()]
    );

    server.authority.active.store(false, Ordering::SeqCst);
    assert_eq!(
        server.search(json!({})).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn the_work_screen_and_cover_come_from_the_current_version_of_a_shelved_work() {
    let server = Server::start().await;
    let covered = save(&server.library, capture("covered", Some(&png())));
    let bare = save(&server.library, capture("bare", None));
    let work = |id: uuid::Uuid| format!("/api/libraries/{}/works/{id}", server.library.id);

    let detail: piep_mobile_api::MobileWorkDetail = server
        .get(&work(covered.entity_id))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(detail.work.id, covered.entity_id.to_string());
    assert!(detail.work.has_cover);
    assert_eq!(detail.versions.len(), 1);
    assert_eq!(detail.versions[0].version, 1);
    // 以前の版を読むときに引く、版ごとの写しの ID。
    assert_eq!(
        detail.versions[0].snapshot_id,
        covered.snapshot_id.to_string()
    );
    assert_eq!(detail.edit_base_version, None);

    for size in ["", "?size=card", "?size=large"] {
        let response = server
            .get(&format!("{}/cover{size}", work(covered.entity_id)))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/webp");
        assert_eq!(response.headers()[header::VARY], "Authorization");
        let bytes = response.bytes().await.unwrap();
        assert_eq!(
            image::load_from_memory(&bytes)
                .unwrap()
                .into_rgb8()
                .dimensions(),
            (8, 12)
        );
    }
    for (path, status) in [
        (
            format!("{}/cover", work(bare.entity_id)),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("{}/cover", work(uuid::Uuid::now_v7())),
            StatusCode::NOT_FOUND,
        ),
        (work(uuid::Uuid::now_v7()), StatusCode::NOT_FOUND),
        (
            format!("{}/cover?size=huge", work(covered.entity_id)),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("{}?extra=1", work(covered.entity_id)),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!(
                "/api/libraries/{}/works/{}",
                uuid::Uuid::now_v7(),
                covered.entity_id
            ),
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(
            server.get(&path).send().await.unwrap().status(),
            status,
            "{path}"
        );
    }

    // 棚から外した作品は、作品の画面も表紙も出さない。
    server
        .library
        .remove_work(uuid::Uuid::now_v7(), covered.entity_id, 1)
        .unwrap();
    for path in [
        work(covered.entity_id),
        format!("{}/cover", work(covered.entity_id)),
    ] {
        assert_eq!(
            server.get(&path).send().await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn work_cover_comes_from_the_requested_snapshot() {
    let server = Server::start().await;
    let old = save(&server.library, capture("covered", Some(&png())));
    let mut update = capture("covered", Some(&png_with_size(12, 8)));
    update.acquired_at = Some(124);
    update.body = Some("changed body".into());
    update.media[0].source_key = Some("updated-cover".into());
    let current = save(&server.library, update);
    assert_eq!(current.entity_id, old.entity_id);
    assert!(current.created_revision);
    let cover = server.shelf(&format!("/works/{}/cover", old.entity_id));

    for (query, dimensions) in [
        (String::new(), (12, 8)),
        (format!("?snapshot={}", old.snapshot_id), (8, 12)),
        (format!("?snapshot={}&size=large", old.snapshot_id), (8, 12)),
    ] {
        let response = server.get(&format!("{cover}{query}")).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{query}");
        let bytes = response.bytes().await.unwrap();
        assert_eq!(
            image::load_from_memory(&bytes)
                .unwrap()
                .into_rgb8()
                .dimensions(),
            dimensions,
            "{query}"
        );
    }

    server
        .library
        .remove_work(uuid::Uuid::now_v7(), old.entity_id, 1)
        .unwrap();
    let response = server
        .get(&format!("{cover}?snapshot={}", old.snapshot_id))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn work_cover_rejects_a_snapshot_from_another_work() {
    let server = Server::start().await;
    let work = save(&server.library, capture("covered", Some(&png())));
    let other = save(&server.library, capture("other", Some(&png())));
    let response = server
        .get(&server.shelf(&format!(
            "/works/{}/cover?snapshot={}",
            work.entity_id, other.snapshot_id
        )))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

impl Server {
    fn favorite_as(
        &self,
        work: uuid::Uuid,
        body: serde_json::Value,
        token: &str,
    ) -> reqwest::RequestBuilder {
        self.with_write_scope(self.favorite_request(work, body, token), token)
    }

    fn favorite_request(
        &self,
        work: uuid::Uuid,
        body: serde_json::Value,
        token: &str,
    ) -> reqwest::RequestBuilder {
        self.client
            .put(format!(
                "{}/api/libraries/{}/works/{work}/favorite",
                self.base, self.library.id
            ))
            .bearer_auth(token)
            .json(&body)
    }
}

#[tokio::test]
async fn favorite_write_from_another_scope_cannot_change_the_shelf() {
    let server = Server::start().await;
    let work = save(&server.library, capture("stale-favorite", None));
    let body = json!({"requestId": uuid::Uuid::now_v7(), "expected": "0", "favorite": true});
    let subject = server.authority.subject.get().to_string();
    let epoch = server.context.sync_epoch.get().to_string();
    let previous_epoch = uuid::Uuid::now_v7().to_string();
    let other_subject = server.authority.other_subject.get().to_string();

    // 同じ棚に作品があり、お気に入りの更新世代も合う要求を送る。
    for (name, subjects, epochs) in [
        ("古い同期世代", vec![&subject], vec![&previous_epoch]),
        ("別の利用者", vec![&other_subject], vec![&epoch]),
        ("同期世代なし", vec![&subject], vec![]),
        ("利用者なし", vec![], vec![&epoch]),
        ("同期世代の重複", vec![&subject], vec![&epoch, &epoch]),
        ("利用者の重複", vec![&subject, &subject], vec![&epoch]),
    ] {
        let mut request = server.favorite_request(work.entity_id, body.clone(), "fixture");
        for value in subjects {
            request = request.header("x-piep-subject-id", value.as_str());
        }
        for value in epochs {
            request = request.header("x-piep-sync-epoch", value.as_str());
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.headers()["x-piep-sync-epoch"], epoch.as_str());
        let status = response.status();
        let payload: serde_json::Value = response.json().await.unwrap();
        let state = server
            .library
            .user_state(work.entity_id, server.authority.subject.get())
            .unwrap();
        assert_eq!(
            (status, payload, state.favorite, state.generation),
            (
                StatusCode::CONFLICT,
                json!({"code": "library_changed"}),
                false,
                0
            ),
            "{name}"
        );
    }
}

#[tokio::test]
async fn a_favorite_from_the_phone_counts_once_and_refuses_a_stale_generation() {
    let server = Server::start().await;
    let north = save(&server.library, capture("north", None));
    let request = uuid::Uuid::now_v7().to_string();
    let body = json!({"requestId": request, "expected": "0", "favorite": true});

    // 一度目で付き、同じ要求を送り直しても世代は進まない（受領を返す）。
    for _ in 0..2 {
        let stored: piep_mobile_api::MobileFavorite = server
            .favorite_as(north.entity_id, body.clone(), "fixture")
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!((stored.favorite, stored.generation.as_str()), (true, "1"));
    }
    // 一覧と作品の画面は、この利用者の値と世代を出す。別の利用者の値は別。
    let page = shelf_page(server.search(json!({})).send().await.unwrap()).await;
    assert!(page.items[0].favorite);
    assert_eq!(page.items[0].favorite_generation, "1");
    let other = shelf_page(server.search_as(json!({}), "second").send().await.unwrap()).await;
    assert!(!other.items[0].favorite);
    assert_eq!(other.items[0].favorite_generation, "0");

    // 古い世代からの変更は断り、今の値を返す（端末の時刻で勝者を決めない）。
    let stale = server
        .favorite_as(
            north.entity_id,
            json!({"requestId": uuid::Uuid::now_v7().to_string(), "expected": "0", "favorite": false}),
            "fixture",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let conflict: serde_json::Value = stale.json().await.unwrap();
    assert_eq!(conflict["code"], "favorite_conflict");
    assert_eq!(
        conflict["current"],
        json!({"favorite": true, "generation": "1"})
    );

    // 形の崩れた値・知らない作品・棚から外した作品・権限の無い端末は断る。
    for (work, body, status) in [
        (
            north.entity_id,
            json!({"requestId": "x", "expected": "1", "favorite": false}),
            StatusCode::BAD_REQUEST,
        ),
        (
            north.entity_id,
            json!({"requestId": uuid::Uuid::now_v7().to_string(), "expected": "01", "favorite": false}),
            StatusCode::BAD_REQUEST,
        ),
        (
            north.entity_id,
            json!({"requestId": uuid::Uuid::now_v7().to_string(), "expected": "-1", "favorite": false}),
            StatusCode::BAD_REQUEST,
        ),
        (
            north.entity_id,
            json!({"requestId": uuid::Uuid::now_v7().to_string(), "expected": "1", "favorite": false, "extra": 1}),
            StatusCode::BAD_REQUEST,
        ),
        (
            uuid::Uuid::now_v7(),
            json!({"requestId": uuid::Uuid::now_v7().to_string(), "expected": "0", "favorite": true}),
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(
            server
                .favorite_as(work, body, "fixture")
                .send()
                .await
                .unwrap()
                .status(),
            status
        );
    }
    assert_eq!(
        server
            .favorite_as(north.entity_id, json!({"requestId": uuid::Uuid::now_v7().to_string(), "expected": "1", "favorite": false}), "nobody")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );

    // 外した作品には書けない。
    server
        .library
        .remove_work(uuid::Uuid::now_v7(), north.entity_id, 1)
        .unwrap();
    assert_eq!(
        server
            .favorite_as(north.entity_id, json!({"requestId": uuid::Uuid::now_v7().to_string(), "expected": "2", "favorite": true}), "fixture")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn edit_reads_are_limited_to_the_subjects_published_and_archived_views() {
    let server = Server::start().await;
    let saved = save(&server.library, capture("edited", None));
    let content = crate::library::EditContent {
        format: "piep-edit-v1".into(),
        title: None,
        blocks: vec![],
        media: vec![],
        provenance: None,
    };
    let save_edit = |user, expected, activate| {
        server
            .library
            .save_edit(crate::library::EditRequest {
                request_id: uuid::Uuid::now_v7(),
                work: saved.entity_id,
                user,
                base: saved.snapshot_id,
                parent: None,
                expected,
                content: &content,
                activate,
            })
            .unwrap()
    };
    let first = save_edit(server.authority.subject.get(), 0, true);
    let draft = save_edit(server.authority.subject.get(), 1, false);
    let second = save_edit(server.authority.other_subject.get(), 0, true);
    let shelf = format!("/api/libraries/{}", server.library.id);
    let lookup = format!("{shelf}/works/lookup?provider=pixiv&key=edited");
    for (token, expected) in [("fixture", first), ("second", second)] {
        let view: protocol::WorkReadingView = server
            .get_as(&lookup, token)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(view.edit_id, Some(Id::new(expected).unwrap()));
        let list: protocol::ReaderWorkPage = server
            .get_as(&format!("{shelf}/works"), token)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(list.items[0].view, view);
    }

    let edit_path = |edit| format!("{shelf}/entities/{}/views/edit/{edit}", saved.entity_id);
    let paths = |edit| {
        let view = edit_path(edit);
        let descriptor = server
            .library
            .resource_transfer_manifest(
                saved.entity_id,
                ResourceScope::Edit(edit),
                &ResourceReference::Body,
            )
            .unwrap();
        [
            format!("{view}/bundle"),
            format!("{view}/resources/body/manifest"),
            format!(
                "{view}/resources/body/files/{}/chunks/0",
                descriptor.file.id.get()
            ),
        ]
    };
    for path in paths(first) {
        assert_eq!(
            server.get(&path).send().await.unwrap().status(),
            StatusCode::OK
        );
        assert_eq!(
            server
                .get_as(&path, "second")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
    for path in paths(draft) {
        assert_eq!(
            server.get(&path).send().await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
    }

    let change = |generation, change| {
        server
            .library
            .change_edit_pointers(
                uuid::Uuid::now_v7(),
                saved.entity_id,
                server.authority.subject.get(),
                generation,
                change,
            )
            .unwrap()
    };
    let bundle = format!("{}/bundle", edit_path(first));
    let archived = change(2, crate::library::EditPointerChange::Unpublish);
    assert_eq!(
        server.get(&bundle).send().await.unwrap().status(),
        StatusCode::OK
    );
    change(
        archived.generation,
        crate::library::EditPointerChange::Forget { edit: first },
    );
    assert_eq!(
        server.get(&bundle).send().await.unwrap().status(),
        StatusCode::NOT_FOUND
    );

    for path in paths(second) {
        assert_eq!(
            server
                .get_as(&path, "second")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK,
            "{path}"
        );
    }
    let generation = server
        .library
        .library_item_generation(saved.entity_id)
        .unwrap()
        .unwrap();
    server
        .library
        .remove_work(uuid::Uuid::now_v7(), saved.entity_id, generation)
        .unwrap();
    for path in paths(second) {
        assert_eq!(
            server
                .get_as(&path, "second")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
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

/// 作者 p1 の作品 w1・w2（シリーズ s1）と、作者 p2 の作品 w3。p1 はアイコンとヘッダーを持つ。
fn grouped_shelf(library: &Library) -> BTreeMap<&'static str, uuid::Uuid> {
    let mut works = BTreeMap::new();
    for (key, author, series) in [
        ("w1", "p1", Some(1)),
        ("w2", "p1", Some(2)),
        ("w3", "p2", None),
    ] {
        let mut input = capture(key, Some(&png()));
        input
            .metadata
            .insert("author_name".into(), json!(format!("{author} の名")));
        input.metadata.insert("author_key".into(), json!(author));
        input.metadata.insert("tags".into(), json!(["a"]));
        if let Some(position) = series {
            input.metadata.insert("series_id".into(), json!("s1"));
            input
                .metadata
                .insert("series_title".into(), json!("s1 の題"));
            input
                .metadata
                .insert("series_position".into(), json!(position));
        }
        works.insert(key, save(library, input).entity_id);
    }
    let mut person = capture("p1", Some(&png()));
    person.kind = EntityKind::Person;
    person.metadata = BTreeMap::from([
        ("display_name".into(), json!("p1 の名")),
        ("description".into(), json!("紹介")),
        (
            "links".into(),
            json!(["https://example.test/p1", "javascript:alert(1)"]),
        ),
    ]);
    person.media[0].slot = "icon".into();
    person.media[0].role = "icon".into();
    let mut banner = capture("p1-banner", Some(&png())).media.remove(0);
    banner.slot = "banner".into();
    banner.role = "banner".into();
    person.media.push(banner);
    save(library, person);
    works
}

impl Server {
    fn shelf(&self, rest: &str) -> String {
        format!("/api/libraries/{}{rest}", self.library.id)
    }
    async fn read<T: serde::de::DeserializeOwned>(&self, rest: &str, token: &str) -> T {
        self.get_as(&self.shelf(rest), token)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn people_series_and_collections_come_from_the_shelf() {
    let server = Server::start().await;
    let works = grouped_shelf(&server.library);
    let collection = uuid::Uuid::now_v7();
    let member = |key: &str| crate::library::CollectionMember {
        work_id: works[key],
        title_at_add: format!("{key} を足した時の題"),
        author_at_add: "足した時の作者".into(),
        role: "main".into(),
        added_by: "manual".into(),
        pinned: false,
        note: None,
    };
    server
        .library
        .save_collection(crate::library::CollectionSave {
            request_id: uuid::Uuid::now_v7(),
            id: collection,
            expected_generation: 0,
            title: "順に読む".into(),
            description: None,
            ordered: true,
            members: vec![member("w2"), member("w1")],
        })
        .unwrap();

    // 作者の一覧。作品の多い順で、続きは何件目からか。
    let people: piep_mobile_api::MobileEntityPage = server.read("/people?limit=1", "fixture").await;
    assert_eq!(people.total, 2);
    assert_eq!(people.next_offset, Some(1));
    assert_eq!(people.items[0].source_key, "p1");
    assert_eq!(people.items[0].count, 2);
    assert!(people.items[0].has_cover);
    let rest: piep_mobile_api::MobileEntityPage = server
        .read("/people?offset=1&sort=name&order=desc", "fixture")
        .await;
    assert_eq!(rest.items.len(), 1);
    assert_eq!(rest.next_offset, None);
    let named: piep_mobile_api::MobileEntityPage = server.read("/people?text=p2", "fixture").await;
    assert_eq!(named.total, 1);
    let series: piep_mobile_api::MobileEntityPage = server.read("/series", "fixture").await;
    assert_eq!(series.items[0].source_key, "s1");
    assert_eq!(series.items[0].count, 2);

    // 作者の画面。外へ開けないリンクは渡さない。
    let p1: piep_mobile_api::MobileEntityDetail = server
        .read("/people/detail?provider=pixiv&key=p1", "fixture")
        .await;
    assert_eq!(p1.display_name, "p1 の名");
    assert_eq!(p1.work_count, 2);
    assert!(p1.has_cover && p1.has_banner);
    assert_eq!(p1.links, vec!["https://example.test/p1".to_string()]);
    assert_eq!(p1.series_count, Some(1));
    assert_eq!(p1.tags[0].name, "a");
    assert_eq!(p1.collections.len(), 1);
    assert_eq!(p1.collections[0].id, collection.to_string());
    let p2: piep_mobile_api::MobileEntityDetail = server
        .read("/people/detail?provider=pixiv&key=p2", "fixture")
        .await;
    assert!(p2.collections.is_empty() && !p2.has_cover);
    let s1: piep_mobile_api::MobileEntityDetail = server
        .read("/series/detail?provider=pixiv&key=s1", "fixture")
        .await;
    assert_eq!((s1.work_count, s1.series_count), (2, None));
    let own: piep_mobile_api::MobileEntitySeriesPage = server
        .read("/people/series?provider=pixiv&key=p1", "fixture")
        .await;
    assert_eq!(own.total, 1);
    assert_eq!(own.items[0].source_key, "s1");
    assert_eq!(own.next_cursor, None);

    // 画像。作者はアイコンとヘッダー、シリーズのヘッダーは無い。
    for rest in [
        "/people/cover?provider=pixiv&key=p1",
        "/people/cover?provider=pixiv&key=p1&variant=banner",
    ] {
        let response = server.get(&server.shelf(rest)).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{rest}");
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/webp");
    }

    // コレクション。回は並べた順で、作品は棚の作品の形。お気に入りは利用者ごと。
    server
        .favorite_as(
            works["w1"],
            json!({"requestId": uuid::Uuid::now_v7().to_string(), "expected": "0", "favorite": true}),
            "fixture",
        )
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    let list: Vec<piep_mobile_api::MobileCollection> = server.read("/collections", "fixture").await;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].collection_kind, "ordered");
    assert_eq!((list[0].member_count, list[0].available_count), (2, 2));
    assert_eq!(list[0].cover_tiles.len(), 2);
    assert!(list[0].cover_tiles.iter().all(|tile| tile.has_cover));
    let detail: piep_mobile_api::MobileCollectionDetail = server
        .read(&format!("/collections/{collection}"), "fixture")
        .await;
    let order: Vec<_> = detail
        .members
        .iter()
        .map(|member| member.work.as_ref().unwrap().id.clone())
        .collect();
    assert_eq!(
        order,
        vec![works["w2"].to_string(), works["w1"].to_string()]
    );
    assert_eq!(detail.members[1].position, 1);
    assert!(detail.members[1].work.as_ref().unwrap().favorite);
    let other: piep_mobile_api::MobileCollectionDetail = server
        .read(&format!("/collections/{collection}"), "second")
        .await;
    assert!(!other.members[1].work.as_ref().unwrap().favorite);
    let tile = server
        .get(&server.shelf(&format!("/collections/{collection}/cover/tile-0")))
        .send()
        .await
        .unwrap();
    assert_eq!(tile.status(), StatusCode::OK);
    let memberships: Vec<piep_mobile_api::MobileCollection> = server
        .read(&format!("/works/{}/collections", works["w1"]), "fixture")
        .await;
    assert_eq!(memberships.len(), 1);
    assert_eq!(memberships[0].position, Some(1));
    let none: Vec<piep_mobile_api::MobileCollection> = server
        .read(&format!("/works/{}/collections", works["w3"]), "fixture")
        .await;
    assert!(none.is_empty());

    // 棚に無いもの・知らない条件は断る。
    for (rest, status) in [
        (
            "/people/detail?provider=pixiv&key=nobody".to_string(),
            StatusCode::NOT_FOUND,
        ),
        (
            "/people/detail?provider=pixiv&key=".to_string(),
            StatusCode::NOT_FOUND,
        ),
        (
            "/people/cover?provider=pixiv&key=p2".to_string(),
            StatusCode::NOT_FOUND,
        ),
        (
            "/series/cover?provider=pixiv&key=s1&variant=banner".to_string(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/people/series?provider=pixiv&key=nobody".to_string(),
            StatusCode::NOT_FOUND,
        ),
        (
            "/people/series?provider=pixiv&key=p1&cursor=-1".to_string(),
            StatusCode::BAD_REQUEST,
        ),
        ("/people?sort=bogus".to_string(), StatusCode::BAD_REQUEST),
        ("/people?order=up".to_string(), StatusCode::BAD_REQUEST),
        ("/people?extra=1".to_string(), StatusCode::BAD_REQUEST),
        (
            format!("/people?text={}", "x".repeat(201)),
            StatusCode::BAD_REQUEST,
        ),
        (
            format!("/collections/{}", uuid::Uuid::now_v7()),
            StatusCode::NOT_FOUND,
        ),
        (
            "/collections/not-moved-here".to_string(),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/collections/{collection}/cover/tile-3"),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/collections/{collection}/cover/single"),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/collections/{collection}/cover/other"),
            StatusCode::NOT_FOUND,
        ),
        (
            format!("/works/{}/collections", uuid::Uuid::now_v7()),
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(
            server
                .get(&server.shelf(&rest))
                .send()
                .await
                .unwrap()
                .status(),
            status,
            "{rest}"
        );
    }
    assert_eq!(
        server
            .get(&format!("/api/libraries/{}/people", uuid::Uuid::now_v7()))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn removing_a_work_hides_it_from_lists_search_and_group_pages() {
    let server = Server::start().await;
    let works = grouped_shelf(&server.library);
    let removed = works["w1"];
    let kept = works["w2"].to_string();
    server.library.sync_search_index(&mut |_, _| {}).unwrap();
    server.context.search_ready.store(true, Ordering::SeqCst);

    for on_shelf in [true, false] {
        if !on_shelf {
            server
                .library
                .remove_work(uuid::Uuid::now_v7(), removed, 1)
                .unwrap();
        }
        let page: protocol::ReaderWorkPage = server.read("/works", "fixture").await;
        assert_eq!(page.items.len(), if on_shelf { 3 } else { 2 });
        assert_eq!(
            page.items
                .iter()
                .any(|work| work.view.entity_id.get() == removed),
            on_shelf
        );

        for (query, grouped) in [
            (json!({"text": "w1"}), false),
            (json!({"personSource": "pixiv", "personKey": "p1"}), true),
            (json!({"seriesSource": "pixiv", "seriesKey": "s1"}), true),
        ] {
            let page = shelf_page(server.search(query.clone()).send().await.unwrap()).await;
            assert_eq!(
                page.items.len(),
                usize::from(on_shelf) + usize::from(grouped),
                "{query}"
            );
            assert_eq!(
                page.items.iter().any(|work| work.id == removed.to_string()),
                on_shelf,
                "{query}"
            );
            assert_eq!(
                page.items.iter().any(|work| work.id == kept),
                grouped,
                "{query}"
            );
        }

        for (route, key) in [("people", "p1"), ("series", "s1")] {
            let count = if on_shelf { 2 } else { 1 };
            let page: piep_mobile_api::MobileEntityPage =
                server.read(&format!("/{route}"), "fixture").await;
            assert_eq!(
                page.items
                    .iter()
                    .find(|item| item.source_key == key)
                    .unwrap()
                    .count,
                count
            );
            let detail: piep_mobile_api::MobileEntityDetail = server
                .read(
                    &format!("/{route}/detail?provider=pixiv&key={key}"),
                    "fixture",
                )
                .await;
            assert_eq!(detail.work_count, count);
        }
    }
}

#[tokio::test]
async fn search_suggestions_come_from_the_shelf() {
    let server = Server::start().await;
    let works = grouped_shelf(&server.library);

    // 作者は取得元と ID、作品の多い順。一致した名前は先に出る。
    let found: piep_mobile_api::MobileSearchSuggestions =
        server.read("/works/suggest?text=p", "fixture").await;
    let authors: Vec<_> = found.items.iter().filter(|i| i.kind == "author").collect();
    assert_eq!(authors.len(), 2);
    assert_eq!(authors[0].source_key.as_deref(), Some("p1"));
    assert_eq!(authors[0].source.as_deref(), Some("pixiv"));
    assert_eq!(authors[0].count, Some(2));
    let exact: piep_mobile_api::MobileSearchSuggestions = server
        .read("/works/suggest?text=p2%20%E3%81%AE%E5%90%8D", "fixture")
        .await;
    assert!(exact.items[0].exact_match);
    assert_eq!(exact.items[0].source_key.as_deref(), Some("p2"));

    // シリーズは「取得元:ID」、題は取得元の ID と棚の作品の ID、タグはタグ。
    let series: piep_mobile_api::MobileSearchSuggestions =
        server.read("/works/suggest?text=s1", "fixture").await;
    assert_eq!(series.items[0].kind, "series");
    assert_eq!(series.items[0].value, "pixiv:s1");
    assert_eq!(series.items[0].count, Some(2));
    let titles: piep_mobile_api::MobileSearchSuggestions =
        server.read("/works/suggest?text=w3", "fixture").await;
    assert_eq!(titles.items.len(), 1);
    assert_eq!(titles.items[0].kind, "title");
    assert_eq!(titles.items[0].value, "w3");
    assert_eq!(
        titles.items[0].source_key.as_deref(),
        Some(works["w3"].to_string().as_str())
    );
    let tags: piep_mobile_api::MobileSearchSuggestions = server
        .read("/works/suggest?text=a&limit=1", "fixture")
        .await;
    assert_eq!(tags.items.len(), 1);
    assert_eq!(
        (tags.items[0].kind.as_str(), tags.items[0].count),
        ("tag", Some(3))
    );

    // 前方一致で足りなければ、名前の途中も拾う。
    let infix: piep_mobile_api::MobileSearchSuggestions = server
        .read("/works/suggest?text=%E3%81%AE%E9%A1%8C", "fixture")
        .await;
    assert!(infix.items.iter().any(|i| i.kind == "series"));

    // 空の語は何も出さず、長すぎる語と知らない条件は断る。
    let empty: piep_mobile_api::MobileSearchSuggestions =
        server.read("/works/suggest?text=%20", "fixture").await;
    assert!(empty.items.is_empty());
    let long = "a".repeat(201);
    for rest in [
        format!("/works/suggest?text={long}"),
        "/works/suggest?text=a&x=1".into(),
    ] {
        let response = server.get(&server.shelf(&rest)).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

impl Server {
    fn reading_as(
        &self,
        work: uuid::Uuid,
        body: serde_json::Value,
        token: &str,
    ) -> reqwest::RequestBuilder {
        self.with_write_scope(self.reading_request(work, body, token), token)
    }

    fn with_write_scope(
        &self,
        request: reqwest::RequestBuilder,
        token: &str,
    ) -> reqwest::RequestBuilder {
        let subject = if token == "second" {
            self.authority.other_subject
        } else {
            self.authority.subject
        };
        request
            .header("x-piep-subject-id", subject.get().to_string())
            .header(
                "x-piep-sync-epoch",
                self.context.sync_epoch.get().to_string(),
            )
    }

    fn reading_request(
        &self,
        work: uuid::Uuid,
        body: serde_json::Value,
        token: &str,
    ) -> reqwest::RequestBuilder {
        self.client
            .put(format!(
                "{}/api/libraries/{}/works/{work}/reading",
                self.base, self.library.id
            ))
            .bearer_auth(token)
            .json(&body)
    }
    async fn reading_page(
        &self,
        cursor: &str,
        limit: usize,
        token: &str,
    ) -> piep_mobile_api::MobileReadingPage {
        self.client
            .get(format!(
                "{}/api/libraries/{}/reading",
                self.base, self.library.id
            ))
            .query(&[("cursor", cursor), ("limit", &limit.to_string())])
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn reading_write_from_another_scope_cannot_change_the_shelf() {
    let server = Server::start().await;
    let work = save(&server.library, capture("stale-reading", None));
    let body = json!({
        "requestId": uuid::Uuid::now_v7(), "expected": "0", "finished": false,
        "location": {"viewKind": "snapshot", "viewId": work.snapshot_id, "page": 0, "ratioPpm": 0}
    });
    let wrong = uuid::Uuid::now_v7();
    for (subject, epoch) in [
        (server.library.id, wrong),
        (wrong, server.context.sync_epoch.get()),
    ] {
        let response = server
            .reading_request(work.entity_id, body.clone(), "fixture")
            .header("x-piep-subject-id", subject.to_string())
            .header("x-piep-sync-epoch", epoch.to_string())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap()["code"],
            "library_changed"
        );
        assert_eq!(
            server
                .library
                .reading_position(work.entity_id, server.library.id)
                .unwrap()
                .generation,
            0
        );
    }
}

#[tokio::test]
async fn reading_positions_are_written_once_refuse_stale_generations_and_come_back_as_changes() {
    let server = Server::start().await;
    let mut paged = capture("north", None);
    paged.body = Some(crate::library::SourceBody::PixivText(
        "一[newpage]二".into(),
    ));
    let north = save(&server.library, paged);
    let south = save(&server.library, capture("south", None));
    let epoch = server.context.sync_epoch.get();
    let at = |snapshot: uuid::Uuid, page: u64, ratio: u32| json!({"viewKind": "snapshot", "viewId": snapshot.to_string(), "page": page, "ratioPpm": ratio});

    // まだ何も無い。全部を返し終えたら、差分の印になる。
    let empty = server.reading_page("", 200, "fixture").await;
    assert!(empty.items.is_empty() && !empty.more);
    assert_eq!(empty.cursor, format!("{epoch}.s.0"));

    // 一度目で書け、同じ要求を送り直しても世代は進まない。
    let request = uuid::Uuid::now_v7().to_string();
    let body = json!({"requestId": request, "expected": "0", "location": at(north.snapshot_id, 1, 500_000), "finished": false});
    for _ in 0..2 {
        let stored: piep_mobile_api::MobileReading = server
            .reading_as(north.entity_id, body.clone(), "fixture")
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(stored.generation, "1");
        assert!(stored.mine && !stored.finished);
        assert_eq!(
            stored.location,
            Some(piep_mobile_api::MobileReadingLocation {
                view_kind: "snapshot".into(),
                view_id: north.snapshot_id.to_string(),
                page: 1,
                ratio_ppm: 500_000,
            })
        );
    }
    // 棚に書いた端末は、サーバーの端末の識別子で記録される。
    let position = server
        .library
        .reading_position(north.entity_id, server.library.id)
        .unwrap();
    assert_eq!(
        position.device,
        Some(crate::library::server_device("phone"))
    );

    // 古い世代からの書き込みは断り、今の位置を返す。
    let stale = server
        .reading_as(
            north.entity_id,
            json!({"requestId": uuid::Uuid::now_v7().to_string(), "expected": "0", "location": at(north.snapshot_id, 0, 0), "finished": false}),
            "fixture",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let conflict: serde_json::Value = stale.json().await.unwrap();
    assert_eq!(conflict["code"], "reading_conflict");
    assert_eq!(conflict["current"]["generation"], "1");
    assert_eq!(conflict["current"]["location"]["page"], 1);

    // 写しに残っていない版は、更新世代が違っていても位置の拒否として返す。
    let missing_view = server
        .reading_as(
            north.entity_id,
            json!({"requestId": uuid::Uuid::now_v7(), "expected": "0", "location": at(uuid::Uuid::now_v7(), 0, 0), "finished": false}),
            "fixture",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(missing_view.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        missing_view.json::<serde_json::Value>().await.unwrap()["code"],
        "invalid_reading_location"
    );

    // 本文に無いページ・別の作品の版・形の崩れ・知らない作品・権限の無い端末は断る。
    let fresh = || uuid::Uuid::now_v7().to_string();
    for (work, body, status) in [
        (
            north.entity_id,
            json!({"requestId": fresh(), "expected": "1", "location": at(north.snapshot_id, 2, 0), "finished": false}),
            StatusCode::BAD_REQUEST,
        ),
        (
            north.entity_id,
            json!({"requestId": fresh(), "expected": "1", "location": at(south.snapshot_id, 0, 0), "finished": false}),
            StatusCode::BAD_REQUEST,
        ),
        (
            north.entity_id,
            json!({"requestId": fresh(), "expected": "1", "location": at(north.snapshot_id, 0, 1_000_001), "finished": false}),
            StatusCode::BAD_REQUEST,
        ),
        (
            north.entity_id,
            json!({"requestId": fresh(), "expected": "1", "location": {"viewKind": "other", "viewId": north.snapshot_id.to_string(), "page": 0, "ratioPpm": 0}, "finished": false}),
            StatusCode::BAD_REQUEST,
        ),
        (
            north.entity_id,
            json!({"requestId": fresh(), "expected": "01", "location": at(north.snapshot_id, 0, 0), "finished": false}),
            StatusCode::BAD_REQUEST,
        ),
        (
            north.entity_id,
            json!({"requestId": fresh(), "expected": "1", "location": at(north.snapshot_id, 0, 0), "finished": false, "extra": 1}),
            StatusCode::BAD_REQUEST,
        ),
        (
            north.entity_id,
            json!({"requestId": uuid::Uuid::nil().to_string(), "expected": "1", "location": at(north.snapshot_id, 0, 0), "finished": false}),
            StatusCode::BAD_REQUEST,
        ),
        (
            uuid::Uuid::now_v7(),
            json!({"requestId": fresh(), "expected": "0", "location": at(north.snapshot_id, 0, 0), "finished": false}),
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(
            server
                .reading_as(work, body.clone(), "fixture")
                .send()
                .await
                .unwrap()
                .status(),
            status,
            "{body}"
        );
    }
    assert_eq!(
        server
            .reading_as(north.entity_id, json!({"requestId": fresh(), "expected": "1", "location": at(north.snapshot_id, 0, 0), "finished": false}), "nobody")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );

    // デスクトップが同じ利用者として書いた位置も届く（この端末のものではない）。
    server
        .library
        .set_reading_position(crate::library::ReadingUpdate {
            request: uuid::Uuid::now_v7(),
            work: south.entity_id,
            user: server.library.id,
            device: uuid::Uuid::now_v7(),
            expected: 0,
            location: Some(crate::library::ReadingLocation::Source {
                snapshot: south.snapshot_id,
                anchor: crate::library::ReadingAnchor::Page {
                    page: 0,
                    ratio_ppm: 250_000,
                },
            }),
            finished: true,
        })
        .unwrap();
    let changes = server.reading_page(&empty.cursor, 200, "fixture").await;
    let works: Vec<_> = changes.items.iter().map(|item| item.work.clone()).collect();
    assert_eq!(
        works,
        [north.entity_id.to_string(), south.entity_id.to_string()]
    );
    assert!(!changes.items[1].mine && changes.items[1].finished);
    // 差分の続きは、また変わるまで空。
    let quiet = server.reading_page(&changes.cursor, 200, "fixture").await;
    assert!(quiet.items.is_empty());
    assert_eq!(quiet.cursor, changes.cursor);

    // 全部を一件ずつ返し、最後に差分の印へ移る。
    let first = server.reading_page("", 1, "fixture").await;
    assert!(first.more && first.items.len() == 1);
    let second = server.reading_page(&first.cursor, 1, "fixture").await;
    assert!(!second.more && second.items.len() == 1);
    assert_ne!(first.items[0].work, second.items[0].work);
    assert_eq!(second.cursor, changes.cursor);

    // ほかの利用者には、この利用者の位置を出さない。
    let other = server.reading_page("", 200, "second").await;
    assert!(other.items.is_empty());
    let other = server.reading_page(&empty.cursor, 200, "second").await;
    assert!(other.items.is_empty());

    // 別の写しの印や崩れた印は断る（端末は空の印から取り直す）。
    for cursor in [
        format!("{}.s.0", uuid::Uuid::now_v7()),
        format!("{epoch}.s.01"),
        "x".into(),
    ] {
        let response = server
            .client
            .get(format!(
                "{}/api/libraries/{}/reading",
                server.base, server.library.id
            ))
            .query(&[("cursor", cursor.as_str())])
            .bearer_auth("fixture")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{cursor}");
    }

    // 棚から外した作品には書けない。
    server
        .library
        .remove_work(uuid::Uuid::now_v7(), north.entity_id, 1)
        .unwrap();
    assert_eq!(
        server
            .reading_as(north.entity_id, json!({"requestId": fresh(), "expected": "1", "location": at(north.snapshot_id, 0, 0), "finished": false}), "fixture")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
}

/// デスクトップ版が管理の口から読む、持ち主の読書位置の一ページ。
async fn owner_page(context: &Arc<Context>, cursor: Option<&str>) -> serde_json::Value {
    let response = Arc::clone(context)
        .owner_reading(cursor.map(str::to_owned), Some(1))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn phone_positions_and_favorites_on_the_served_copy_come_back_to_the_desktop_shelf() {
    use crate::commands::shelf::adopt_remote_reading;
    use crate::library::{ReadingAnchor, ReadingLocation, ReadingPosition, ReadingUpdate};
    // デスクトップの棚と、その写しを配るサーバーの棚（同じ棚の ID・同じ作品の ID）。
    let desktop_dir = Temp::new();
    let desktop = Library::create(&desktop_dir.0).unwrap();
    let mut paged = capture("north", None);
    paged.body = Some(crate::library::SourceBody::PixivText(
        "一[newpage]二".into(),
    ));
    let north = save(&desktop, paged);
    let south = save(&desktop, capture("south", None));
    let served_dir = Temp::new();
    fs::create_dir_all(&served_dir.0).unwrap();
    desktop.backup(&served_dir.0.join("shelf")).unwrap();
    let served = Arc::new(Library::open(&served_dir.0.join("shelf")).unwrap());
    assert_eq!(served.id, desktop.id);
    let context = Arc::new(
        Context::new(
            Arc::clone(&served),
            Arc::new(TestAuthority {
                library: Id::new(served.id).unwrap(),
                subject: Id::new(served.id).unwrap(),
                other_subject: Id::new(uuid::Uuid::now_v7()).unwrap(),
                active: AtomicBool::new(true),
            }),
            Id::new(uuid::Uuid::now_v7()).unwrap(),
        )
        .unwrap(),
    );
    let page = |snapshot: uuid::Uuid, page: u64| ReadingLocation::Source {
        snapshot,
        anchor: ReadingAnchor::Page {
            page,
            ratio_ppm: 250_000,
        },
    };
    let write =
        |library: &Library, work: uuid::Uuid, device: uuid::Uuid, location: ReadingLocation| {
            let expected = library
                .reading_position(work, library.id)
                .unwrap()
                .generation;
            library
                .set_reading_position(ReadingUpdate {
                    request: uuid::Uuid::now_v7(),
                    work,
                    user: library.id,
                    device,
                    expected,
                    location: Some(location),
                    finished: false,
                })
                .unwrap()
        };
    let phone = crate::library::server_device("phone");
    // 携帯は写しの north を読み進めた。south はそのあとデスクトップで読んだ（デスクトップが新しい）。
    write(&served, north.entity_id, phone, page(north.snapshot_id, 1));
    write(&served, south.entity_id, phone, page(south.snapshot_id, 0));
    std::thread::sleep(std::time::Duration::from_millis(5));
    let mine = uuid::Uuid::now_v7();
    write(&desktop, south.entity_id, mine, page(south.snapshot_id, 0));
    let south_before = desktop
        .reading_position(south.entity_id, desktop.id)
        .unwrap();

    // 一件ずつ（limit 1）読み、印をたどって全部をデスクトップの棚へ当てる。
    let mut cursor: Option<String> = None;
    let mut adopted = 0;
    let mut seen = 0;
    loop {
        let body = owner_page(&context, cursor.as_deref()).await;
        assert_eq!(body["libraryId"], served.id.to_string());
        for item in body["items"].as_array().unwrap() {
            seen += 1;
            let work: uuid::Uuid = serde_json::from_value(item["work"].clone()).unwrap();
            let position: ReadingPosition =
                serde_json::from_value(item["position"].clone()).unwrap();
            adopted += usize::from(adopt_remote_reading(&desktop, work, &position, 0).unwrap());
        }
        cursor = Some(body["cursor"].as_str().unwrap().to_owned());
        if !body["more"].as_bool().unwrap() {
            break;
        }
    }
    assert_eq!((seen, adopted), (2, 1));
    let north_now = desktop
        .reading_position(north.entity_id, desktop.id)
        .unwrap();
    assert_eq!(north_now.location, Some(page(north.snapshot_id, 1)));
    // 書いた端末は携帯のまま（読む画面がほかの端末の位置として案内する）。
    assert_eq!(north_now.device, Some(phone));
    assert_eq!(
        desktop
            .reading_position(south.entity_id, desktop.id)
            .unwrap(),
        south_before,
        "デスクトップのほうが新しい位置は変えない"
    );

    // 差分の印からは、そのあと変わった作品だけが届く。同じ位置は二度書かない。
    let changes = owner_page(&context, cursor.as_deref()).await;
    assert!(changes["items"].as_array().unwrap().is_empty());
    std::thread::sleep(std::time::Duration::from_millis(5));
    write(&served, north.entity_id, phone, page(north.snapshot_id, 0));
    let changes = owner_page(&context, cursor.as_deref()).await;
    let items = changes["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    let position: ReadingPosition = serde_json::from_value(items[0]["position"].clone()).unwrap();
    assert!(adopt_remote_reading(&desktop, north.entity_id, &position, 0).unwrap());
    assert!(!adopt_remote_reading(&desktop, north.entity_id, &position, 0).unwrap());
    assert_eq!(
        desktop
            .reading_position(north.entity_id, desktop.id)
            .unwrap()
            .location,
        Some(page(north.snapshot_id, 0))
    );

    // 別の写しの印・崩れた印は初めから返す。
    let restarted = owner_page(&context, Some("garbage")).await;
    assert_eq!(restarted["items"].as_array().unwrap().len(), 1);
    assert!(restarted["more"].as_bool().unwrap());

    // デスクトップが書いた位置・棚に無い作品は写さない。
    let own = ReadingPosition {
        location: Some(page(south.snapshot_id, 0)),
        finished: true,
        device: Some(crate::commands::shelf::desktop_device(&desktop)),
        updated_at: Some(i64::MAX),
        generation: 9,
    };
    assert!(!adopt_remote_reading(&desktop, south.entity_id, &own, 0).unwrap());
    let unknown = ReadingPosition {
        device: Some(phone),
        ..own.clone()
    };
    assert!(!adopt_remote_reading(&desktop, uuid::Uuid::now_v7(), &unknown, 0).unwrap());
    // 本文に無いページは写さず、止めもしない。
    let beyond = ReadingPosition {
        location: Some(page(south.snapshot_id, 5)),
        ..unknown
    };
    assert!(!adopt_remote_reading(&desktop, south.entity_id, &beyond, 0).unwrap());

    desktop.verify().unwrap();
}
