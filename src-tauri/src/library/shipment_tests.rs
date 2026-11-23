use super::*;
use crate::library::{ReadingAnchor, ReadingLocation, WorkSave};
use std::path::PathBuf;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("piep-shipment-{}", LibraryId::now_v7()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

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

/// 送り手から、受け手が持っていない実体だけを受け取る（HTTP の代わり）。
fn receive(
    source: &Library,
    chunks: &[FileChunk],
    have: &[&ObjectStore],
    into: &ObjectStore,
) -> usize {
    let mut sent = 0;
    for chunk in chunks {
        if have.iter().any(|store| store.has_object(chunk.id).unwrap()) {
            continue;
        }
        let mut encoded = Vec::new();
        source.objects.export_chunk(chunk, &mut encoded).unwrap();
        into.import_chunk(chunk, std::io::Cursor::new(encoded))
            .unwrap();
        sent += 1;
    }
    sent
}

fn store(path: &Path) -> ObjectStore {
    fs::create_dir_all(path).unwrap();
    ObjectStore::open(path).unwrap()
}

#[test]
fn reclaim_cannot_quarantine_objects_in_a_shipment_snapshot() {
    let temp = Temp::new();
    let source = Library::create(&temp.0.join("source")).unwrap();
    let orphan = source
        .store_original(&b"unreferenced file".to_vec().into())
        .unwrap();
    let chunks = source.ship_snapshot(&temp.0.join("shipment.db")).unwrap();
    let chunk = &orphan.chunks[0];
    assert!(chunks.contains(chunk));

    let options = super::super::ReclaimOptions::default();
    let now = std::time::SystemTime::now() + options.grace * 2;
    source.reclaim_at(options, now).unwrap();
    source.objects.export_chunk(chunk, &mut Vec::new()).unwrap();
    drop(chunks);

    source.reclaim_at(options, now).unwrap();
    assert!(!source.objects.has_object(chunk.id).unwrap());
}

#[test]
fn a_shipment_becomes_a_verified_shelf_and_the_next_one_only_carries_new_objects() {
    let temp = Temp::new();
    let source = Library::create(&temp.0.join("desktop")).unwrap();
    save(&source, "1", "一つめの本文");
    save(&source, "2", "二つめの本文");

    // 初回：全部の実体を受け取って組む。
    let first_db = temp.0.join("first.db");
    let chunks = source.ship_snapshot(&first_db).unwrap();
    assert!(!chunks.is_empty());
    assert!(
        source.ship_snapshot(&first_db).is_err(),
        "書きかけの写しに重ねない"
    );
    let received = store(&temp.0.join("in-1"));
    assert_eq!(receive(&source, &chunks, &[], &received), chunks.len());
    let served = temp.0.join("gen-1");
    let first_made =
        Library::assemble_shipment(&served, &first_db, &received, None, source.id).unwrap();
    assert_eq!(first_made.works, 2);
    // 初回は前に検査した棚が無いので、全部のファイルを読み直す。
    assert_eq!(first_made.read, first_made.files);
    let first = Library::open_matching(&served, source.id).unwrap();
    assert_eq!(first.works(10).unwrap().len(), 2);

    // 二回目：足した作品の実体だけを受け取り、ほかは配っている棚と共有する。
    save(&source, "3", "三つめの本文");
    let second_db = temp.0.join("second.db");
    let chunks = source.ship_snapshot(&second_db).unwrap();
    let received = store(&temp.0.join("in-2"));
    let sent = receive(&source, &chunks, &[&first.objects], &received);
    assert!(sent > 0 && sent < chunks.len(), "{sent} / {}", chunks.len());
    let next = temp.0.join("gen-2");
    let second_made =
        Library::assemble_shipment(&next, &second_db, &received, Some(&first), source.id).unwrap();
    assert_eq!(second_made.works, 3);
    // 二回目は、新しく受け取った実体を含むファイルだけを読み直す。
    assert!(
        second_made.read > 0 && second_made.read < second_made.files,
        "{second_made:?}"
    );
    let second = Library::open_matching(&next, source.id).unwrap();
    assert_eq!(second.works(10).unwrap().len(), 3);
    second.verify().unwrap();
    // 前の写しはそのまま開ける（共有した実体を消していない）。
    first.verify().unwrap();
}

#[test]
fn a_shipment_missing_an_object_or_from_another_shelf_is_not_published() {
    let temp = Temp::new();
    let source = Library::create(&temp.0.join("desktop")).unwrap();
    save(&source, "1", "本文");
    let database = temp.0.join("ship.db");
    let chunks = source.ship_snapshot(&database).unwrap();
    let received = store(&temp.0.join("in"));
    receive(&source, &chunks[1..], &[], &received);
    let destination = temp.0.join("gen");
    assert!(
        Library::assemble_shipment(&destination, &database, &received, None, source.id).is_err()
    );
    assert!(!destination.exists());
    receive(&source, &chunks[..1], &[], &received);
    // 別の棚の ID では組まない。
    assert!(Library::assemble_shipment(
        &destination,
        &database,
        &received,
        None,
        LibraryId::now_v7()
    )
    .is_err());
    assert!(!destination.exists());
    // 途中の置き場を残さない。
    let leftovers: Vec<_> = fs::read_dir(&temp.0)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".piep-shipment-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    Library::assemble_shipment(&destination, &database, &received, None, source.id).unwrap();
}

#[test]
fn positions_written_on_the_old_copy_after_shipping_move_to_the_new_copy() {
    let temp = Temp::new();
    let source = Library::create(&temp.0.join("desktop")).unwrap();
    let paged = save(&source, "1", "一[newpage]二");
    let other = save(&source, "2", "本文");
    let ship = |name: &str, previous: Option<&Library>| {
        let database = temp.0.join(format!("{name}.db"));
        let chunks = source.ship_snapshot(&database).unwrap();
        let received = store(&temp.0.join(format!("in-{name}")));
        receive(
            &source,
            &chunks,
            &previous.map(|p| &p.objects).into_iter().collect::<Vec<_>>(),
            &received,
        );
        let destination = temp.0.join(name);
        Library::assemble_shipment(&destination, &database, &received, previous, source.id)
            .unwrap();
        Library::open_matching(&destination, source.id).unwrap()
    };
    let old = ship("old", None);
    let new = ship("new", Some(&old));
    // 新しい写しを作ったあとで、携帯が古い写しに書いた。
    let phone = LibraryId::now_v7();
    let at = |page| ReadingLocation::Source {
        snapshot: paged.snapshot_id,
        anchor: ReadingAnchor::Page { page, ratio_ppm: 0 },
    };
    old.set_reading_position(ReadingUpdate {
        request: LibraryId::now_v7(),
        work: paged.entity_id,
        user: source.id,
        device: phone,
        expected: 0,
        location: Some(at(1)),
        finished: false,
    })
    .unwrap();
    assert_eq!(new.adopt_from_previous(&old).unwrap().0, 1);
    let moved = new.reading_position(paged.entity_id, source.id).unwrap();
    assert_eq!(moved.location, Some(at(1)));
    assert_eq!(moved.device, Some(phone));
    // 二度目は何もしない。新しい写しのほうが新しい位置も変えない。
    assert_eq!(new.adopt_from_previous(&old).unwrap().0, 0);
    std::thread::sleep(std::time::Duration::from_millis(5));
    new.set_reading_position(ReadingUpdate {
        request: LibraryId::now_v7(),
        work: paged.entity_id,
        user: source.id,
        device: phone,
        expected: moved.generation,
        location: Some(at(0)),
        finished: false,
    })
    .unwrap();
    assert_eq!(new.adopt_from_previous(&old).unwrap().0, 0);
    assert_eq!(
        new.reading_position(paged.entity_id, source.id)
            .unwrap()
            .location,
        Some(at(0))
    );
    let _ = other;
    new.verify().unwrap();
}

/// `source` の今の写しを `name` に組んで開く（前の写しの実体は共有する）。
fn ship(temp: &Temp, source: &Library, name: &str, previous: Option<&Library>) -> Library {
    let database = temp.0.join(format!("{name}.db"));
    let chunks = source.ship_snapshot(&database).unwrap();
    let received = store(&temp.0.join(format!("in-{name}")));
    receive(
        source,
        &chunks,
        &previous.map(|p| &p.objects).into_iter().collect::<Vec<_>>(),
        &received,
    );
    let destination = temp.0.join(name);
    Library::assemble_shipment(&destination, &database, &received, previous, source.id).unwrap();
    Library::open_matching(&destination, source.id).unwrap()
}

/// 写しの読書位置の書いた時刻を `at` にする（機械の時計がずれている代わり）。
fn backdate(copy: &Path, work: LibraryId, at: i64) {
    let conn = rusqlite::Connection::open(copy.join("library.db")).unwrap();
    assert_eq!(
        conn.execute(
            "UPDATE reading_positions SET updated_at=?1 WHERE work_id=?2",
            rusqlite::params![at, work.as_bytes().as_slice()],
        )
        .unwrap(),
        1
    );
}

#[test]
fn the_phone_wins_unless_the_desktop_changed_it_too_whatever_the_clocks_say() {
    let temp = Temp::new();
    let source = Library::create(&temp.0.join("desktop")).unwrap();
    let paged = save(&source, "1", "一[newpage]二[newpage]三");
    let liked = save(&source, "2", "本文");
    let third = save(&source, "3", "本文");
    let owner = source.id;
    let (desktop, phone) = (LibraryId::now_v7(), LibraryId::now_v7());
    let at = |page| ReadingLocation::Source {
        snapshot: paged.snapshot_id,
        anchor: ReadingAnchor::Page { page, ratio_ppm: 0 },
    };
    let read = |library: &Library, device, page| {
        let expected = library
            .reading_position(paged.entity_id, owner)
            .unwrap()
            .generation;
        library
            .set_reading_position(ReadingUpdate {
                request: LibraryId::now_v7(),
                work: paged.entity_id,
                user: owner,
                device,
                expected,
                location: Some(at(page)),
                finished: false,
            })
            .unwrap();
    };
    let like = |library: &Library, work, favorite| {
        let expected = library.user_state(work, owner).unwrap().generation;
        library
            .set_favorite(LibraryId::now_v7(), work, owner, expected, favorite)
            .unwrap();
    };
    read(&source, desktop, 2);
    let old = ship(&temp, &source, "old", None);
    let new = ship(&temp, &source, "new", Some(&old));

    // 送ったあと、時計の遅れた携帯が古い写しに書いた。デスクトップは変えていないので携帯の値にする。
    read(&old, phone, 1);
    backdate(&temp.0.join("old"), paged.entity_id, 1);
    like(&old, liked.entity_id, true);
    let before = new.outbox_sequence().unwrap();
    assert_eq!(new.adopt_from_previous(&old).unwrap(), (1, 1));
    let moved = new.reading_position(paged.entity_id, owner).unwrap();
    assert_eq!((moved.location, moved.device), (Some(at(1)), Some(phone)));
    assert!(new.user_state(liked.entity_id, owner).unwrap().favorite);
    // 写した変更は版 8 の要求で書き、この棚自身の変更には数えない。
    assert_eq!(adopted_request().as_bytes()[6] >> 4, 8);
    assert!(!new
        .changed_here_since(paged.entity_id, "reading_position", before)
        .unwrap());
    assert!(!new
        .changed_here_since(liked.entity_id, "user_state", before)
        .unwrap());
    assert!(new
        .changed_here_since(paged.entity_id, "reading_position", 0)
        .unwrap());

    // 両方で変えたとき：読書位置は書いた時刻の新しいほう、お気に入りはデスクトップの値を残す。
    read(&source, desktop, 0);
    like(&source, third.entity_id, true);
    let newer = ship(&temp, &source, "newer", Some(&new));
    read(&new, phone, 2);
    backdate(&temp.0.join("new"), paged.entity_id, 1);
    like(&new, third.entity_id, true);
    like(&new, third.entity_id, false);
    // 前の写しに写してあった携帯のお気に入りも、デスクトップが取り込む前なら引き継ぐ。
    assert_eq!(newer.adopt_from_previous(&new).unwrap(), (0, 1));
    assert!(newer.user_state(liked.entity_id, owner).unwrap().favorite);
    assert_eq!(
        newer
            .reading_position(paged.entity_id, owner)
            .unwrap()
            .location,
        Some(at(0))
    );
    assert!(newer.user_state(third.entity_id, owner).unwrap().favorite);
    newer.verify().unwrap();
}
