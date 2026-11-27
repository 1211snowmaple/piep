use super::*;
use crate::library::{Capture, EntityKind};
use std::collections::BTreeMap;

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp() -> Temp {
    let root = std::env::temp_dir().join(format!("piep-shelf-backup-{}", LibraryId::now_v7()));
    std::fs::create_dir_all(&root).unwrap();
    Temp(root)
}

fn save(library: &Library, key: &str, title: &str) {
    library
        .capture(Capture {
            acquired_at: None,
            request_id: LibraryId::now_v7(),
            provider: "local".into(),
            external_key: key.into(),
            kind: EntityKind::Work,
            route: "test".into(),
            raw: b"{}".to_vec().into(),
            metadata: BTreeMap::from([("title".into(), serde_json::json!(title))]),
            field_states: BTreeMap::new(),
            body: Some("本文".into()),
            media: vec![],
            attachments: vec![],
        })
        .unwrap();
}

fn titles(path: &Path) -> Vec<String> {
    let library = Library::open(path).unwrap();
    let mut titles: Vec<String> = library
        .works(10)
        .unwrap()
        .into_iter()
        .map(|work| work.title)
        .collect();
    titles.sort();
    titles
}

#[test]
fn a_backup_is_inspected_and_a_scheduled_restore_swaps_the_shelf_and_keeps_the_old_one() {
    let temp = temp();
    let live = temp.0.join("shelf");
    let backup = temp.0.join("backup");
    let id = {
        let library = Library::create(&live).unwrap();
        save(&library, "one", "残す本");
        library.backup(&backup).unwrap();
        save(&library, "two", "あとで足した本");
        library.id
    };

    let inspection = inspect(&backup, Some(id)).unwrap();
    assert_eq!(inspection.works, 1);
    assert!(inspection.same_library);
    assert!(
        !inspect(&backup, Some(LibraryId::now_v7()))
            .unwrap()
            .same_library
    );
    assert!(inspect(&temp.0, None).is_err(), "棚でないフォルダーは断る");

    write_json(&request_path(&live), &RestoreRequest::new(backup.clone())).unwrap();
    recover_pending_restore(&live).unwrap();
    assert!(!request_path(&live).exists(), "予約は一度きり");
    let outcome: RestoreOutcome =
        serde_json::from_slice(&std::fs::read(outcome_path(&live)).unwrap()).unwrap();
    assert_eq!(outcome.error, None);
    let previous = outcome.previous.unwrap();
    assert_eq!(titles(&live), vec!["残す本".to_string()]);
    assert_eq!(
        titles(&previous),
        vec!["あとで足した本".to_string(), "残す本".to_string()]
    );
    // 写しは手元に残る。
    assert_eq!(titles(&backup), vec!["残す本".to_string()]);
    Library::open(&live).unwrap().verify().unwrap();
}

#[test]
fn startup_resumes_restore_after_each_disk_change() {
    for had_live in [true, false] {
        for stopped_after in 0..=7 {
            let temp = temp();
            let live = temp.0.join("shelf");
            let backup = temp.0.join("backup");
            if had_live {
                let library = Library::create(&live).unwrap();
                save(&library, "old", "今の本");
            }
            {
                let library = Library::create(&backup).unwrap();
                save(&library, "restored", "写しの本");
            }
            let mut pending = RestoreRequest::new(backup.clone());
            let request = request_path(&live);
            let stage = pending.stage(&live);
            let previous = pending.previous(&live);
            let archived_backup = temp.0.join("offline-backup");
            let partial = temp
                .0
                .join(format!(".piep-backup-{}.part", LibraryId::now_v7()));
            write_json(&request, &pending).unwrap();
            if stopped_after == 1 {
                // 写しの作成中に止まった。
                fs::create_dir(&partial).unwrap();
                fs::write(partial.join("library.db"), b"incomplete").unwrap();
            }
            if stopped_after >= 2 {
                prepare_restore(&pending, &stage).unwrap();
                // 写しができた後は、選んだバックアップをもう一度読めなくても続けられる。
                fs::rename(&backup, &archived_backup).unwrap();
            }
            if stopped_after >= 3 {
                pending.prepared = true;
                write_json(&request, &pending).unwrap();
            }
            if stopped_after >= 4 && had_live {
                fs::rename(&live, &previous).unwrap();
            }
            if stopped_after >= 5 {
                fs::rename(&stage, &live).unwrap();
            }
            let recorded = RestoreOutcome {
                id: pending.id,
                backup: backup.clone(),
                previous: had_live.then(|| previous.clone()),
                error: None,
                finished_at: 1,
            };
            if stopped_after >= 6 {
                write_json(&outcome_path(&live), &recorded).unwrap();
            }
            if stopped_after >= 7 {
                fs::remove_file(&request).unwrap();
            }

            let checkpoint = format!("元の棚あり={had_live}、停止箇所={stopped_after}");
            recover_pending_restore(&live).expect(&checkpoint);
            assert!(
                !request.exists() && !stage.exists() && !partial.exists(),
                "{checkpoint}"
            );
            let outcome: RestoreOutcome =
                serde_json::from_slice(&fs::read(outcome_path(&live)).unwrap()).unwrap();
            assert_eq!(outcome.error, None, "{checkpoint}");
            assert_eq!(outcome.previous, recorded.previous, "{checkpoint}");
            assert_eq!(titles(&live), ["写しの本"], "{checkpoint}");
            Library::open(&live).unwrap().verify().unwrap();
            if had_live {
                assert_eq!(titles(&previous), ["今の本"], "{checkpoint}");
                Library::open(&previous).unwrap().verify().unwrap();
            }
            if stopped_after >= 2 {
                fs::rename(&archived_backup, &backup).unwrap();
            }
            assert_eq!(titles(&backup), ["写しの本"], "{checkpoint}");
            if stopped_after >= 6 {
                assert_eq!(outcome, recorded, "{checkpoint}");
            }
            recover_pending_restore(&live).unwrap();
            let outcome_after_retry: RestoreOutcome =
                serde_json::from_slice(&fs::read(outcome_path(&live)).unwrap()).unwrap();
            assert_eq!(outcome_after_retry, outcome, "{checkpoint}");
        }
    }
}

