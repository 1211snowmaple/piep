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
    assert!(!inspection.switched);
    assert!(
        !inspect(&backup, Some(LibraryId::now_v7()))
            .unwrap()
            .same_library
    );
    assert!(inspect(&temp.0, None).is_err(), "棚でないフォルダーは断る");

    write_json(
        &request_path(&live),
        &RestoreRequest {
            backup: backup.clone(),
            requested_at: 1,
        },
    )
    .unwrap();
    apply_pending_restore(&live);
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
fn a_broken_backup_leaves_the_shelf_in_place() {
    let temp = temp();
    let live = temp.0.join("shelf");
    {
        let library = Library::create(&live).unwrap();
        save(&library, "one", "今の本");
    }
    write_json(
        &request_path(&live),
        &RestoreRequest {
            backup: temp.0.join("missing"),
            requested_at: 1,
        },
    )
    .unwrap();
    apply_pending_restore(&live);
    let outcome: RestoreOutcome =
        serde_json::from_slice(&std::fs::read(outcome_path(&live)).unwrap()).unwrap();
    assert!(outcome.error.is_some());
    assert_eq!(outcome.previous, None);
    assert!(!request_path(&live).exists());
    assert_eq!(titles(&live), vec!["今の本".to_string()]);
}
