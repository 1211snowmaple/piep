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

async fn interrupted_desktop_upload(cancel: bool) {
    use crate::server::sync_send::{ClientCache, RemoteServer, SendHandle, CANCELED};
    use axum::{http::StatusCode, routing, Json, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    let temp = TestRoot::created("interrupted");
    let library = Arc::new(Library::create(&temp.0.join("desktop")).unwrap());
    for n in 1..=12 {
        save(&library, &n.to_string(), &format!("本文 {n}"));
    }
    let outgoing = temp.0.join("outgoing");
    let count = Arc::new(AtomicUsize::new(0));
    let uploads = Arc::clone(&count);
    let handle = SendHandle::new();
    let cancel_send = handle.clone();
    let snapshot = outgoing.join("library.db");
    let router = Router::new()
        .route(
            "/api/admin/shelf/shipments",
            routing::get(|| async {
                Json(ShipmentStatus {
                    schema: Some(crate::library::schema::VERSION),
                    publication_by_id: true,
                    ..Default::default()
                })
            })
            .post(|| async {
                Json(Opened {
                    shipment_id: "test".into(),
                    resumed: false,
                })
            }),
        )
        .route(
            "/api/admin/shelf/shipments/test/needed",
            routing::post(|Json(body): Json<serde_json::Value>| async move {
                let chunks = body["chunks"].as_array().unwrap();
                assert!(chunks.len() > 4);
                let needed: Vec<_> = chunks.iter().map(|chunk| &chunk["id"]).collect();
                Json(serde_json::json!({ "needed": needed }))
            }),
        )
        .route(
            "/api/admin/shelf/shipments/test/chunks/{id}",
            routing::put(move || {
                let uploads = Arc::clone(&uploads);
                let cancel_send = cancel_send.clone();
                let snapshot = snapshot.clone();
                async move {
                    assert!(snapshot.is_file());
                    uploads.fetch_add(1, Ordering::Relaxed);
                    if cancel {
                        cancel_send.cancel();
                        std::future::pending::<()>().await;
                    }
                    StatusCode::UNAUTHORIZED
                }
            }),
        );
    let clients = ClientCache::default();
    let remote = RemoteServer {
        url: crate::server::tests::spawn_plain(router),
        fingerprint: None,
    };
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        crate::server::shelf_send::send_shelf(
            &clients, &remote, "test", library, &outgoing, &handle,
        ),
    )
    .await;
    let requests = count.load(Ordering::Relaxed);
    assert!(
        (1..=4).contains(&requests) && !outgoing.exists(),
        "実体の要求数: {requests}、写しの残存: {}",
        outgoing.exists(),
    );
    let expected = if cancel {
        CANCELED
    } else {
        "サーバーに登録が残っていません。登録し直してください"
    };
    assert_eq!(result.expect("送信をすぐに終える").unwrap_err(), expected);
}

#[tokio::test]
async fn desktop_upload_stops_after_the_first_fatal_failure_and_removes_the_snapshot() {
    interrupted_desktop_upload(false).await;
}

#[tokio::test]
async fn canceling_a_desktop_upload_removes_the_snapshot() {
    interrupted_desktop_upload(true).await;
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
    let first_id = first.status().shipment_id.unwrap();
    assert_eq!(
        first.result(&first_id).unwrap().state,
        PublicationState::Pending { restarting: false }
    );
    assert!(matches!(
        first.open(desktop.id),
        Err(ShipError::Conflict(_))
    ));
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
    let first_result = PublicationState::Published(PublishedShelf {
        sync_epoch: config.sync_epoch,
        works: 2,
    });
    assert_eq!(second.result(&first_id).unwrap().state, first_result);
    let sent_again = ship(&second, &desktop, &temp.0);
    assert!(sent_again > 0 && sent_again < sent + 3, "{sent_again}");
    let second_id = second.status().shipment_id.unwrap();
    assert_ne!(second_id, first_id);
    assert_eq!(second.result(&first_id).unwrap().state, first_result);
    assert_eq!(
        second.result(&second_id).unwrap().state,
        PublicationState::Pending { restarting: false }
    );
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
            reader_pages: Some(2),
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
    let restarted = ShelfShipments::new(paths.clone(), None);
    assert_eq!(restarted.result(&first_id).unwrap().state, first_result);
    // 台帳の部分は、最後に組んだ台帳のもの（小さな台帳なので一つ）だけが残る。
    let parts = ObjectStore::open(&paths.root.join(DATABASE_PARTS)).unwrap();
    assert_eq!(parts.listing().unwrap().len(), 1);
}

