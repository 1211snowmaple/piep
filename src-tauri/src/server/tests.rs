//! サーバーモードを通しで確かめる。本物の HTTP で話し、本物のファイルと DB を動かす。
//! 棚を読む口（`/api/libraries/...`）の試験は `reader/tests.rs`、棚の送信は `standalone_shipment_tests.rs`。

use super::http::{
    admin_router, combined_router, reader_router, AdminAsset, AdminWeb, ConnectionInfo,
};
use super::standalone::{
    assemble, command_admin_pair, command_init, run_listener, ServerPaths, StandaloneParts,
};
use super::store::{random_token, Role, ServerStore};
use super::sync_send::{pair_admin, ClientCache, RemoteServer};
use super::API_VERSION;
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
        for attempt in 0..6 {
            if fs::remove_dir_all(&self.0).is_ok() || !self.0.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5 << attempt));
        }
    }
}

async fn pair_with(client: &reqwest::Client, url: &str, secret: &str) -> serde_json::Value {
    let paired = client
        .post(format!("{url}/api/pair"))
        .json(&serde_json::json!({ "secret": secret, "deviceName": "test Android" }))
        .send()
        .await
        .expect("pair");
    assert_eq!(paired.status(), StatusCode::OK);
    paired.json().await.expect("pair JSON")
}

#[tokio::test]
async fn admin_pairing_checks_the_reply_version_and_role() {
    for (version, role, accepted) in [
        (API_VERSION, "admin", true),
        (API_VERSION + 1, "admin", false),
        (API_VERSION, "reader", false),
    ] {
        let url = spawn_plain(axum::Router::new().route(
            "/api/pair",
            axum::routing::post(move || async move {
                axum::Json(serde_json::json!({
                    "apiVersion": version,
                    "role": role,
                    "deviceId": "fixture",
                    "token": "fixture",
                }))
            }),
        ));
        let remote = RemoteServer {
            url,
            fingerprint: None,
        };
        let result = pair_admin(&ClientCache::default(), &remote, "fixture", "試験端末").await;
        assert_eq!(result.is_ok(), accepted, "{version} {role}");
    }
}

#[tokio::test]
async fn self_signed_https_requires_pairing_and_revocation_takes_effect() {
    let root = TestRoot::created("self_signed");
    let (parts, _paths) = standalone(&root.0);
    let material = super::tls::self_signed(&root.0.join("tls"))
        .await
        .expect("tls");
    let address = TcpListener::bind("127.0.0.1:0")
        .expect("bind")
        .local_addr()
        .expect("address");
    let handle = axum_server::Handle::new();
    let router = combined_router(Arc::clone(&parts.context));
    let served = handle.clone();
    tokio::spawn(async move {
        run_listener(address, Some(material.config), router, served)
            .await
            .expect("serve");
    });
    let expected_fingerprint = material.fingerprint;
    let url = format!("https://{address}");

    let probe = reqwest::Client::builder()
        .tls_backend_rustls()
        .tls_danger_accept_invalid_certs(true)
        .tls_info(true)
        .no_proxy()
        .build()
        .expect("probe client");
    let mut hello = None;
    for _ in 0..40 {
        match probe.get(format!("{url}/api/hello")).send().await {
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
    assert_eq!(Some(fingerprint), expected_fingerprint);
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
        .get(format!("{url}/api/about"))
        .send()
        .await
        .expect("request");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let secret = parts
        .context
        .store
        .issue_pairing(Role::Reader, Duration::from_secs(600))
        .expect("pairing");
    let credentials = pair_with(&client, &url, &secret).await;
    let token = credentials["token"].as_str().expect("token");
    let device_id = credentials["deviceId"].as_str().expect("device id");

    // 一回限りの鍵は、使ったら終わり。
    let again = client
        .post(format!("{url}/api/pair"))
        .json(&serde_json::json!({ "secret": secret, "deviceName": "another" }))
        .send()
        .await
        .expect("second pair");
    assert_eq!(again.status(), StatusCode::UNAUTHORIZED);

    let about = client
        .get(format!("{url}/api/about"))
        .bearer_auth(token)
        .send()
        .await
        .expect("about");
    assert_eq!(about.status(), StatusCode::OK);

    // 読み手の鍵では管理の API を使えない。
    let admin = client
        .get(format!("{url}/api/admin/status"))
        .bearer_auth(token)
        .send()
        .await
        .expect("admin");
    assert_eq!(admin.status(), StatusCode::FORBIDDEN);

    assert!(parts
        .context
        .store
        .revoke_device(device_id)
        .expect("revoke"));
    let revoked = client
        .get(format!("{url}/api/about"))
        .bearer_auth(token)
        .send()
        .await
        .expect("revoked");
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);

    handle.shutdown();
}

pub(super) fn spawn_plain(router: axum::Router) -> String {
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

#[tokio::test]
async fn repeated_admin_requests_probe_only_once_per_remote_setting() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let root = TestRoot::created("desktop-probe");
    let tls = super::tls::self_signed(&root.0).await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let remote = RemoteServer {
        url: format!("https://{}", listener.local_addr().unwrap()),
        fingerprint: tls.fingerprint,
    };
    let probes = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&probes);
    let router = axum::Router::new()
        .route(
            "/api/hello",
            axum::routing::get(move || async move {
                count.fetch_add(1, Ordering::Relaxed);
                StatusCode::OK
            }),
        )
        .route(
            "/api/admin/status",
            axum::routing::get(|| async { axum::Json(serde_json::json!({ "ready": true })) }),
        );
    let serving = tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls.config)
            .unwrap()
            .serve(router.into_make_service())
            .await
            .unwrap();
    });
    let clients = ClientCache::default();
    for _ in 0..3 {
        let status = super::sync_send::admin_get(&clients, &remote, "test", "status")
            .await
            .unwrap();
        assert_eq!(status["ready"], true);
    }
    assert_eq!(probes.load(Ordering::Relaxed), 1);
    let changed = RemoteServer {
        fingerprint: Some("0".repeat(64)),
        ..remote.clone()
    };
    assert!(
        super::sync_send::admin_get(&clients, &changed, "test", "status")
            .await
            .is_err()
    );
    assert_eq!(probes.load(Ordering::Relaxed), 2);
    assert!(
        super::sync_send::admin_get(&clients, &remote, "test", "status")
            .await
            .is_ok()
    );
    assert_eq!(probes.load(Ordering::Relaxed), 3);
    serving.abort();
}

