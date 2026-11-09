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
    fn subject(&self, bearer: &str, library: Id) -> Option<Id> {
        (bearer == "fixture" && library == self.library && self.active.load(Ordering::SeqCst))
            .then_some(self.subject)
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
async fn read_capacity_is_bounded_and_scope_headers_identify_the_epoch() {
    let server = Server::start().await;
    let saved = save(&server.library, capture("one", Some(b"image")));
    let path = format!(
        "{}/manifest?slot=cover",
        server.resource(saved.entity_id, saved.snapshot_id)
    );
    let held: Vec<_> = (0..4)
        .map(|_| read_permit(&server.context).unwrap())
        .collect();
    assert_eq!(
        server.get(&path).send().await.unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
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
        server
            .get(&path)
            .header(header::IF_NONE_MATCH, "*")
            .send()
            .await
            .unwrap()
            .status(),
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
async fn identity_requires_bearer_and_exposes_no_reader_or_mutation_capability() {
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
    assert!(!identity.capabilities.reader_bundle && !identity.capabilities.mutations);
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
async fn transfer_roundtrips_and_conditional_reads_recheck_authority_and_ownership() {
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
    let manifest_etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_owned();
    let bytes = response.bytes().await.unwrap();
    let transfer = protocol::ValidatedTransfer::parse(&bytes).unwrap();
    assert_eq!(manifest_etag, format!("\"sha256-{}\"", transfer.id()));
    let path = format!(
        "{resource}/files/{}/chunks/0?slot=cover",
        transfer.file().id
    );
    let response = server.get(&path).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_owned();
    let encoded = response.bytes().await.unwrap();
    assert_eq!(
        etag,
        format!("\"sha256-{}\"", crate::library::Fingerprint::of(&encoded))
    );
    assert_eq!(
        piep_storage::decode_chunk(&encoded, &transfer.file().chunks[0]).unwrap(),
        original
    );
    for conditional in [
        etag.clone(),
        format!("W/{etag}"),
        format!("\"other\", {etag}"),
        "*".into(),
    ] {
        let response = server
            .get(&path)
            .header(header::IF_NONE_MATCH, conditional)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert!(response.bytes().await.unwrap().is_empty());
    }
    let wrong = path.replace(&saved.entity_id.to_string(), &foreign.entity_id.to_string());
    assert_eq!(
        server
            .get(&wrong)
            .header(header::IF_NONE_MATCH, "*")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    let wrong = path.replace(
        &transfer.file().id.to_string(),
        &crate::library::Fingerprint::of(b"other").to_string(),
    );
    assert_eq!(
        server
            .get(&wrong)
            .header(header::IF_NONE_MATCH, "*")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    server.authority.active.store(false, Ordering::SeqCst);
    assert_eq!(
        server
            .get(&path)
            .header(header::IF_NONE_MATCH, etag)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
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
async fn corrupt_cached_bytes_never_become_a_not_modified_response_or_expose_paths() {
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
    let response = server
        .get(&url)
        .header(header::IF_NONE_MATCH, "*")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let text = response.text().await.unwrap();
    assert!(
        !text.contains("objects") && !text.contains(&server.temp.0.to_string_lossy().to_string())
    );
}