#[test]
fn an_interrupted_swap_keeps_phone_changes_after_restart() {
    for stop_at in [
        SwapStage::AfterActivation,
        SwapStage::BeforeAdoption,
        SwapStage::AfterAdoption,
    ] {
        let temp = TestRoot::created("v2-shipment-restart");
        let paths = paths_with_config(&temp.0);
        let desktop = Library::create(&temp.0.join("desktop")).unwrap();
        let work = save(&desktop, "1", "一[newpage]二");
        ship(&ShelfShipments::new(paths.clone(), None), &desktop, &temp.0);
        apply_pending(&paths);
        let before = load_config(&paths).unwrap().shelf.unwrap();
        let old = Arc::new(Library::open(&before.root(&paths).unwrap()).unwrap());
        let shipments = ShelfShipments::new(paths.clone(), Some(Arc::clone(&old)));
        ship(&shipments, &desktop, &temp.0);
        let target = shipments.status().generation.unwrap();
        let shipment_id = shipments.status().shipment_id.unwrap();
        let phone = LibraryId::now_v7();
        let location = ReadingLocation::Source {
            snapshot: work.snapshot_id,
            anchor: ReadingAnchor::Page {
                page: 1,
                ratio_ppm: 0,
            },
        };
        old.set_reading_position(ReadingUpdate {
            request: LibraryId::now_v7(),
            work: work.entity_id,
            user: desktop.id,
            device: phone,
            expected: 0,
            location: Some(location.clone()),
            finished: false,
            reader_pages: Some(2),
        })
        .unwrap();
        let favorite = old.user_state(work.entity_id, desktop.id).unwrap();
        old.set_favorite(
            LibraryId::now_v7(),
            work.entity_id,
            desktop.id,
            favorite.generation,
            true,
        )
        .unwrap();
        // 前の棚をまだ開いているときは引き継げない。配信設定と予約を保って再試行する。
        let error = apply_pending_with(&paths, |_| {}).unwrap_err();
        assert!(error.starts_with("前の棚を開けません"), "{error}");
        assert_eq!(
            shipments.result(&shipment_id).unwrap().state,
            PublicationState::Pending { restarting: false }
        );
        let unchanged = load_config(&paths).unwrap().shelf.unwrap();
        assert_eq!(unchanged.generation, before.generation);
        assert_eq!(unchanged.sync_epoch, before.sync_epoch);
        drop(shipments);
        drop(old);

        let pending = read_pending(&paths).unwrap().unwrap();
        assert_eq!(pending.library_id, desktop.id);
        assert_eq!(
            pending.previous,
            ShelfSource::Generation(before.generation.unwrap())
        );
        assert_eq!(pending.next_generation, target);
        // 手動で先に配ると引き継げなくなるため、予約した世代の選択は起動時に任せる。
        assert!(super::super::generations::activate(&paths, target).is_err());

        // 停止点で処理を打ち切り、ディスクに残った記録だけで起動し直す。
        let interrupted = std::panic::catch_unwind(|| {
            apply_pending_with(&paths, |stage| {
                if stage == stop_at {
                    std::panic::panic_any(stage);
                }
            })
            .unwrap();
        })
        .unwrap_err();
        assert_eq!(interrupted.downcast_ref::<SwapStage>(), Some(&stop_at));
        assert!(paths.root.join(PENDING).is_file());
        let interrupted_config = load_config(&paths).unwrap().shelf.unwrap();
        apply_pending(&paths);

        let after = load_config(&paths).unwrap().shelf.unwrap();
        assert_eq!(after.generation, Some(target));
        assert_ne!(after.sync_epoch, before.sync_epoch);
        assert_eq!(
            ShelfShipments::new(paths.clone(), None)
                .result(&shipment_id)
                .unwrap()
                .state,
            PublicationState::Published(PublishedShelf {
                sync_epoch: after.sync_epoch,
                works: 1
            })
        );
        let new = Library::open(&after.root(&paths).unwrap()).unwrap();
        let moved = new.reading_position(work.entity_id, desktop.id).unwrap();
        assert_eq!(moved.location, Some(location), "{stop_at:?}");
        assert_eq!(moved.device, Some(phone));
        let favorite = new.user_state(work.entity_id, desktop.id).unwrap();
        assert!(favorite.favorite, "{stop_at:?}");
        assert_eq!(moved.generation, 1);
        assert_eq!(favorite.generation, 1);
        if stop_at == SwapStage::AfterActivation {
            assert_eq!(after.sync_epoch, interrupted_config.sync_epoch);
        } else {
            assert_eq!(interrupted_config.generation, before.generation);
            assert_eq!(interrupted_config.sync_epoch, before.sync_epoch);
        }
        assert!(before.root(&paths).unwrap().is_dir());
        assert!(!paths.root.join(PENDING).exists());
    }
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
    let restarted = ShelfShipments::new(paths.clone(), None);
    assert_eq!(
        restarted.result(&resumed.shipment_id).unwrap().state,
        PublicationState::Rejected {
            reason: Rejection::Canceled
        }
    );
}