#[tokio::test]
async fn a_hanging_admin_request_has_a_short_timeout() {
    let remote = RemoteServer {
        url: spawn_plain(axum::Router::new().route(
            "/api/admin/shelf/reading",
            axum::routing::get(std::future::pending::<StatusCode>),
        )),
        fingerprint: None,
    };
    let result = tokio::time::timeout(
        Duration::from_secs(40),
        super::sync_send::admin_get_if_present(
            &ClientCache::default(),
            &remote,
            "test",
            "shelf/reading",
        ),
    )
    .await
    .expect("応答のない要求は短い制限時間で終わる");
    assert_eq!(
        result.unwrap_err(),
        "サーバーの応答が時間内にありませんでした"
    );
}

fn standalone(root: &Path) -> (StandaloneParts, ServerPaths) {
    let paths = ServerPaths::new(&root.join("server")).expect("paths");
    fs::create_dir_all(paths.state()).expect("state");
    (assemble(&paths, None, None).expect("assemble"), paths)
}

async fn request_pairing(
    role: Role,
    public_url: Option<&str>,
    page_url: &str,
) -> (StatusCode, serde_json::Value) {
    let root = TestRoot::created("pairing-url");
    let paths = ServerPaths::new(&root.0).unwrap();
    fs::create_dir_all(paths.state()).unwrap();
    let parts = assemble(&paths, public_url.map(str::to_owned), None).unwrap();
    let store = &parts.context.store;
    let secret = store
        .issue_pairing(Role::Admin, Duration::from_secs(60))
        .unwrap();
    let admin = store.consume_pairing(&secret, "試験端末").unwrap();
    let handle = axum_server::Handle::new();
    let mut server = tokio::spawn(run_listener(
        "127.0.0.1:0".parse().unwrap(),
        None,
        combined_router(Arc::clone(&parts.context)),
        handle.clone(),
    ));
    let address = tokio::select! {
        address = handle.listening() => address.unwrap(),
        result = &mut server => panic!("試験サーバーが待ち受けを始めませんでした: {result:?}"),
    };
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("http://{address}/api/admin/pairings"))
        .bearer_auth(&admin.token)
        .json(&serde_json::json!({ "role": role.as_str(), "url": page_url }))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.json().await.unwrap();
    handle.graceful_shutdown(Some(Duration::from_secs(2)));
    server.await.unwrap().unwrap();
    assert!(store.authorize(&admin.token).is_some());
    (status, body)
}

