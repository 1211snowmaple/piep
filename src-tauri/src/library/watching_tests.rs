use super::*;
use std::path::PathBuf;

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> (Temp, Library) {
    let root = std::env::temp_dir().join(format!("piep-watch-{}", LibraryId::now_v7()));
    let library = Library::create(&root).unwrap();
    (Temp(root), library)
}
fn author(key: &str) -> WatchEdit {
    WatchEdit::Upsert {
        target_type: "author".into(),
        provider: "pixiv".into(),
        key: key.into(),
        name: format!("作者{key}"),
        enabled: true,
        options: json!({}),
    }
}
fn apply(library: &Library, edit: WatchEdit) -> WatchResult {
    library
        .edit_watch(
            LibraryId::now_v7(),
            library.watch_generation().unwrap(),
            edit,
        )
        .unwrap()
}

#[test]
fn changes_are_atomic_retryable_and_keep_other_targets_and_saved_works() {
    let (_temp, library) = fixture();
    let data =
        json!({"detail":{"title":"本文のある作品","user":{"id":77,"name":"作者77"}},"text":"本文"});
    let saved = library
        .save_work(crate::library::WorkSave {
            request: LibraryId::now_v7(),
            provider: "pixiv",
            key: "11",
            route: "test",
            data: &data,
            assets: vec![],
        })
        .unwrap();
    let request = LibraryId::now_v7();
    let first = library.edit_watch(request, 0, author("77")).unwrap();
    let other = apply(&library, author("88"));
    assert_eq!(library.edit_watch(request, 0, author("77")).unwrap(), first);
    assert!(library.edit_watch(request, 0, author("different")).is_err());
    assert!(library
        .edit_watch(LibraryId::now_v7(), 0, author("99"))
        .is_err());
    let group = apply(
        &library,
        WatchEdit::CreateGroup {
            name: " 毎日 ".into(),
        },
    );
    let add = apply(
        &library,
        WatchEdit::Members {
            group: group.id,
            targets: vec![first.id, other.id, first.id],
            add: true,
        },
    );
    assert_eq!(add.changed, 2);
    let generation = library.watch_generation().unwrap();
    assert!(library
        .edit_watch(
            LibraryId::now_v7(),
            generation,
            WatchEdit::Members {
                group: group.id,
                targets: vec![first.id, LibraryId::now_v7()],
                add: false
            }
        )
        .is_err());
    assert_eq!(library.watch_groups().unwrap()[0].targets.len(), 2);
    assert_eq!(library.watch_generation().unwrap(), generation);
    assert!(library
        .edit_watch(
            LibraryId::now_v7(),
            generation,
            WatchEdit::CreateGroup {
                name: "毎日".into()
            }
        )
        .is_err());
    assert!(library
        .edit_watch(
            LibraryId::now_v7(),
            generation,
            WatchEdit::RenameGroup {
                group: group.id,
                name: " ".into()
            }
        )
        .is_err());
    assert_eq!(library.watch_generation().unwrap(), generation);
    apply(
        &library,
        WatchEdit::RenameGroup {
            group: group.id,
            name: "毎週".into(),
        },
    );
    assert_eq!(
        library
            .watch_targets()
            .unwrap()
            .iter()
            .find(|v| v.id == first.id)
            .unwrap()
            .groups,
        ["毎週"]
    );
    apply(
        &library,
        WatchEdit::Enable {
            target: first.id,
            enabled: false,
        },
    );
    assert!(
        !library
            .watch_targets()
            .unwrap()
            .iter()
            .find(|v| v.id == first.id)
            .unwrap()
            .enabled
    );
    let remove_request = LibraryId::now_v7();
    let expected = library.watch_generation().unwrap();
    let removed = library
        .edit_watch(
            remove_request,
            expected,
            WatchEdit::RemoveTarget { target: first.id },
        )
        .unwrap();
    assert_eq!(
        library
            .edit_watch(
                remove_request,
                expected,
                WatchEdit::RemoveTarget { target: first.id }
            )
            .unwrap(),
        removed
    );
    assert_eq!(library.watch_groups().unwrap()[0].targets, [other.id]);
    assert_eq!(
        library.current_snapshot(saved.entity_id).unwrap(),
        saved.snapshot_id
    );
    let recreated = apply(&library, author("77"));
    assert_ne!(recreated.id, first.id);
    library
        .record_watch_check(first.id, Some("old-response"), None, 1, false)
        .unwrap();
    assert!(library
        .watch_targets()
        .unwrap()
        .iter()
        .find(|v| v.id == recreated.id)
        .unwrap()
        .seen_key
        .is_none());
    apply(&library, WatchEdit::RemoveGroup { group: group.id });
    assert!(library.watch_groups().unwrap().is_empty());
    assert_eq!(library.watch_targets().unwrap().len(), 2);
    library.verify().unwrap();
}