#[test]
fn an_interrupted_assembly_is_rejected_and_its_id_cannot_be_reused() {
    let temp = TestRoot::created("v2-shipment-interrupted");
    let paths = paths_with_config(&temp.0);
    let library_id = LibraryId::now_v7();
    let receiver = ShelfShipments::new(paths.clone(), None);
    let id = receiver.open(library_id).unwrap().shipment_id;
    write_result(&paths, &id, &PublicationState::Assembling).unwrap();
    drop(receiver);

    apply_pending(&paths);
    let restarted = ShelfShipments::new(paths, None);
    assert_ne!(restarted.open(library_id).unwrap().shipment_id, id);
    assert_eq!(
        restarted.result(&id).unwrap().state,
        PublicationState::Rejected {
            reason: Rejection::Interrupted
        }
    );
}

#[test]
fn a_failed_assembly_keeps_its_rejection_after_restart() {
    let temp = TestRoot::created("v2-shipment-assembly-failure");
    let paths = paths_with_config(&temp.0);
    let receiver = ShelfShipments::new(paths.clone(), None);
    let id = receiver.open(LibraryId::now_v7()).unwrap().shipment_id;
    // 実体と台帳のハッシュは合うが、棚として開けない台帳を送る。
    let bytes = b"invalid database";
    let database = ObjectStore::plan(&bytes[..]).unwrap();
    let (chunk, body) = piep_storage::encode_chunk(bytes).unwrap();
    receiver
        .put_chunk(
            &id,
            &WireChunk {
                id: chunk.id.to_string(),
                length: chunk.length,
            },
            &body,
        )
        .unwrap();
    receiver.commit(&id, &database).unwrap();
    let rejected = PublicationState::Rejected {
        reason: Rejection::AssemblyFailed,
    };
    for _ in 0..600 {
        let state = receiver.result(&id).unwrap().state;
        if state != PublicationState::Assembling {
            assert_eq!(state, rejected);
            drop(receiver);
            apply_pending(&paths);
            let restarted = ShelfShipments::new(paths, None);
            assert_eq!(restarted.result(&id).unwrap().state, rejected);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("組み立ての拒否を確認できません");
}

#[tokio::test]
async fn a_superseded_reservation_is_rejected_for_its_own_shipment() {
    use crate::server::store::Role;
    use crate::server::sync_send::{admin_get, ClientCache, RemoteServer};

    let temp = TestRoot::created("v2-shipment-superseded");
    let paths = paths_with_config(&temp.0);
    let desktop = Library::create(&temp.0.join("desktop")).unwrap();
    save(&desktop, "1", "本文");
    let receiver = ShelfShipments::new(paths.clone(), None);
    ship(&receiver, &desktop, &temp.0);
    let id = receiver.status().shipment_id.unwrap();
    let staged = temp.0.join("staged");
    desktop.backup(&staged).unwrap();
    let alternative = super::super::generations::stage(&paths, &staged).unwrap();
    super::super::generations::activate(&paths, alternative).unwrap();
    drop(receiver);

    apply_pending(&paths);
    let restarted = ShelfShipments::new(paths.clone(), None);
    assert_eq!(
        restarted.result(&id).unwrap().state,
        PublicationState::Rejected {
            reason: Rejection::Superseded
        }
    );
    assert_eq!(
        load_config(&paths).unwrap().shelf.unwrap().generation,
        Some(alternative)
    );
    assert!(!restarted.pending());
    assert!(restarted.open(desktop.id).is_ok());

    // 次の起動で同じ拒否を重ねて記録しない。
    let (base, store, _, serving) = serve(&paths, "127.0.0.1:0").await;
    let admin = store
        .consume_pairing(
            &store
                .issue_pairing(Role::Admin, std::time::Duration::from_secs(60))
                .unwrap(),
            "デスクトップ",
        )
        .unwrap();
    let remote = RemoteServer {
        url: base,
        fingerprint: None,
    };
    let audit = admin_get(
        &ClientCache::default(),
        &remote,
        &admin.token,
        "audit?kind=library&ok=false",
    )
    .await
    .unwrap();
    let audit = audit.as_array().unwrap();
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0]["kind"], "library");
    assert_eq!(audit[0]["action"], "棚の公開を拒否");
    assert_eq!(audit[0]["actor"], "サーバー");
    assert_eq!(audit[0]["ok"], false);
    assert_eq!(
        audit[0]["detail"],
        format!("送信 ID {id}・引き継ぎ元の棚が変わりました")
    );
    stop(serving).await;
}

