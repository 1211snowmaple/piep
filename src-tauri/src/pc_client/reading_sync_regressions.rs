use super::*;
use std::io::{Read, Write};

struct TestCipher;
impl crate::secret_cipher::SecretCipher for TestCipher {
    fn seal(&self, value: &[u8]) -> Result<Vec<u8>, ()> {
        Ok(value.to_vec())
    }
    fn open(&self, value: &[u8]) -> Result<Vec<u8>, ()> {
        Ok(value.to_vec())
    }
}

fn saved(url: &str, device: &str) -> SavedConnection {
    SavedConnection {
        url: url.into(),
        fingerprint: None,
        certificate_der: None,
        token: "test".into(),
        device_id: device.into(),
        device_name: "test".into(),
        capabilities: vec!["reading-sync".into()],
    }
}

fn write(page: i64, time: i64) -> ReadingWrite {
    ReadingWrite {
        source: "pixiv".into(),
        source_id: "17".into(),
        version: Some(1),
        edit_hash: None,
        page,
        position: 0.5,
        finished: false,
        device_time: time,
    }
}

fn position(id: &str, time: i64) -> ReadingPosition {
    ReadingPosition {
        source: "pixiv".into(),
        source_id: id.into(),
        version: Some(1),
        edit_hash: None,
        page: 5,
        position: 0.5,
        finished: false,
        device_id: "other".into(),
        device_time: time,
        cursor: 0,
    }
}

pub(super) fn fixture() -> (PathBuf, Arc<ReadingSyncState>, Arc<PcClientState>) {
    let root =
        std::env::temp_dir().join(format!("piep-reading-regressions-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let state = Arc::new(ReadingSyncState::new(root.join("reading-sync.json")));
    let pc = Arc::new(PcClientState::new(
        root.join("connection.json"),
        Arc::new(TestCipher),
    ));
    (root, state, pc)
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
}

fn respond(stream: &mut std::net::TcpStream, body: serde_json::Value) {
    let body = serde_json::to_vec(&body).unwrap();
    write!(stream, "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", body.len()).unwrap();
    stream.write_all(&body).unwrap();
}

#[test]
fn old_http_responses_cannot_delete_current_pending_even_after_connection_aba() {
    tauri::async_runtime::block_on(async {
        for mode in 0..4 {
            let (root, state, pc) = fixture();
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let old = saved(&format!("http://{}", listener.local_addr().unwrap()), "old");
            pc.save(&old).unwrap();
            let target = sync_target(&pc).unwrap().unwrap();
            let pushing = mode % 2 == 1;
            if pushing {
                state.record_current(&pc, &target, write(5, 100)).unwrap();
            }
            let (sent, received) = std::sync::mpsc::channel();
            let (release, resume) = std::sync::mpsc::channel();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = [0; 4096];
                let count = stream.read(&mut bytes).unwrap();
                assert!(
                    String::from_utf8_lossy(&bytes[..count]).starts_with(if pushing {
                        "PUT /api/v1/reading/"
                    } else {
                        "GET /api/v1/reading?"
                    })
                );
                sent.send(()).unwrap();
                resume.recv_timeout(Duration::from_secs(5)).unwrap();
                respond(
                    &mut stream,
                    if pushing {
                        serde_json::to_value(position("17", 100)).unwrap()
                    } else {
                        serde_json::json!({"items":[], "cursor":0})
                    },
                );
            });
            let running = {
                let (state, pc) = (state.clone(), pc.clone());
                tauri::async_runtime::spawn(async move {
                    if pushing {
                        state.push(&http_client(), &pc, &target).await
                    } else {
                        state.pull(&http_client(), &pc, &target).await
                    }
                })
            };
            received.recv_timeout(Duration::from_secs(5)).unwrap();
            pc.save(&saved("https://new.example", "new")).unwrap();
            if mode >= 2 {
                pc.save(&old).unwrap();
            } // A -> B -> A still has a newer generation.
            let current = sync_target(&pc).unwrap().unwrap();
            state.record_current(&pc, &current, write(9, 200)).unwrap();
            release.send(()).unwrap();
            assert_eq!(
                running.await.unwrap().unwrap_err().code,
                "connection_changed"
            );
            server.join().unwrap();
            let pending: ReadingWrite = state
                .store_current(&pc, &current, |tx| {
                    Ok(query_entry(tx, "pending", "pixiv:17")?.unwrap())
                })
                .unwrap();
            assert_eq!(pending, write(9, 200));
            drop((state, pc));
            fs::remove_dir_all(root).unwrap();
        }
    });
}