#[test]
fn check_results_preserve_settings_and_existing_checkpoint_when_nothing_new_is_seen() {
    let (_temp, library) = fixture();
    let target = apply(&library, author("77"));
    let generation = library.watch_generation().unwrap();
    library
        .record_watch_check(target.id, Some("newest"), Some(1234), 2, false)
        .unwrap();
    library
        .record_watch_check(target.id, None, None, 0, true)
        .unwrap();
    let failed = library.watch_targets().unwrap().pop().unwrap();
    assert_eq!(failed.errors, 1);
    assert_eq!(failed.seen_key.as_deref(), Some("newest"));
    assert_eq!(failed.seen_updated_at, Some(1234));
    assert!(failed.hit_at.is_some());
    library
        .record_watch_check(target.id, None, None, 0, false)
        .unwrap();
    let checked = library.watch_targets().unwrap().pop().unwrap();
    assert_eq!(checked.errors, 0);
    assert_eq!(checked.seen_key, failed.seen_key);
    assert!(checked.enabled);
    assert_eq!(library.watch_generation().unwrap(), generation);
    assert!(library
        .record_watch_check(target.id, None, Some(i64::MAX), 0, false)
        .is_err());
    library.verify().unwrap();
}

#[test]
fn watch_receipts_are_catalog_owned_and_cannot_be_reused_for_other_operations() {
    let (_temp, library) = fixture();
    let target = apply(&library, author("77"));
    let conn = library.conn.lock().unwrap();
    let row: (Option<Vec<u8>>, Option<i64>, String) = conn
        .query_row(
            "SELECT entity_id,watch_catalog_id,operation FROM outbox WHERE request_id=?1",
            [target.id.as_bytes().as_slice()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(row, (None, Some(1), "watch".into()));
    assert!(conn.execute("INSERT INTO outbox(request_id,operation,payload_json,created_at) VALUES(?1,'watch','{}',0)",[LibraryId::now_v7().as_bytes().as_slice()]).is_err());
    drop(conn);
    let group = apply(
        &library,
        WatchEdit::CreateGroup {
            name: "全員".into(),
        },
    );
    let expected = library.watch_generation().unwrap();
    let request = LibraryId::now_v7();
    let change = WatchEdit::RemoveGroup { group: group.id };
    let once = library
        .edit_watch(request, expected, change.clone())
        .unwrap();
    assert_eq!(library.edit_watch(request, expected, change).unwrap(), once);
    assert_eq!(
        library.watch_request_expected(request).unwrap(),
        Some(expected)
    );
    assert!(library
        .edit_watch(
            request,
            expected,
            WatchEdit::CreateGroup {
                name: "全員".into()
            }
        )
        .is_err());
}

/// 開発版で手で確かめる用。空の棚を作り、場所だけ表示する。利用者の棚へは触らない。
#[test]
#[ignore]
fn watching_fixture() {
    let root = std::env::temp_dir().join(format!("piep-watch-fixture-{}", LibraryId::now_v7()));
    Library::create(&root).unwrap();
    println!("WATCH_FIXTURE={}", root.display());
}