#[test]
fn a_broken_publication_reservation_is_audited() {
    let temp = TestRoot::created("v2-shipment-broken-reservation");
    let paths = paths_with_config(&temp.0);
    let marker = paths.root.join(PENDING);
    fs::write(&marker, "壊れた予約").unwrap();

    apply_pending(&paths);

    assert!(marker.exists());
    let store = ServerStore::open(&paths.store()).unwrap();
    let audit = store
        .list_audit(Some("library"), Some(false), None, 10)
        .unwrap();
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].action, "棚の公開に失敗");
    assert_eq!(audit[0].actor, "サーバー");
    assert!(!audit[0].ok);
    assert_eq!(
        audit[0].detail.as_deref(),
        Some("棚の切り替え処理を完了できませんでした。次の起動で再試行します")
    );
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
    address: &str,
) -> (
    String,
    Arc<ServerStore>,
    Option<Arc<crate::server::reader::Context>>,
    Serving,
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
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (shutdown, closed) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = closed.await;
            })
            .await
            .unwrap()
    });
    (base, store, shelf, Serving { shutdown, task })
}

/// 試験の中で動かしている単独サーバー。
struct Serving {
    shutdown: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

async fn restart_when_pending(
    paths: &ServerPaths,
    base: &str,
    serving: Serving,
) -> (
    String,
    Arc<ServerStore>,
    Option<Arc<crate::server::reader::Context>>,
    Serving,
) {
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        while !paths.root.join(PENDING).is_file() {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("棚を組み終えません");
    stop(serving).await;
    serve(paths, base.strip_prefix("http://").unwrap()).await
}

/// 止めて、開いていた棚を手放すまで待つ。
///
/// 本番は再起動でプロセスを抜けるので、棚の錠はそこで外れる。試験は同じプロセスで立て直すので、
/// つないだままの接続のタスクが棚を掴み終えるまで待つ必要がある。以前は待ち受けを abort して
/// 300ms 眠るだけで、abort は接続のタスクを止めないため、混んだ PC では前の棚が掴まれたまま
/// 次の起動の引き継ぎが「このライブラリは別の処理で使用中です」で落ちた（AUD-024）。
/// 穏やかな停止は接続を閉じ、全部の接続のタスクが終わってから返る。
async fn stop(serving: Serving) {
    let _ = serving.shutdown.send(());
    tokio::time::timeout(std::time::Duration::from_secs(30), serving.task)
        .await
        .expect("待ち受けが止まりません")
        .unwrap();
}

#[tokio::test]
async fn the_desktop_sends_its_shelf_over_http_and_only_changed_parts_go_the_second_time() {
    use crate::server::store::Role;
    use crate::server::sync_send::{
        admin_get, admin_post, admin_put, ClientCache, RemoteServer, SendHandle,
    };
    use std::time::Duration;

    let temp = TestRoot::created("v2-shipment-http");
    let paths = paths_with_config(&temp.0);
    let desktop = Arc::new(Library::create(&temp.0.join("desktop")).unwrap());
    let paged = save(&desktop, "1", "一[newpage]二");
    save(&desktop, "2", "本文");

    let (base, store, shelf, serving) = serve(&paths, "127.0.0.1:0").await;
    assert!(shelf.is_none());
    store.audit("system", "起動", "サーバー", None, true, None);
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
    let clients = ClientCache::default();
    let remote = RemoteServer {
        url: base,
        fingerprint: None,
    };
    // 棚を配る前は、携帯に許可を付けられない。
    assert!(admin_put(
        &clients,
        &remote,
        &admin.token,
        &format!("devices/{}/shelf", phone.device_id),
        &serde_json::json!({"read": true}),
    )
    .await
    .is_err());
    let handle = SendHandle::new();
    let outgoing = temp.0.join("outgoing");
    drop(store);
    let (first, (_, store, shelf, serving)) = tokio::join!(
        crate::server::shelf_send::send_shelf(
            &clients,
            &remote,
            &admin.token,
            Arc::clone(&desktop),
            &outgoing,
            &handle,
        ),
        restart_when_pending(&paths, &remote.url, serving),
    );
    let first = first.unwrap();
    assert_eq!(first.works, 2);
    assert!(first.sent > 0);
    assert!(!outgoing.exists(), "送り手の作業場所は片付ける");

    // 立ち上げ直すと送った棚を配る。携帯ごとに許可を付けると、すぐ読める。
    let shelf = shelf.expect("送った棚を配る");
    assert!(!shelf.reads(&phone.device_id));
    let status = admin_get(&clients, &remote, &admin.token, "status")
        .await
        .unwrap();
    assert_eq!(status["shelfPending"], false);
    assert_eq!(
        status["shelf"]["syncEpoch"],
        first.sync_epoch.get().to_string()
    );
    admin_put(
        &clients,
        &remote,
        &admin.token,
        &format!("devices/{}/shelf", phone.device_id),
        &serde_json::json!({"read": true}),
    )
    .await
    .unwrap();
    assert!(shelf.reads(&phone.device_id));
    let devices = admin_get(&clients, &remote, &admin.token, "devices")
        .await
        .unwrap();
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
        &clients,
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
            reader_pages: Some(2),
        })
        .unwrap();
    let reading = admin_get(
        &clients,
        &remote,
        &admin.token,
        "shelf/reading?cursor=&limit=10",
    )
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
    let favorites = admin_get(
        &clients,
        &remote,
        &admin.token,
        "shelf/favorites?cursor=&limit=10",
    )
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
            &clients,
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
        &clients,
        &remote,
        &admin.token,
        &format!("shelf/favorites?cursor={cursor}&limit=10"),
    )
    .await
    .unwrap();
    assert!(after["items"].as_array().unwrap().is_empty(), "{after}");

    // 二回目：一作品だけ足した。送るのは新しい実体と、変わった台帳の部分だけ。
    save(&desktop, "3", "三つめ");
    drop(shelf);
    drop(store);
    let (second, (_, _store, shelf, serving)) = tokio::join!(
        crate::server::shelf_send::send_shelf(
            &clients,
            &remote,
            &admin.token,
            Arc::clone(&desktop),
            &outgoing,
            &handle,
        ),
        restart_when_pending(&paths, &remote.url, serving),
    );
    let second = second.unwrap();
    assert_eq!(second.works, 3);
    assert!(second.sent < first.sent, "{second:?} / {first:?}");
    assert_ne!(second.sync_epoch, first.sync_epoch);
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

    admin_put(
        &clients,
        &remote,
        &admin.token,
        &format!("devices/{}/shelf", phone.device_id),
        &serde_json::json!({"read": false}),
    )
    .await
    .unwrap();
    assert!(!shelf.reads(&phone.device_id));
    let opened = admin_post(
        &clients,
        &remote,
        &admin.token,
        "shelf/shipments",
        &serde_json::json!({"libraryId": desktop.id}),
    )
    .await
    .unwrap();
    let aborted_id = opened["shipmentId"].as_str().unwrap();
    let aborted = reqwest::Client::new()
        .delete(format!(
            "{}/api/admin/shelf/shipments/{aborted_id}",
            remote.url
        ))
        .bearer_auth(&admin.token)
        .send()
        .await
        .unwrap();
    assert_eq!(aborted.status(), reqwest::StatusCode::NO_CONTENT);

    let audit = admin_get(&clients, &remote, &admin.token, "audit?kind=library")
        .await
        .unwrap();
    let audit = audit.as_array().unwrap();
    let expected = [
        ("棚の受け取りを中止", "デスクトップ版"),
        ("棚の受け取りを開始", "デスクトップ版"),
        ("棚を読む許可の変更", "デスクトップ版"),
        ("棚を公開", "サーバー"),
        ("棚の受け取りを確定", "デスクトップ版"),
        ("棚の受け取りを開始", "デスクトップ版"),
        ("棚を読む許可の変更", "デスクトップ版"),
        ("棚を公開", "サーバー"),
        ("棚の受け取りを確定", "デスクトップ版"),
        ("棚の受け取りを開始", "デスクトップ版"),
    ];
    assert_eq!(audit.len(), expected.len(), "{audit:?}");
    for (row, (action, actor)) in audit.iter().zip(expected) {
        assert_eq!(row["kind"], "library");
        assert_eq!(row["action"], action);
        assert_eq!(row["actor"], actor);
        assert_eq!(row["ok"], true);
    }
    assert_eq!(audit[0]["detail"], format!("送信 ID {aborted_id}"));
    assert_eq!(audit[2]["detail"], format!("端末 {}・切", phone.device_id));
    assert_eq!(audit[6]["detail"], format!("端末 {}・入", phone.device_id));
    stop(serving).await;
}