#[test]
fn a_queued_sync_chooses_its_connection_after_acquiring_the_lock() {
    use std::future::Future;
    use std::task::Poll;
    tauri::async_runtime::block_on(async {
        let (root, state, pc) = fixture();
        pc.save(&saved("http://127.0.0.1:9", "old")).unwrap();
        let held = state.sync_lock.lock().await;
        let mut queued = Box::pin(state.sync(&pc));
        let poll = std::future::poll_fn(|cx| Poll::Ready(queued.as_mut().poll(cx))).await;
        assert!(poll.is_pending());
        let mut new = saved("https://new.example", "new");
        new.capabilities.clear();
        pc.save(&new).unwrap();
        drop(held);
        queued.await.unwrap(); // Capturing old before the lock would attempt HTTP and fail.
        drop((state, pc));
        fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn large_history_lookup_and_empty_http_sync_write_nothing_and_updates_touch_only_rows() {
    tauri::async_runtime::block_on(async {
        let (root, state, pc) = fixture();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let connection = saved(
            &format!("http://{}", listener.local_addr().unwrap()),
            "device",
        );
        pc.save(&connection).unwrap();
        let mut legacy = SyncFile {
            url: connection.url.clone(),
            device_id: connection.device_id.clone(),
            ..SyncFile::default()
        };
        for n in 0..10_000 {
            legacy
                .positions
                .insert(format!("pixiv:{n}"), position(&n.to_string(), 100));
        }
        let original = serde_json::to_vec(&legacy).unwrap();
        fs::write(&state.file, &original).unwrap();
        let target = sync_target(&pc).unwrap().unwrap();
        state.store_current(&pc, &target, |_| Ok(())).unwrap();
        let initial_changes = {
            let db = state.database.lock().unwrap();
            let db = db.as_ref().unwrap();
            db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
            db.total_changes()
        };
        let server = std::thread::spawn(move || {
            for step in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = [0; 4096];
                let count = stream.read(&mut bytes).unwrap();
                let request = String::from_utf8_lossy(&bytes[..count]);
                if step == 1 {
                    assert!(request.starts_with("PUT "));
                } else {
                    assert!(request.starts_with("GET "));
                }
                respond(
                    &mut stream,
                    match step {
                        0 => serde_json::json!({"items":[], "cursor":0}),
                        1 => serde_json::to_value(position("17", 200)).unwrap(),
                        _ => serde_json::json!({"items":[position("17", 200)], "cursor":1}),
                    },
                );
            }
        });
        for _ in 0..100 {
            let found: ReadingPosition = state
                .store_current(&pc, &target, |tx| {
                    Ok(query_entry(tx, "positions", "pixiv:17")?.unwrap())
                })
                .unwrap();
            assert_eq!(found.page, 5);
        }
        state.push(&http_client(), &pc, &target).await.unwrap();
        state.pull(&http_client(), &pc, &target).await.unwrap();
        assert_eq!(
            state
                .database
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .total_changes(),
            initial_changes
        );
        let wal = state.file.with_extension("sqlite3-wal");
        assert_eq!(fs::metadata(&wal).unwrap().len(), 0);
        state.record_current(&pc, &target, write(7, 200)).unwrap();
        state.record_current(&pc, &target, write(7, 200)).unwrap();
        state.push(&http_client(), &pc, &target).await.unwrap();
        state.pull(&http_client(), &pc, &target).await.unwrap();
        server.join().unwrap();
        assert_eq!(
            state
                .database
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .total_changes()
                - initial_changes,
            4
        );
        assert!(
            fs::metadata(&wal).unwrap().len() <= 128 * 1024,
            "one record/push/pull must not write the 10,000-row history"
        );
        assert_eq!(fs::read(&state.file).unwrap(), original);
        let count: i64 = state
            .store_current(&pc, &target, |tx| {
                tx.query_row("SELECT count(*) FROM positions", [], |row| row.get(0))
                    .map_err(storage_error)
            })
            .unwrap();
        assert_eq!(count, 10_000);
        drop((state, pc));
        fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn migration_and_failed_transactions_preserve_the_previous_pending() {
    let (root, state, pc) = fixture();
    let connection = saved("https://server.example", "device");
    pc.save(&connection).unwrap();
    let mut legacy = SyncFile {
        url: connection.url.clone(),
        device_id: connection.device_id.clone(),
        ..SyncFile::default()
    };
    legacy.pending.insert("pixiv:17".into(), write(5, 100));
    let original = serde_json::to_vec(&legacy).unwrap();
    fs::write(&state.file, &original).unwrap();
    let target = sync_target(&pc).unwrap().unwrap();
    let pending: ReadingWrite = state
        .store_current(&pc, &target, |tx| {
            Ok(query_entry(tx, "pending", "pixiv:17")?.unwrap())
        })
        .unwrap();
    assert_eq!(pending, write(5, 100));
    // Abort after the new row was written but before commit, as on an interrupted transaction.
    assert!(
        state
            .store_current(&pc, &target, |tx| {
                put_entry(tx, "pending", "pixiv:17", &write(9, 200))?;
                Err::<(), _>(ClientError::storage())
            })
            .is_err()
    );
    let file = state.file.clone();
    drop(state);
    let reopened = ReadingSyncState::new(file);
    let pending: ReadingWrite = reopened
        .store_current(&pc, &target, |tx| {
            Ok(query_entry(tx, "pending", "pixiv:17")?.unwrap())
        })
        .unwrap();
    assert_eq!(pending, write(5, 100));
    assert_eq!(fs::read(&reopened.file).unwrap(), original);
    drop((reopened, pc));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn migration_failure_rolls_back_every_imported_row_and_can_be_retried() {
    let (root, state, pc) = fixture();
    let connection = saved("https://server.example", "device");
    pc.save(&connection).unwrap();
    let mut legacy = SyncFile {
        url: connection.url.clone(),
        device_id: connection.device_id.clone(),
        ..SyncFile::default()
    };
    legacy.pending.insert("pixiv:17".into(), write(5, 100));
    legacy.pending.insert(
        "pixiv:z-fail".into(),
        ReadingWrite {
            source_id: "z-fail".into(),
            ..write(9, 200)
        },
    );
    let original = serde_json::to_vec(&legacy).unwrap();
    fs::write(&state.file, &original).unwrap();
    let db_path = state.file.with_extension("sqlite3");
    {
        let db = rusqlite::Connection::open(&db_path).unwrap();
        db.execute_batch(SYNC_SCHEMA).unwrap();
        db.execute_batch("CREATE TRIGGER fail_migration BEFORE INSERT ON pending WHEN NEW.key='pixiv:z-fail' BEGIN SELECT RAISE(ABORT, 'simulated write failure'); END;").unwrap();
    }
    let target = sync_target(&pc).unwrap().unwrap();
    assert!(state.store_current(&pc, &target, |_| Ok(())).is_err());
    assert_eq!(fs::read(&state.file).unwrap(), original);
    {
        let db = rusqlite::Connection::open(&db_path).unwrap();
        let imported: i64 = db
            .query_row(
                "SELECT (SELECT count(*) FROM sync_meta) + (SELECT count(*) FROM pending)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            imported, 0,
            "a failed import must not leave its initialization marker or partial pending"
        );
        db.execute("DROP TRIGGER fail_migration", []).unwrap();
    }
    let count: i64 = state
        .store_current(&pc, &target, |tx| {
            tx.query_row("SELECT count(*) FROM pending", [], |row| row.get(0))
                .map_err(storage_error)
        })
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(fs::read(&state.file).unwrap(), original);
    drop((state, pc));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_new_position_in_the_same_millisecond_survives_the_previous_delivery() {
    let (root, state, pc) = fixture();
    pc.save(&saved("https://server.example", "device")).unwrap();
    let target = sync_target(&pc).unwrap().unwrap();
    let old = write(5, 100);
    state.record_current(&pc, &target, old.clone()).unwrap();
    state.record_current(&pc, &target, write(9, 100)).unwrap();
    state
        .store_current(&pc, &target, |tx| delivered_write(tx, "pixiv:17", &old))
        .unwrap();
    let pending: ReadingWrite = state
        .store_current(&pc, &target, |tx| {
            Ok(query_entry(tx, "pending", "pixiv:17")?.unwrap())
        })
        .unwrap();
    assert_eq!(pending, write(9, 100));
    drop((state, pc));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn corrupt_storage_is_reported_without_replacing_json_or_the_database() {
    for corrupt_database in [false, true] {
        let (root, state, pc) = fixture();
        pc.save(&saved("https://server.example", "device")).unwrap();
        let target = sync_target(&pc).unwrap().unwrap();
        let original = if corrupt_database {
            let mut legacy = SyncFile {
                url: target.saved.url.clone(),
                device_id: target.saved.device_id.clone(),
                ..SyncFile::default()
            };
            legacy.pending.insert("pixiv:17".into(), write(5, 100));
            serde_json::to_vec(&legacy).unwrap()
        } else {
            b"{ broken JSON with unsent data".to_vec()
        };
        fs::write(&state.file, &original).unwrap();
        if corrupt_database {
            fs::write(
                state.file.with_extension("sqlite3"),
                b"broken database with pending",
            )
            .unwrap();
        }
        assert_eq!(
            state
                .record_current(&pc, &target, write(9, 200))
                .unwrap_err()
                .code,
            "storage"
        );
        assert_eq!(fs::read(&state.file).unwrap(), original);
        if corrupt_database {
            assert_eq!(
                fs::read(state.file.with_extension("sqlite3")).unwrap(),
                b"broken database with pending"
            );
        }
        drop((state, pc));
        fs::remove_dir_all(root).unwrap();
    }
}