fn assert_pairing_url((status, body): (StatusCode, serde_json::Value), role: Role, expected: &str) {
    assert_eq!(status, StatusCode::OK);
    let payload: serde_json::Value =
        serde_json::from_str(body["payload"].as_str().unwrap()).unwrap();
    assert_eq!(payload["role"], role.as_str());
    assert_eq!(payload["url"], expected);
}

#[tokio::test]
async fn admin_replacement_pairing_prefers_the_public_url() {
    let public_url = "https://server.example/shelf/";
    let response = request_pairing(Role::Admin, Some(public_url), "http://127.0.0.1:38778").await;
    assert_pairing_url(response, Role::Admin, public_url);
}

#[tokio::test]
async fn admin_replacement_pairing_refuses_a_loopback_page_without_a_public_url() {
    for url in [
        "http://127.0.0.1:38778",
        "https://127.42.0.1",
        "https://[::1]",
        "https://[::ffff:127.0.0.1]",
        "https://localhost.",
    ] {
        let (status, body) = request_pairing(Role::Admin, None, url).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{url}");
        assert_eq!(
            body["message"],
            "このアドレスはこの PC でしか使えません。登録する端末から届く URL を使ってください"
        );
        assert!(body.get("payload").is_none());
    }
}

#[tokio::test]
async fn admin_replacement_pairing_uses_the_page_url_without_a_public_url() {
    let page_url = "https://server.example:38777";
    let response = request_pairing(Role::Admin, None, page_url).await;
    assert_pairing_url(response, Role::Admin, page_url);
}