/// 実データでの確認（手で動かす）。`PIEP_SHIP_PROBE` に、稼働棚の台帳の写しと一人の作者の
/// 実体だけを写した `shelf/` と、その作品の一覧 `works.txt` を置いたフォルダーを渡す。
/// 稼働棚には触れない。写しから部分写しを作り、手元のサーバーへ HTTP で二度送って測る。
#[tokio::test]
#[ignore]
async fn probe_shipping_a_real_partial_shelf() {
    use crate::server::store::Role;
    use crate::server::sync_send::{admin_get, ClientCache, RemoteServer, SendHandle};
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
    let (base, store, _, serving) = serve(&paths, "127.0.0.1:0").await;
    let admin = store
        .consume_pairing(
            &store
                .issue_pairing(Role::Admin, Duration::from_secs(60))
                .unwrap(),
            "確認",
        )
        .unwrap();
    let clients = ClientCache::default();
    let remote = RemoteServer {
        url: base,
        fingerprint: None,
    };
    let handle = SendHandle::new();
    let outgoing = probe.join("outgoing");
    let started = Instant::now();
    drop(store);
    let (first, (_, store, shelf, serving)) = tokio::join!(
        crate::server::shelf_send::send_shelf(
            &clients,
            &remote,
            &admin.token,
            Arc::clone(&desktop),
            &outgoing,
            &handle,
        ),
        restart_when_pending(&paths, &remote.url, serving),
    );
    let first =
        first.unwrap_or_else(|error| panic!("{error} ({})", handle.progress.lock().unwrap().phase));
    let first_time = started.elapsed();
    let first_bytes = handle.progress.lock().unwrap().bytes_total;
    let shelf = shelf.unwrap();
    let status = admin_get(&clients, &remote, &admin.token, "status")
        .await
        .unwrap();
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
    drop(shelf);
    drop(store);
    let (second, (_, store, shelf, serving)) = tokio::join!(
        crate::server::shelf_send::send_shelf(
            &clients,
            &remote,
            &admin.token,
            Arc::clone(&desktop),
            &outgoing,
            &handle,
        ),
        restart_when_pending(&paths, &remote.url, serving),
    );
    let second = second.unwrap();
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
