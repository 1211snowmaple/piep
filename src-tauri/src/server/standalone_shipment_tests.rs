use super::*;
use crate::library::{ReadingAnchor, ReadingLocation, ReadingUpdate, WorkSave};
use crate::server::store::ServerStore;
use crate::server::tests::TestRoot;

fn save(library: &Library, key: &str, text: &str) -> crate::library::CaptureResult {
    library
        .save_readable_work(
            WorkSave {
                request: LibraryId::now_v7(),
                provider: "pixiv",
                key,
                route: "test",
                data: &serde_json::json!({"detail":{"title":format!("題 {key}")},"text":text}),
                assets: vec![],
            },
            None,
        )
        .unwrap()
}

/// デスクトップ版の代わりに、一つの写しを送り切る。送った実体（台帳の部分も含む）の数を返す。
fn ship(shipments: &ShelfShipments, desktop: &Library, scratch: &Path) -> usize {
    let name = uuid::Uuid::now_v7();
    let database = scratch.join(format!("{name}.db"));
    let chunks = desktop.ship_snapshot(&database).unwrap();
    let parts = ObjectStore::open(&scratch.join(format!("{name}-parts"))).unwrap();
    let file = parts.put_path(&database).unwrap();
    let opened = shipments.open(desktop.id).unwrap();
    let wires: Vec<WireChunk> = chunks
        .iter()
        .chain(&file.chunks)
        .map(|chunk| WireChunk {
            id: chunk.id.to_string(),
            length: chunk.length,
        })
        .collect();
    let needed = shipments.needed(&opened.shipment_id, &wires).unwrap();
    // 台帳の部分が届く前は確定できない。
    if file
        .chunks
        .iter()
        .any(|chunk| needed.contains(&chunk.id.to_string()))
    {
        assert!(matches!(
            shipments.commit(&opened.shipment_id, &file),
            Err(ShipError::Invalid(_))
        ));
    }
    let mut sent = std::collections::HashSet::new();
    for wire in wires.iter().filter(|wire| needed.contains(&wire.id)) {
        if !sent.insert(wire.id.clone()) {
            continue;
        }
        let chunk = wire.parse().unwrap();
        let mut encoded = Vec::new();
        if parts.has_object(chunk.id).unwrap() {
            parts.export_chunk(&chunk, &mut encoded).unwrap();
        } else {
            desktop.objects.export_chunk(&chunk, &mut encoded).unwrap();
        }
        shipments
            .put_chunk(&opened.shipment_id, wire, &encoded)
            .unwrap();
    }
    // 全体のハッシュが合わない並びは断る。
    let mut wrong = file.clone();
    wrong.id = chunks.first().map_or(file.chunks[0].id, |chunk| chunk.id);
    assert!(matches!(
        shipments.commit(&opened.shipment_id, &wrong),
        Err(ShipError::Invalid(_))
    ));
    assert_eq!(
        shipments.commit(&opened.shipment_id, &file).unwrap().state,
        "assembling"
    );
    for _ in 0..600 {
        let status = shipments.status();
        if status.state != "assembling" {
            assert_eq!(status.state, "ready", "{status:?}");
            assert_eq!(status.restarting, Some(false));
            return sent.len();
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("棚を組み終えません");
}

fn paths_with_config(root: &Path) -> ServerPaths {
    let paths = ServerPaths::new(&root.join("server")).unwrap();
    fs::create_dir_all(paths.state()).unwrap();
    fs::write(paths.config(), "# 設定を保つ\n[update]\nauto = false\n").unwrap();
    paths
}

#[test]
fn a_shipped_shelf_is_served_after_restart_and_keeps_what_the_phone_wrote_on_the_old_copy() {
    let temp = TestRoot::created("v2-shipment");
    let paths = paths_with_config(&temp.0);
    let desktop = Library::create(&temp.0.join("desktop")).unwrap();
    let paged = save(&desktop, "1", "一[newpage]二");
    save(&desktop, "2", "本文");

    // 初回：配っている棚が無いので全部の実体を送る。起動のときに配る棚になる。
    let first = ShelfShipments::new(paths.clone(), None);
    let sent = ship(&first, &desktop, &temp.0);
    assert!(sent > 0);
    apply_pending(&paths);
    let config = load_config(&paths).unwrap().shelf.unwrap();
    assert_eq!(config.library_id.get(), desktop.id);
    let first_generation = config.generation.unwrap();
    assert!(fs::read_to_string(paths.config())
        .unwrap()
        .contains("# 設定を保つ"));
    let store = Arc::new(ServerStore::open(&paths.store()).unwrap());
    let served = config.open(&paths, Arc::clone(&store)).unwrap();
    assert_eq!(
        serde_json::to_value(served.shelf_status().unwrap()).unwrap()["works"],
        "2"
    );

    // 二回目：足した作品の実体だけを送る。送ったあと、携帯が配っている棚に書いた。
    save(&desktop, "3", "三つめ");
    let second = ShelfShipments::new(paths.clone(), Some(Arc::clone(served.library())));
    let sent_again = ship(&second, &desktop, &temp.0);
    assert!(sent_again > 0 && sent_again < sent + 3, "{sent_again}");
    let phone = LibraryId::now_v7();
    let at = ReadingLocation::Source {
        snapshot: paged.snapshot_id,
        anchor: ReadingAnchor::Page {
            page: 1,
            ratio_ppm: 0,
        },
    };
    served
        .library()
        .set_reading_position(ReadingUpdate {
            request: LibraryId::now_v7(),
            work: paged.entity_id,
            user: desktop.id,
            device: phone,
            expected: 0,
            location: Some(at.clone()),
            finished: false,
        })
        .unwrap();
    drop(second);
    drop(served);
    apply_pending(&paths);
    let config = load_config(&paths).unwrap().shelf.unwrap();
    assert_ne!(config.generation, Some(first_generation));
    let served = config.open(&paths, Arc::clone(&store)).unwrap();
    assert_eq!(
        serde_json::to_value(served.shelf_status().unwrap()).unwrap()["works"],
        "3"
    );
    let moved = served
        .library()
        .reading_position(paged.entity_id, desktop.id)
        .unwrap();
    assert_eq!(moved.location, Some(at));
    assert_eq!(moved.device, Some(phone));
    // 前の世代は残る（戻せる）。予約は一度きり。
    assert_eq!(fs::read_dir(generations(&paths)).unwrap().count(), 2);
    assert!(!paths.root.join(PENDING).exists());
    assert!(!incoming(&paths).exists());
    drop(served);

    // 三回目のあと、二つ前の世代は消える。
    let served = config.open(&paths, Arc::clone(&store)).unwrap();
    let third = ShelfShipments::new(paths.clone(), Some(Arc::clone(served.library())));
    ship(&third, &desktop, &temp.0);
    drop(third);
    drop(served);
    apply_pending(&paths);
    let names: Vec<String> = fs::read_dir(generations(&paths))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 2);
    assert!(!names.contains(&first_generation.get().to_string()));
    // 台帳の部分は、最後に組んだ台帳のもの（小さな台帳なので一つ）だけが残る。
    let parts = ObjectStore::open(&paths.root.join(DATABASE_PARTS)).unwrap();
    assert_eq!(parts.listing().unwrap().len(), 1);
}

#[test]
fn a_shelf_holding_an_empty_file_ships() {
    let temp = TestRoot::created("v2-shipment-empty");
    let paths = paths_with_config(&temp.0);
    let desktop = Library::create(&temp.0.join("desktop")).unwrap();
    let staged = temp.0.join("staged");
    fs::create_dir_all(&staged).unwrap();
    let empty = staged.join("empty.txt");
    fs::write(&empty, b"").unwrap();
    desktop
        .save_readable_work(
            WorkSave {
                request: LibraryId::now_v7(),
                provider: "pixiv",
                key: "1",
                route: "test",
                data: &serde_json::json!({"detail":{"title":"題"},"text":"本文"}),
                assets: vec![crate::library::StagedAsset {
                    role: "file".into(),
                    name: "empty.txt".into(),
                    path: empty,
                    url: None,
                    mime_type: Some("text/plain".into()),
                }],
            },
            None,
        )
        .unwrap();
    // 実際の棚にも空のファイルがあり、長さ 0 の実体を断って送れなかった。
    let snapshot = temp.0.join("check.db");
    let chunks = desktop.ship_snapshot(&snapshot).unwrap();
    assert!(chunks.iter().any(|chunk| chunk.length == 0));
    let shipments = ShelfShipments::new(paths.clone(), None);
    ship(&shipments, &desktop, &temp.0);
    apply_pending(&paths);
    let config = load_config(&paths).unwrap().shelf.unwrap();
    let store = Arc::new(ServerStore::open(&paths.store()).unwrap());
    let served = config.open(&paths, store).unwrap();
    assert_eq!(
        serde_json::to_value(served.shelf_status().unwrap()).unwrap()["works"],
        "1"
    );
}

#[test]
fn a_shipment_from_another_shelf_is_refused_and_an_interrupted_one_resumes() {
    let temp = TestRoot::created("v2-shipment-refuse");
    let paths = paths_with_config(&temp.0);
    let desktop = Library::create(&temp.0.join("desktop")).unwrap();
    save(&desktop, "1", "本文");
    let shipments = ShelfShipments::new(paths.clone(), None);
    ship(&shipments, &desktop, &temp.0);
    apply_pending(&paths);

    let other = Library::create(&temp.0.join("other")).unwrap();
    let shipments = ShelfShipments::new(paths.clone(), None);
    assert!(matches!(
        shipments.open(other.id),
        Err(ShipError::Conflict(_))
    ));
    // 途中で切れた受け取りは、同じ棚なら続きから（受け取った実体は残る）。
    let opened = shipments.open(desktop.id).unwrap();
    let again = ShelfShipments::new(paths.clone(), None);
    let resumed = again.open(desktop.id).unwrap();
    assert!(resumed.resumed);
    assert_eq!(resumed.shipment_id, opened.shipment_id);
    // 知らない受け取り・崩れた識別子は断る。
    assert!(matches!(
        again.needed("nope", &[]),
        Err(ShipError::NotFound)
    ));
    assert!(matches!(
        again.needed(
            &resumed.shipment_id,
            &[WireChunk {
                id: "AB".into(),
                length: 1
            }]
        ),
        Err(ShipError::Invalid(_))
    ));
    again.abort(&resumed.shipment_id).unwrap();
    assert!(!incoming(&paths).exists());
}

#[test]
fn reading_permission_is_written_per_device_and_keeps_the_rest_of_the_settings() {
    let temp = TestRoot::created("v2-readers");
    let paths = paths_with_config(&temp.0);
    assert!(
        set_reader(&paths, "phone", true).is_err(),
        "棚を配る前は付けられない"
    );
    let desktop = Library::create(&temp.0.join("desktop")).unwrap();
    save(&desktop, "1", "本文");
    let shipments = ShelfShipments::new(paths.clone(), None);
    ship(&shipments, &desktop, &temp.0);
    apply_pending(&paths);
    let owner = set_reader(&paths, "phone", true).unwrap();
    assert_eq!(owner.get(), desktop.id);
    set_reader(&paths, "tablet", true).unwrap();
    set_reader(&paths, "tablet", false).unwrap();
    let config = load_config(&paths).unwrap();
    let readers = &config.shelf.unwrap().readers;
    assert_eq!(readers.get("phone"), Some(&owner));
    assert!(!readers.contains_key("tablet"));
    assert!(!config.update.auto);
}

/// 単独サーバーを一つ、この試験の中で動かす（`run_locked` と同じ組み方）。
async fn serve(
    paths: &ServerPaths,
) -> (
    String,
    Arc<ServerStore>,
    Option<Arc<crate::server::reader::Context>>,
    tokio::task::JoinHandle<()>,
) {
    apply_pending(paths);
    let config = load_config(paths).unwrap();
    let parts = crate::server::standalone::assemble(paths, None, None).unwrap();
    let store = Arc::clone(&parts.context.store);
    let mut served = None;
    let mut shelf = None;
    if let Some(settings) = &config.shelf {
        let opened = settings.open(paths, Arc::clone(&store)).unwrap();
        served = Some(Arc::clone(opened.library()));
        shelf = Some(Arc::clone(&opened));
        parts.context.set_reader_shelf(opened).unwrap();
    }
    let shipments = Arc::new(ShelfShipments::new(paths.clone(), served));
    parts.context.set_shipments(paths.clone(), shipments);
    let router = crate::server::http::combined_router(Arc::clone(&parts.context));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (base, store, shelf, serving)
}

/// 止めて、開いていた棚を手放すまで待つ（つないだままの接続も閉じる）。
async fn stop(serving: tokio::task::JoinHandle<()>) {
    serving.abort();
    let _ = serving.await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
}

#[tokio::test]
async fn the_desktop_sends_its_shelf_over_http_and_only_changed_parts_go_the_second_time() {
    use crate::server::store::Role;
    use crate::server::sync_send::{admin_get, admin_put, RemoteServer, SendHandle};
    use std::time::Duration;

    let temp = TestRoot::created("v2-shipment-http");
    let paths = paths_with_config(&temp.0);
    let desktop = Arc::new(Library::create(&temp.0.join("desktop")).unwrap());
    let paged = save(&desktop, "1", "一[newpage]二");
    save(&desktop, "2", "本文");

    let (base, store, shelf, serving) = serve(&paths).await;
    assert!(shelf.is_none());
    let admin = store
        .consume_pairing(
            &store
                .issue_pairing(Role::Admin, Duration::from_secs(60))
                .unwrap(),
            "デスクトップ",
        )
        .unwrap();
    let phone = store
        .consume_pairing(
            &store
                .issue_pairing(Role::Reader, Duration::from_secs(60))
                .unwrap(),
            "携帯",
        )
        .unwrap();
    let remote = RemoteServer {
        url: base,
        fingerprint: None,
    };
    // 棚を配る前は、携帯に許可を付けられない。
    assert!(admin_put(
        &remote,
        &admin.token,
        &format!("devices/{}/shelf", phone.device_id),
        &serde_json::json!({"read": true}),
    )
    .await
    .is_err());
    let handle = SendHandle::new();
    let outgoing = temp.0.join("outgoing");
    let first = crate::server::shelf_send::send_shelf(
        &remote,
        &admin.token,
        Arc::clone(&desktop),
        &outgoing,
        &handle,
    )
    .await
    .unwrap();
    assert_eq!(first.works, Some(2));
    assert!(first.sent > 0);
    assert!(!outgoing.exists(), "送り手の作業場所は片付ける");
    // 見張り役がいないので、立ち上げ直すまで配るのを待っている。
    let status = admin_get(&remote, &admin.token, "status").await.unwrap();
    assert_eq!(status["shelfPending"], true);
    stop(serving).await;
    drop(store);

    // 立ち上げ直すと送った棚を配る。携帯ごとに許可を付けると、すぐ読める。
    let (base, store, shelf, serving) = serve(&paths).await;
    let remote = RemoteServer {
        url: base,
        fingerprint: None,
    };
    let shelf = shelf.expect("送った棚を配る");
    assert!(!shelf.reads(&phone.device_id));
    let status = admin_get(&remote, &admin.token, "status").await.unwrap();
    assert_eq!(status["shelfPending"], false);
    admin_put(
        &remote,
        &admin.token,
        &format!("devices/{}/shelf", phone.device_id),
        &serde_json::json!({"read": true}),
    )
    .await
    .unwrap();
    assert!(shelf.reads(&phone.device_id));
    let devices = admin_get(&remote, &admin.token, "devices").await.unwrap();
    let row = devices
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == phone.device_id.as_str())
        .unwrap()
        .clone();
    assert_eq!(row["readsShelf"], true);
    // 管理者の登録には付けられない。
    assert!(admin_put(
        &remote,
        &admin.token,
        &format!("devices/{}/shelf", admin.device_id),
        &serde_json::json!({"read": true}),
    )
    .await
    .is_err());

    // 携帯が配っている棚に読書位置を書き、デスクトップはそれを管理の口から読める。
    let at = ReadingLocation::Source {
        snapshot: paged.snapshot_id,
        anchor: ReadingAnchor::Page {
            page: 1,
            ratio_ppm: 0,
        },
    };
    let phone_device = LibraryId::now_v7();
    shelf
        .library()
        .set_reading_position(ReadingUpdate {
            request: LibraryId::now_v7(),
            work: paged.entity_id,
            user: desktop.id,
            device: phone_device,
            expected: 0,
            location: Some(at.clone()),
            finished: false,
        })
        .unwrap();
    let reading = admin_get(&remote, &admin.token, "shelf/reading?cursor=&limit=10")
        .await
        .unwrap();
    assert_eq!(reading["items"].as_array().unwrap().len(), 1);
    // お気に入りも同じように読める。初めは全部、印のあとは変わった分だけ。
    let liked = shelf
        .library()
        .user_state(paged.entity_id, desktop.id)
        .unwrap();
    shelf
        .library()
        .set_favorite(
            LibraryId::now_v7(),
            paged.entity_id,
            desktop.id,
            liked.generation,
            true,
        )
        .unwrap();
    let favorites = admin_get(&remote, &admin.token, "shelf/favorites?cursor=&limit=10")
        .await
        .unwrap();
    let item = favorites["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["work"] == paged.entity_id.to_string())
        .cloned()
        .unwrap();
    assert_eq!(item["favorite"], true);
    let mut cursor = favorites["cursor"].as_str().unwrap().to_string();
    let mut more = favorites["more"] == true;
    while more {
        let page = admin_get(
            &remote,
            &admin.token,
            &format!("shelf/favorites?cursor={cursor}&limit=10"),
        )
        .await
        .unwrap();
        cursor = page["cursor"].as_str().unwrap().to_string();
        more = page["more"] == true;
    }
    let after = admin_get(
        &remote,
        &admin.token,
        &format!("shelf/favorites?cursor={cursor}&limit=10"),
    )
    .await
    .unwrap();
    assert!(after["items"].as_array().unwrap().is_empty(), "{after}");

    // 二回目：一作品だけ足した。送るのは新しい実体と、変わった台帳の部分だけ。
    save(&desktop, "3", "三つめ");
    let second = crate::server::shelf_send::send_shelf(
        &remote,
        &admin.token,
        Arc::clone(&desktop),
        &outgoing,
        &handle,
    )
    .await
    .unwrap();
    assert_eq!(second.works, Some(3));
    assert!(second.sent < first.sent, "{second:?} / {first:?}");
    stop(serving).await;
    drop(shelf);
    drop(store);

    let (_base, _store, shelf, serving) = serve(&paths).await;
    let shelf = shelf.unwrap();
    assert!(shelf.reads(&phone.device_id), "許可は設定に残る");
    let moved = shelf
        .library()
        .reading_position(paged.entity_id, desktop.id)
        .unwrap();
    assert_eq!(moved.location, Some(at));
    assert_eq!(moved.device, Some(phone_device));
    assert!(
        shelf
            .library()
            .user_state(paged.entity_id, desktop.id)
            .unwrap()
            .favorite,
        "携帯が前の世代で付けたお気に入りも残る"
    );
    serving.abort();
}

/// 実データでの確認（手で動かす）。`PIEP_SHIP_PROBE` に、稼働棚の台帳の写しと一人の作者の
/// 実体だけを写した `shelf/` と、その作品の一覧 `works.txt` を置いたフォルダーを渡す。
/// 稼働棚には触れない。写しから部分写しを作り、手元のサーバーへ HTTP で二度送って測る。
#[tokio::test]
#[ignore]
async fn probe_shipping_a_real_partial_shelf() {
    use crate::server::store::Role;
    use crate::server::sync_send::{admin_get, RemoteServer, SendHandle};
    use std::time::{Duration, Instant};

    let Some(probe) = std::env::var_os("PIEP_SHIP_PROBE").map(PathBuf::from) else {
        return;
    };
    struct Print;
    impl log::Log for Print {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            eprintln!("[{}] {}", record.level(), record.args());
        }
        fn flush(&self) {}
    }
    static PRINT: Print = Print;
    let _ = log::set_logger(&PRINT);
    log::set_max_level(log::LevelFilter::Warn);
    let copy = Library::open(&probe.join("shelf")).unwrap();
    let works: Vec<LibraryId> = fs::read_to_string(probe.join("works.txt"))
        .unwrap()
        .lines()
        .map(|hex| uuid::Uuid::from_slice(&hex_bytes(hex)).unwrap())
        .collect();

    // 一つ目の測り：全体の台帳（実データの大きさ）で、小さな書き込みのあと変わる部分の数。
    let scratch = probe.join(format!("measure-{}", uuid::Uuid::now_v7()));
    fs::create_dir_all(&scratch).unwrap();
    let plan = |name: &str| ObjectStore::plan(fs::File::open(scratch.join(name)).unwrap()).unwrap();
    let started = Instant::now();
    let objects = copy.ship_snapshot(&scratch.join("a.db")).unwrap();
    let snapshot_a = started.elapsed();
    let started = Instant::now();
    let a = plan("a.db");
    let split_a = started.elapsed();
    let state = copy.user_state(works[0], copy.id).unwrap();
    copy.set_favorite(
        LibraryId::now_v7(),
        works[0],
        copy.id,
        state.generation,
        !state.favorite,
    )
    .unwrap();
    copy.ship_snapshot(&scratch.join("b.db")).unwrap();
    let b = plan("b.db");
    let before: std::collections::HashSet<_> = a.chunks.iter().map(|chunk| chunk.id).collect();
    let changed = b
        .chunks
        .iter()
        .filter(|chunk| !before.contains(&chunk.id))
        .count();
    let started = Instant::now();
    let mut encoded = 0;
    let mut offset = 0;
    let bytes = fs::read(scratch.join("b.db")).unwrap();
    for chunk in &b.chunks {
        let block = &bytes[offset..offset + chunk.length as usize];
        offset += chunk.length as usize;
        if !before.contains(&chunk.id) {
            encoded += piep_storage::encode_chunk(block).unwrap().1.len();
        }
    }
    println!(
        "full-db bytes={} parts={} changed_after_one_favorite={changed} encoded_bytes={encoded} encode={:?} objects_referenced={} snapshot={snapshot_a:?} plan={split_a:?}",
        a.length,
        a.chunks.len(),
        started.elapsed(),
        objects.len(),
    );
    fs::remove_dir_all(&scratch).unwrap();

    // 二つ目：一人の作者の部分写しを、自立した棚にして手元のサーバーへ送る。
    let part = probe.join(format!("part-{}", uuid::Uuid::now_v7()));
    let copied = copy.copy_works(&works, &part).unwrap();
    drop(copy);
    fs::rename(part.join("library-part.json"), part.join("library.json")).unwrap();
    let desktop = Arc::new(Library::open(&part).unwrap());
    println!("partial works={} copied={copied:?}", works.len());

    // サーバーの置き場は短い場所に（実際のサーバーの置き場も短い。長いと 260 文字を超える）。
    let server_root = std::env::var_os("PIEP_SHIP_SERVER")
        .map(PathBuf::from)
        .unwrap_or_else(|| probe.clone())
        .join(format!(
            "s{}",
            &uuid::Uuid::now_v7().simple().to_string()[..8]
        ));
    let paths = paths_with_config(&server_root);
    let (base, store, _, serving) = serve(&paths).await;
    let admin = store
        .consume_pairing(
            &store
                .issue_pairing(Role::Admin, Duration::from_secs(60))
                .unwrap(),
            "確認",
        )
        .unwrap();
    let remote = RemoteServer {
        url: base,
        fingerprint: None,
    };
    let handle = SendHandle::new();
    let outgoing = probe.join("outgoing");
    let started = Instant::now();
    let first = crate::server::shelf_send::send_shelf(
        &remote,
        &admin.token,
        Arc::clone(&desktop),
        &outgoing,
        &handle,
    )
    .await
    .unwrap_or_else(|error| panic!("{error} ({})", handle.progress.lock().unwrap().phase));
    let first_time = started.elapsed();
    let first_bytes = handle.progress.lock().unwrap().bytes_total;
    stop(serving).await;
    drop(store);

    let (base, store, shelf, serving) = serve(&paths).await;
    let remote = RemoteServer {
        url: base,
        fingerprint: None,
    };
    let shelf = shelf.unwrap();
    let status = admin_get(&remote, &admin.token, "status").await.unwrap();
    println!(
        "first send works={:?} sent={} bytes={} time={:?} served={}",
        first.works, first.sent, first_bytes, first_time, status["shelf"]["works"]
    );
    shelf.library().verify().unwrap();
    // 送った棚と手元の棚の作品が同じか。
    let count = |library: &Library| -> i64 {
        library
            .reader()
            .unwrap()
            .query_row("SELECT count(*) FROM library_items", [], |row| row.get(0))
            .unwrap()
    };
    assert_eq!(count(shelf.library()), count(&desktop));

    let state = desktop.user_state(works[1], desktop.id).unwrap();
    desktop
        .set_favorite(
            LibraryId::now_v7(),
            works[1],
            desktop.id,
            state.generation,
            !state.favorite,
        )
        .unwrap();
    let started = Instant::now();
    let second = crate::server::shelf_send::send_shelf(
        &remote,
        &admin.token,
        Arc::clone(&desktop),
        &outgoing,
        &handle,
    )
    .await
    .unwrap();
    let second_bytes = handle.progress.lock().unwrap().bytes_total;
    println!(
        "second send (one favorite) sent={} bytes={} time={:?}",
        second.sent,
        second_bytes,
        started.elapsed()
    );
    stop(serving).await;
    drop(shelf);
    drop(store);
    drop(desktop);
    tokio::time::sleep(Duration::from_secs(2)).await;
    for dir in [&server_root, &part] {
        if let Err(error) = fs::remove_dir_all(dir) {
            println!("片付けられません（あとで消す）: {} {error}", dir.display());
        }
    }
}

fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
        .collect()
}