#[test]
fn startup_keeps_the_reservation_if_the_library_cannot_be_put_back_in_place() {
    let temp = temp();
    let live = temp.0.join("shelf");
    let pending = RestoreRequest::new(temp.0.join("missing"));
    let request = request_path(&live);
    write_json(&request, &pending).unwrap();

    assert!(recover_pending_restore(&live).is_err());
    assert_eq!(read_request(&request).unwrap(), Some(pending));
    assert!(!live.exists());
}

#[test]
fn a_broken_backup_leaves_the_shelf_in_place() {
    let temp = temp();
    let live = temp.0.join("shelf");
    {
        let library = Library::create(&live).unwrap();
        save(&library, "one", "今の本");
    }
    write_json(
        &request_path(&live),
        &RestoreRequest::new(temp.0.join("missing")),
    )
    .unwrap();
    recover_pending_restore(&live).unwrap();
    let outcome: RestoreOutcome =
        serde_json::from_slice(&std::fs::read(outcome_path(&live)).unwrap()).unwrap();
    assert!(outcome.error.is_some());
    assert_eq!(outcome.previous, None);
    assert!(!request_path(&live).exists());
    assert_eq!(titles(&live), vec!["今の本".to_string()]);
}

#[test]
fn an_author_archive_is_inspected_as_a_part_and_never_swapped_in_for_the_shelf() {
    let temp = temp();
    let live = temp.0.join("shelf");
    let archive = temp.0.join("archive");
    let id = {
        let library = Library::create(&live).unwrap();
        for (key, author) in [("1", "a"), ("2", "a"), ("3", "b")] {
            library
                .capture(Capture {
                    acquired_at: None,
                    request_id: LibraryId::now_v7(),
                    provider: "local".into(),
                    external_key: key.into(),
                    kind: EntityKind::Work,
                    route: "test".into(),
                    raw: b"{}".to_vec().into(),
                    metadata: BTreeMap::from([
                        ("title".into(), serde_json::json!(format!("本 {key}"))),
                        ("author_key".into(), serde_json::json!(author)),
                    ]),
                    field_states: BTreeMap::new(),
                    body: Some("本文".into()),
                    media: vec![],
                    attachments: vec![],
                })
                .unwrap();
        }
        let author = library
            .entity_by_key(EntityKind::Person, "local", "a")
            .unwrap()
            .unwrap();
        let works = library.entity_works(EntityKind::Person, author).unwrap();
        assert_eq!(works.len(), 2);
        library.copy_works(&works, &archive).unwrap();
        library.id
    };
    let inspection = inspect(&archive, Some(id)).unwrap();
    assert!(inspection.partial);
    assert_eq!(inspection.works, 2);
    assert!(inspection.same_library);
    // 予約が書かれても、部分写しで棚を入れ替えない。
    write_json(&request_path(&live), &RestoreRequest::new(archive.clone())).unwrap();
    recover_pending_restore(&live).unwrap();
    let outcome: RestoreOutcome =
        serde_json::from_slice(&std::fs::read(outcome_path(&live)).unwrap()).unwrap();
    assert!(outcome.error.is_some());
    assert_eq!(titles(&live).len(), 3);
}

#[test]
fn an_archive_folder_gets_a_safe_unused_name() {
    let temp = temp();
    assert_eq!(
        unused_folder(&temp.0, "作者: a/b").unwrap(),
        temp.0.join("作者_ a_b")
    );
    std::fs::create_dir(temp.0.join("作者")).unwrap();
    assert_eq!(
        unused_folder(&temp.0, "作者").unwrap(),
        temp.0.join("作者 (2)")
    );
    assert_eq!(
        unused_folder(&temp.0, " .. ").unwrap(),
        temp.0.join("piep-archive")
    );
}
