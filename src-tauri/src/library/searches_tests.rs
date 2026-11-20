use super::*;
use serde_json::json;
use std::path::PathBuf;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("piep-search-state-{}", LibraryId::now_v7())))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn definition(name: &str) -> SavedSearchDefinition {
    SavedSearchDefinition {
        name: name.into(),
        query: Some("本文の言葉".into()),
        params_json:
            "{ \"tab\":\"works\",\"future\":9007199254740993,\"number\":0.07221960411257732 }\r\n"
                .into(),
    }
}
fn update(
    library: &Library,
    request_id: LibraryId,
    id: LibraryId,
    user: LibraryId,
    expected_generation: i64,
    definition: Option<SavedSearchDefinition>,
) -> Result<SavedSearch> {
    library.update_saved_search(SavedSearchUpdate {
        request_id,
        id,
        user,
        expected_generation,
        definition,
    })
}
fn generation(library: &Library) -> i64 {
    library
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT generation FROM library_meta", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn searches_keep_exact_conditions_and_isolate_owners_and_name_collisions() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let user = LibraryId::now_v7();
    let other = LibraryId::now_v7();
    let id = LibraryId::now_v7();
    let first = update(
        &library,
        LibraryId::now_v7(),
        id,
        user,
        0,
        Some(definition("検索")),
    )
    .unwrap();
    assert_eq!(first.definition, definition("検索"));
    assert_eq!(first.position, 0);
    assert_eq!(first.generation, 1);
    assert!(library.saved_search(other, id).is_err());
    assert!(library.saved_searches(other).unwrap().is_empty());
    let before = generation(&library);
    assert!(update(
        &library,
        LibraryId::now_v7(),
        id,
        other,
        0,
        Some(definition("別の所有者"))
    )
    .is_err());
    assert!(update(
        &library,
        LibraryId::now_v7(),
        LibraryId::now_v7(),
        user,
        0,
        Some(definition("検索"))
    )
    .is_err());
    assert_eq!(generation(&library), before);
    assert_eq!(library.saved_search(user, id).unwrap(), first);
    let another = update(
        &library,
        LibraryId::now_v7(),
        LibraryId::now_v7(),
        other,
        0,
        Some(definition("検索")),
    )
    .unwrap();
    assert_eq!(library.saved_searches(other).unwrap(), vec![another]);
    assert!(library.works(10).unwrap().is_empty());
    library.verify().unwrap();
}

#[test]
fn retry_receipts_never_rewind_later_updates_or_deletions_and_names_can_be_reused() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let user = library.id;
    let id = LibraryId::now_v7();
    let request = LibraryId::now_v7();
    let first = update(&library, request, id, user, 0, Some(definition("検索"))).unwrap();
    let renamed = update(
        &library,
        LibraryId::now_v7(),
        id,
        user,
        1,
        Some(definition("新しい名前")),
    )
    .unwrap();
    assert_eq!(renamed.position, first.position);
    assert_eq!(renamed.created_at, first.created_at);
    assert_eq!(
        update(&library, request, id, user, 0, Some(definition("検索"))).unwrap(),
        first
    );
    assert_eq!(library.saved_search(user, id).unwrap(), renamed);
    assert!(update(&library, request, id, user, 0, Some(definition("不一致"))).is_err());
    assert!(update(&library, LibraryId::now_v7(), id, user, 1, None).is_err());
    let deleted = update(&library, LibraryId::now_v7(), id, user, 2, None).unwrap();
    assert_eq!(deleted.generation, 3);
    assert!(deleted.deleted_at.is_some());
    assert_eq!(deleted.definition, renamed.definition);
    assert!(library.saved_searches(user).unwrap().is_empty());
    let next = update(
        &library,
        LibraryId::now_v7(),
        LibraryId::now_v7(),
        user,
        0,
        Some(definition("新しい名前")),
    )
    .unwrap();
    assert_eq!(next.position, 1);
    assert!(update(
        &library,
        LibraryId::now_v7(),
        id,
        user,
        3,
        Some(definition("新しい名前"))
    )
    .is_err());
    assert_eq!(library.saved_search(user, id).unwrap(), deleted);
    assert_eq!(
        update(&library, request, id, user, 0, Some(definition("検索"))).unwrap(),
        first
    );
    assert_eq!(library.saved_search(user, id).unwrap(), deleted);
    library.backup(&temp.0.with_extension("backup")).unwrap();
    let backup_path = temp.0.with_extension("backup");
    let backup = Library::open(&backup_path).unwrap();
    assert_eq!(backup.saved_search(user, id).unwrap(), deleted);
    assert_eq!(backup.saved_searches(user).unwrap(), vec![next]);
    backup.verify().unwrap();
    drop(backup);
    assert_eq!(backup_path.parent(), Some(std::env::temp_dir().as_path()));
    std::fs::remove_dir_all(backup_path).unwrap();
}