#[tokio::test]
async fn reader_pairing_prefers_the_public_url_and_refuses_a_loopback_fallback() {
    let public_url = "https://server.example/shelf/";
    let local_url = "https://localhost:38777/";
    let response = request_pairing(Role::Reader, Some(public_url), local_url).await;
    assert_pairing_url(response, Role::Reader, public_url);

    let (status, _) = request_pairing(Role::Reader, None, local_url).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
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
        ("POST", "/api/admin/pairings"),
        ("POST", "/api/admin/shelf/shipments"),
        ("POST", "/api/admin/shelf/shipments/unused/needed"),
        ("PUT", "/api/admin/shelf/shipments/unused/chunks/unused"),
        ("POST", "/api/admin/shelf/shipments/unused/commit"),
        ("PUT", "/api/admin/devices/unused/shelf"),
        ("PUT", "/api/admin/settings/server-name"),
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
    let token = pair_admin(
        &ClientCache::default(),
        &remote,
        &admin_secret(&paths),
        "test desktop",
    )
    .await
    .expect("pair admin");

    // 無認証で答えるのは hello と pair だけ。
    assert_eq!(
        client
            .get(format!("{url}/api/hello"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    for path in [
        "about",
        "admin/status",
        "admin/devices",
        "admin/shelf/reading",
        "admin/shelf/favorites",
        "admin/shelf/shipments",
    ] {
        let response = client
            .get(format!("{url}/api/{path}"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
        let wrong = client
            .get(format!("{url}/api/{path}"))
            .bearer_auth("wrong")
            .send()
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
    // 更新を頼めるのは管理者だけ。手で置いたサーバーは自分では更新できない。
    let update_url = format!("{url}/api/admin/update");
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
        .get(format!("{reader_url}/api/admin/status"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(split.status(), StatusCode::NOT_FOUND);
    let body: serde_json::Value = split.json().await.unwrap();
    assert_eq!(body["code"], "not_found");
    assert_eq!(
        client
            .get(format!("{reader_url}/api/hello"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    // 要求の大きさの上限。
    let huge = "x".repeat(70 * 1024);
    let pair = client
        .post(format!("{url}/api/pair"))
        .json(&serde_json::json!({ "secret": huge, "deviceName": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(pair.status(), StatusCode::PAYLOAD_TOO_LARGE);

    // 経路が付けた利用者名は、認証にも記録にも使わない。
    let forged = client
        .get(format!("{url}/api/about"))
        .header("Tailscale-User-Login", "someone@example.test")
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), StatusCode::UNAUTHORIZED);
    let listed = client
        .get(format!("{url}/api/admin/devices"))
        .bearer_auth(&token)
        .header("Tailscale-User-Login", "someone@example.test")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!listed.contains("someone@example.test"));

    // 読み手の鍵では、棚の送信も端末の管理もできない。
    let secret = ServerStore::open(&paths.store())
        .unwrap()
        .issue_pairing(Role::Reader, Duration::from_secs(60))
        .unwrap();
    let reader = pair_with(&client, &url, &secret).await;
    let reader_token = reader["token"].as_str().unwrap();
    for (method, path) in [
        (reqwest::Method::GET, "admin/shelf/shipments"),
        (reqwest::Method::GET, "admin/devices"),
    ] {
        let response = client
            .request(method, format!("{url}/api/{path}"))
            .bearer_auth(reader_token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
    }

    // 登録の失敗は 1 分に 10 回まで。止めた試行では鍵を使い切らない。
    let secret = ServerStore::open(&paths.store())
        .unwrap()
        .issue_pairing(Role::Reader, Duration::from_secs(60))
        .unwrap();
    for _ in 0..10 {
        let failure = client
            .post(format!("{url}/api/pair"))
            .json(&serde_json::json!({ "secret": "wrong", "deviceName": "x" }))
            .send()
            .await
            .unwrap();
        assert_eq!(failure.status(), StatusCode::UNAUTHORIZED);
    }
    let limited = client
        .post(format!("{url}/api/pair"))
        .json(&serde_json::json!({ "secret": secret, "deviceName": "x" }))
        .send()
        .await
        .unwrap();
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    let seconds: u64 = limited.headers()[header::RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&seconds));
    assert!(ServerStore::open(&paths.store())
        .unwrap()
        .consume_pairing(&secret, "x")
        .is_ok());
}

#[tokio::test]
async fn successful_pairings_do_not_count_toward_the_failure_limit() {
    let root = TestRoot::created("pair-failures");
    let store = Arc::new(ServerStore::open(&root.0.join("state.sqlite")).unwrap());
    let ctx = super::http::ServerContext::new(Arc::clone(&store), root.0.clone(), None, None);
    let url = spawn_plain(combined_router(ctx));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let attempt = |secret: &str| {
        client
            .post(format!("{url}/api/pair"))
            .json(&serde_json::json!({ "secret": secret, "deviceName": "試験端末" }))
            .send()
    };
    for _ in 0..9 {
        assert_eq!(
            attempt("wrong").await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    for _ in 0..2 {
        let secret = store
            .issue_pairing(Role::Reader, Duration::from_secs(60))
            .unwrap();
        pair_with(&client, &url, &secret).await;
    }
    assert_eq!(
        attempt("wrong").await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let secret = store
        .issue_pairing(Role::Reader, Duration::from_secs(60))
        .unwrap();
    let limited = attempt(&secret).await.unwrap();
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.headers().contains_key(header::RETRY_AFTER));
    assert!(store.consume_pairing(&secret, "試験端末").is_ok());
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

#[tokio::test]
async fn non_bearer_authorization_is_subject_to_browser_checks_and_cannot_authenticate() {
    let site = AdminSite::new("admin-auth-scheme");
    let cookie = site.set_up().await;
    let status = || {
        site.get("/api/admin/status")
            .header(header::AUTHORIZATION, "Basic x")
            .header(header::COOKIE, &cookie)
    };
    let host = status()
        .header(header::HOST, "unknown.example")
        .send()
        .await
        .unwrap();
    assert_eq!(host.status(), StatusCode::FORBIDDEN);
    let origin = site
        .client
        .post(format!("{}/api/admin/tickets", site.url))
        .header(header::ORIGIN, "https://other.example")
        .header(header::AUTHORIZATION, "Basic x")
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(origin.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        origin.json::<serde_json::Value>().await.unwrap()["code"],
        "cross_site"
    );
    assert_eq!(
        status().send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn session_cookie_is_only_accepted_where_the_admin_page_is_served() {
    let site = AdminSite::new("admin-cookie-port");
    let cookie = site.set_up().await;
    let public = spawn_plain(combined_router(Arc::clone(&site.parts.context)));
    for (url, expected) in [
        (&site.url, StatusCode::OK),
        (&public, StatusCode::UNAUTHORIZED),
    ] {
        let response = site
            .client
            .get(format!("{url}/api/admin/status"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
}

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
            .post("/api/admin/web/setup")
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
        self.post("/api/admin/web/login")
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
        site.post("/api/admin/web/setup")
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
        .post("/api/admin/web/setup")
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
        .post("/api/admin/web/setup")
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
        .get("/api/admin/status")
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    assert_eq!(
        site.get("/api/admin/status").send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );

    let logout = site
        .post("/api/admin/web/logout")
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
        .get("/api/admin/status")
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
        .post("/api/admin/web/ticket")
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
    let url = format!("{}/api/admin/web/logout", site.url);
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
        .get("/api/admin/status")
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
    site.post("/api/admin/web/ticket")
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
        &ClientCache::default(),
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
        .get(format!("{}/api/admin/status", site.url))
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
        &ClientCache::default(),
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
        .post(format!("{public}/api/admin/tickets"))
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
        .post(format!("{}/api/admin/tickets", site.url))
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
        &ClientCache::default(),
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
        .get("/api/admin/web/state")
        .header(header::HOST, "piep.example.net")
        .send()
        .await
        .unwrap();
    assert_eq!(api.status(), StatusCode::FORBIDDEN);
    let error: serde_json::Value = api.json().await.unwrap();
    assert_eq!(error["code"], "host_rejected");

    let issued = site
        .client
        .post(format!("{}/api/admin/tickets", site.url))
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
        .post("/api/admin/web/ticket")
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
        &ClientCache::default(),
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
            .post(format!("{}/api/admin/tickets", site.url))
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
        .post("/api/admin/web/ticket")
        .json(&serde_json::json!({ "ticket": ticket }))
        .send()
        .await
        .unwrap();
    assert_eq!(entered.status(), StatusCode::OK);
    let cookie = cookie_of(&entered);
    let again = site
        .post("/api/admin/web/ticket")
        .json(&serde_json::json!({ "ticket": ticket }))
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::UNAUTHORIZED);

    // ブラウザからは券を作れない。
    let by_cookie = site
        .post("/api/admin/tickets")
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(by_cookie.status(), StatusCode::FORBIDDEN);

    // 券で入っただけでは、パスワードの変更を聞き直す。
    let change = |password: &'static str| {
        site.client
            .put(format!("{}/api/admin/password", site.url))
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
        .post("/api/admin/web/reauth")
        .header(header::COOKIE, &cookie)
        .json(&serde_json::json!({ "password": "違うパスワードをここに入れる" }))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    let right = site
        .post("/api/admin/web/reauth")
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
            .post("/api/admin/web/setup")
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
        .get("/api/admin/audit")
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
        "/api/admin/status",
        "/api/admin/web/state",
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
        .get(format!("{public}/api/admin/status"))
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
        .get("/api/admin/status")
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
        .get("/api/hello")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(hello.get("version").is_none());
    assert_eq!(
        site.get("/api/about").send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let token = pair_admin(
        &ClientCache::default(),
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
        .get("/api/about")
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
    let plain = client.get(format!("{url}/api/hello")).send().await.unwrap();
    assert_eq!(plain.status(), StatusCode::OK);
    assert!(plain.headers().get("content-encoding").is_none());
    let packed = client
        .get(format!("{url}/api/hello"))
        .header("accept-encoding", "gzip")
        .send()
        .await
        .unwrap();
    assert_eq!(packed.status(), StatusCode::OK);
    assert_eq!(packed.headers()["content-encoding"], "gzip");
    // 圧縮しても、本文を保存させない指示は変わらない。
    assert_eq!(packed.headers()["cache-control"], "private, no-store");
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
        ("POST", "/api/admin/shelf/shipments/s/needed", None, 401),
        ("PUT", "/api/admin/shelf/shipments/s/chunks/c", None, 401),
        (
            "PUT",
            "/api/admin/shelf/shipments/s/chunks/c",
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
        assert!(pair_admin(
            &ClientCache::default(),
            &remote,
            "one-time-secret",
            "desktop"
        )
        .await
        .is_err());
        let client = ClientCache::default().client(&remote).await.unwrap();
        let response = client
            .put(format!(
                "{}/api/admin/shelf/shipments/s/chunks/c",
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
