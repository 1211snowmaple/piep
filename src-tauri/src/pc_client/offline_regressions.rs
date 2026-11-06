use super::*;
use std::io::{Read, Write};

fn metadata_json() -> serde_json::Value {
    serde_json::json!({
        "work": {"id": 3, "source": "pixiv", "sourceId": "3", "title": "Dummy",
            "authorName": "Dummy", "contentType": "novel", "tags": [], "excerpt": null,
            "textLength": 2, "assetCount": 0, "hasCover": false, "currentVersion": 1,
            "contentHash": "source-v1", "favorite": false, "seriesTitle": null, "seriesId": null},
        "versions": [], "isEdited": true, "activeEditId": 1, "activeEditHash": "edit-A"
    })
}

fn server(
    revision: serde_json::Value,
    expected_requests: usize,
) -> (SavedConnection, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        for _ in 0..expected_requests {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = [0; 4096];
            let length = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..length]);
            let body = if request.starts_with("GET /api/v1/works/3/pages/") {
                let index = usize::from(request.starts_with("GET /api/v1/works/3/pages/1 "));
                serde_json::json!({"page": index, "pageCount": 2,
                    "html": format!("<p>page-{index}</p>"), "totalPlainTextChars": 2, "sourcePageStarts": [0],
                    "revision": if index == 0 { serde_json::json!({"currentVersion": 1, "contentHash": "source-v1", "activeEditHash": "edit-A"}) } else { revision.clone() }})
            } else if request.starts_with("GET /api/v1/works/3/outline ")
                || request.starts_with("GET /api/v1/works/3/collections ")
            {
                serde_json::json!([])
            } else {
                assert!(request.starts_with("GET /api/v1/works/3 "), "{request}");
                // The latest metadata is A again, even if page 1 was B.
                metadata_json()
            };
            let body = serde_json::to_vec(&body).unwrap();
            write!(stream, "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", body.len()).unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    (
        SavedConnection {
            url: format!("http://{address}"),
            fingerprint: None,
            certificate_der: None,
            token: "dummy".into(),
            device_id: "dummy".into(),
            device_name: "dummy".into(),
            capabilities: vec![],
        },
        handle,
    )
}

#[test]
fn every_page_must_match_metadata_and_failed_updates_keep_readable_old_pages() {
    tauri::async_runtime::block_on(async {
        let root = std::env::temp_dir().join(format!("piep-offline-revisions-{}", Uuid::new_v4()));
        let state = OfflineState::new(root.clone());
        let metadata: MobileMetadata = serde_json::from_value(metadata_json()).unwrap();
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let good = serde_json::json!({"currentVersion": 1, "contentHash": "source-v1", "activeEditHash": "edit-A"});
        let (saved, handle) = server(good.clone(), 5);
        save_snapshot(&state, &client, &saved, &metadata)
            .await
            .unwrap();
        handle.join().unwrap();
        let original = read_manifest(&state, 3).unwrap().1;
        for bad in [
            serde_json::json!({"currentVersion": 1, "contentHash": "source-v1", "activeEditHash": "edit-B"}),
            serde_json::json!({"currentVersion": 2, "contentHash": "source-v1", "activeEditHash": "edit-A"}),
            serde_json::json!({"currentVersion": 1, "contentHash": "source-v2", "activeEditHash": "edit-A"}),
            serde_json::Value::Null,
        ] {
            let (saved, handle) = server(bad, 2);
            assert!(
                save_snapshot(&state, &client, &saved, &metadata)
                    .await
                    .is_err()
            );
            handle.join().unwrap();
            assert_eq!(read_manifest(&state, 3).unwrap().1, original);
            assert!(
                read_offline_page(&state, 3, 0)
                    .unwrap()
                    .page
                    .html
                    .contains("page-0")
            );
            assert!(
                read_offline_page(&state, 3, 1)
                    .unwrap()
                    .page
                    .html
                    .contains("page-1")
            );
            assert_eq!(fs::read_dir(state.work_dir(3).unwrap()).unwrap().count(), 2);
        }
        fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn reading_during_a_save_and_aborting_it_keep_the_published_snapshot() {
    tauri::async_runtime::block_on(async {
        let root = std::env::temp_dir().join(format!("piep-offline-abort-{}", Uuid::new_v4()));
        let state = Arc::new(OfflineState::new(root.clone()));
        let metadata: MobileMetadata = serde_json::from_value(metadata_json()).unwrap();
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let good = serde_json::json!({"currentVersion": 1, "contentHash": "source-v1", "activeEditHash": "edit-A"});
        let (saved, handle) = server(good, 5);
        save_snapshot(&state, &client, &saved, &metadata)
            .await
            .unwrap();
        handle.join().unwrap();
        let original = read_manifest(&state, 3).unwrap().1;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (sent, received) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let blocked = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = [0; 4096];
            let count = stream.read(&mut request).unwrap();
            assert!(
                String::from_utf8_lossy(&request[..count])
                    .starts_with("GET /api/v1/works/3/pages/0 ")
            );
            sent.send(()).unwrap();
            resume.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        let saving = {
            let state = state.clone();
            tauri::async_runtime::spawn(async move {
                let saved = SavedConnection { url, ..saved };
                save_snapshot(&state, &client, &saved, &metadata).await
            })
        };
        received.recv_timeout(Duration::from_secs(5)).unwrap();
        {
            let _read = state.lock.lock().await;
            assert_eq!(read_manifest(&state, 3).unwrap().1, original);
            assert!(
                read_offline_page(&state, 3, 1)
                    .unwrap()
                    .page
                    .html
                    .contains("page-1")
            );
        }
        saving.abort();
        assert!(saving.await.is_err());
        release.send(()).unwrap();
        blocked.join().unwrap();
        assert_eq!(read_manifest(&state, 3).unwrap().1, original);
        drop(state);
        let reopened = OfflineState::new(root.clone());
        assert_eq!(read_manifest(&reopened, 3).unwrap().1, original);
        assert!(
            read_offline_page(&reopened, 3, 1)
                .unwrap()
                .page
                .html
                .contains("page-1")
        );
        assert_eq!(
            fs::read_dir(reopened.work_dir(3).unwrap()).unwrap().count(),
            2
        );
        fs::remove_dir_all(root).unwrap();
    });
}