#[test]
fn failed_publication_and_foreign_receipts_roll_back_the_search_and_library_generation() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let user = library.id;
    let id = LibraryId::now_v7();
    let request = LibraryId::now_v7();
    library.conn.lock().unwrap().execute_batch("CREATE TRIGGER refuse_search BEFORE INSERT ON outbox WHEN NEW.operation='saved_search' BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
    assert!(update(&library, request, id, user, 0, Some(definition("検索"))).is_err());
    assert!(library.saved_searches(user).unwrap().is_empty());
    assert_eq!(generation(&library), 0);
    library
        .conn
        .lock()
        .unwrap()
        .execute_batch("DROP TRIGGER refuse_search;")
        .unwrap();
    let saved = update(&library, request, id, user, 0, Some(definition("検索"))).unwrap();
    assert_eq!(saved.generation, 1);
    let nonce = LibraryId::now_v7();
    let entity = LibraryId::now_v7();
    {
        let conn = library.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO entities(id,kind,created_at) VALUES(?1,'work',0)",
            [entity.as_bytes().as_slice()],
        )
        .unwrap();
        conn.execute("INSERT INTO outbox(request_id,entity_id,operation,payload_json,created_at) VALUES(?1,?2,'favorite','{}',0)",params![nonce.as_bytes().as_slice(),entity.as_bytes().as_slice()]).unwrap();
    }
    assert!(update(&library, nonce, id, user, 1, None).is_err());
    assert_eq!(library.saved_search(user, id).unwrap(), saved);
    assert_eq!(generation(&library), 1);
    library.verify().unwrap();
}

#[test]
fn sql_rejects_cross_owner_outbox_and_physical_deletion_or_generation_reuse() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let user = library.id;
    let id = LibraryId::now_v7();
    update(
        &library,
        LibraryId::now_v7(),
        id,
        user,
        0,
        Some(definition("検索")),
    )
    .unwrap();
    let conn = library.conn.lock().unwrap();
    assert!(conn
        .execute(
            "UPDATE saved_searches SET name='changed' WHERE id=?1",
            [id.as_bytes().as_slice()]
        )
        .is_err());
    assert!(conn
        .execute(
            "DELETE FROM saved_searches WHERE id=?1",
            [id.as_bytes().as_slice()]
        )
        .is_err());
    assert!(conn
        .execute(
            "UPDATE saved_searches SET user_id=?2,generation=generation+1 WHERE id=?1",
            params![
                id.as_bytes().as_slice(),
                LibraryId::now_v7().as_bytes().as_slice()
            ]
        )
        .is_err());
    let insert="INSERT INTO outbox(request_id,saved_search_id,user_id,operation,payload_json,created_at) VALUES(?1,?2,?3,'saved_search','{}',0)";
    assert!(conn
        .execute(
            insert,
            params![
                LibraryId::now_v7().as_bytes().as_slice(),
                id.as_bytes().as_slice(),
                LibraryId::now_v7().as_bytes().as_slice()
            ]
        )
        .is_err());
    assert!(conn.execute("INSERT INTO outbox(request_id,saved_search_id,operation,payload_json,created_at) VALUES(?1,?2,'saved_search','{}',0)",params![LibraryId::now_v7().as_bytes().as_slice(),id.as_bytes().as_slice()]).is_err());
}

#[test]
fn invalid_definitions_are_rejected_without_truncation_and_audit_detects_an_invalid_date() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let user = library.id;
    let mut cases = Vec::new();
    let mut bad = definition("bad");
    bad.query = Some("字".repeat(513));
    cases.push(bad);
    cases.push(definition(&"字".repeat(81)));
    let mut bad = definition("bad");
    bad.params_json = format!("{{\"large\":\"{}\"}}", "a".repeat(16 * 1024));
    cases.push(bad);
    for bad in cases {
        assert!(update(
            &library,
            LibraryId::now_v7(),
            LibraryId::now_v7(),
            user,
            0,
            Some(bad)
        )
        .is_err());
    }
    assert_eq!(generation(&library), 0);
    assert!(library.saved_searches(user).unwrap().is_empty());
    let id = LibraryId::now_v7();
    update(
        &library,
        LibraryId::now_v7(),
        id,
        user,
        0,
        Some(definition("検索")),
    )
    .unwrap();
    library
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE saved_searches SET updated_at=?2,generation=generation+1 WHERE id=?1",
            params![id.as_bytes().as_slice(), i64::MAX],
        )
        .unwrap();
    assert!(matches!(library.verify(), Err(Error::Integrity(_))));
}

#[test]
fn limits_apply_per_user_and_deletion_frees_a_slot_without_discarding_the_tombstone() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let user = library.id;
    for n in 0..100 {
        update(
            &library,
            LibraryId::now_v7(),
            LibraryId::now_v7(),
            user,
            0,
            Some(definition(&n.to_string())),
        )
        .unwrap();
    }
    assert!(update(
        &library,
        LibraryId::now_v7(),
        LibraryId::now_v7(),
        user,
        0,
        Some(definition("overflow"))
    )
    .is_err());
    let first = library.saved_searches(user).unwrap().remove(0);
    update(&library, LibraryId::now_v7(), first.id, user, 1, None).unwrap();
    update(
        &library,
        LibraryId::now_v7(),
        LibraryId::now_v7(),
        user,
        0,
        Some(definition("next")),
    )
    .unwrap();
    assert!(update(
        &library,
        LibraryId::now_v7(),
        first.id,
        user,
        2,
        Some(definition("restore"))
    )
    .is_err());
    assert!(library
        .saved_search(user, first.id)
        .unwrap()
        .deleted_at
        .is_some());
    for invalid in [json!([]), json!(null)] {
        let mut bad = definition("bad");
        bad.params_json = invalid.to_string();
        assert!(update(
            &library,
            LibraryId::now_v7(),
            LibraryId::now_v7(),
            LibraryId::now_v7(),
            0,
            Some(bad)
        )
        .is_err());
    }
    library.verify().unwrap();
}
