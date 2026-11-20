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
    let works = Library::assemble_shipment(&served, &first_db, &received, None, source.id).unwrap();
    assert_eq!(works, 2);
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
    assert_eq!(
        Library::assemble_shipment(
            &next,
            &second_db,
            &received,
            Some(&first.objects),
            source.id
        )
        .unwrap(),
        3
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
    let ship = |name: &str, previous: Option<&ObjectStore>| {
        let database = temp.0.join(format!("{name}.db"));
        let chunks = source.ship_snapshot(&database).unwrap();
        let received = store(&temp.0.join(format!("in-{name}")));
        receive(
            &source,
            &chunks,
            &previous.into_iter().collect::<Vec<_>>(),
            &received,
        );
        let destination = temp.0.join(name);
        Library::assemble_shipment(&destination, &database, &received, previous, source.id)
            .unwrap();
        Library::open_matching(&destination, source.id).unwrap()
    };
    let old = ship("old", None);
    let new = ship("new", Some(&old.objects));
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
    assert_eq!(new.adopt_newer_positions(&old).unwrap(), 1);
    let moved = new.reading_position(paged.entity_id, source.id).unwrap();
    assert_eq!(moved.location, Some(at(1)));
    assert_eq!(moved.device, Some(phone));
    // 二度目は何もしない。新しい写しのほうが新しい位置も変えない。
    assert_eq!(new.adopt_newer_positions(&old).unwrap(), 0);
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
    assert_eq!(new.adopt_newer_positions(&old).unwrap(), 0);
    assert_eq!(
        new.reading_position(paged.entity_id, source.id)
            .unwrap()
            .location,
        Some(at(0))
    );
    let _ = other;
    new.verify().unwrap();
}
