use super::*;
use crate::library::{Capture, CollectionMember as SavedMember, CollectionSave, EntityKind};
use serde_json::json;
use std::{collections::BTreeMap, fs, path::PathBuf};

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp() -> Temp {
    Temp(std::env::temp_dir().join(format!("piep-collections-{}", LibraryId::now_v7())))
}

fn save_work(library: &Library, key: &str, title: &str, author: &str, body: &str) -> LibraryId {
    library
        .capture(Capture {
            acquired_at: Some(1_000),
            request_id: LibraryId::now_v7(),
            provider: "pixiv".into(),
            external_key: key.into(),
            kind: EntityKind::Work,
            route: "test".into(),
            raw: serde_json::to_vec(&json!({"key": key})).unwrap().into(),
            metadata: serde_json::from_value::<BTreeMap<String, serde_json::Value>>(json!({
                "title": title,
                "author_name": author,
                "author_key": author,
                "create_date": "2024-01-01T00:00:00Z",
            }))
            .unwrap(),
            field_states: BTreeMap::new(),
            body: Some(body.into()),
            media: vec![],
            attachments: vec![],
        })
        .unwrap();
    library
        .entity_by_key(EntityKind::Work, "pixiv", key)
        .unwrap()
        .unwrap()
}

fn member(work: LibraryId, title: &str) -> SavedMember {
    SavedMember {
        work_id: work,
        title_at_add: title.into(),
        author_at_add: "足した時の作者".into(),
        role: "main".into(),
        added_by: "manual".into(),
        pinned: false,
        note: Some("覚え書き".into()),
    }
}

fn save(library: &Library, title: &str, ordered: bool, members: Vec<SavedMember>) -> LibraryId {
    let id = LibraryId::now_v7();
    library
        .save_collection(CollectionSave {
            request_id: LibraryId::now_v7(),
            id,
            expected_generation: 0,
            title: title.into(),
            description: None,
            ordered,
            members,
        })
        .unwrap();
    id
}

#[test]
fn every_collection_is_listed_newest_change_first() {
    let temp = temp();
    let library = Library::create(&temp.0).unwrap();
    let work = save_work(&library, "w1", "題", "作者", "本文");
    let first = save(&library, "一つ目", true, vec![member(work, "題")]);
    let second = save(&library, "二つ目", false, vec![]);
    let listed: Vec<LibraryId> = library
        .list_collections()
        .unwrap()
        .into_iter()
        .map(|summary| summary.id)
        .collect();
    assert_eq!(listed.len(), 2);
    assert!(listed.contains(&first) && listed.contains(&second));
    // 変えたものが先頭に来る。
    save_again(&library, first);
    assert_eq!(library.list_collections().unwrap()[0].id, first);
}

fn save_again(library: &Library, id: LibraryId) {
    let generation = library.collection_detail(id).unwrap().summary.revision;
    std::thread::sleep(std::time::Duration::from_millis(5));
    library
        .save_collection(CollectionSave {
            request_id: LibraryId::now_v7(),
            id,
            expected_generation: generation,
            title: "一つ目（改）".into(),
            description: None,
            ordered: true,
            members: vec![],
        })
        .unwrap();
}

#[test]
fn a_collection_lists_its_works_in_order_with_their_cards_and_other_editions() {
    let temp = temp();
    let library = Library::create(&temp.0).unwrap();
    let w1 = save_work(&library, "w1", "前編", "作者", "本文");
    let w2 = save_work(&library, "w2", "後編", "作者", "本文本文");
    // 同じ作者が同じ題で出した別の作品（コレクションには入っていない）は、別版として畳む。
    let other = save_work(&library, "w3", "後編", "作者", "本文本文本文");
    // 別の作者の同じ題は別版ではない。
    save_work(&library, "w4", "後編", "別人", "本文");
    let id = save(
        &library,
        "束",
        true,
        vec![member(w2, "後編"), member(w1, "前編")],
    );
    let detail = library.collection_detail(id).unwrap();
    assert_eq!(detail.summary.name, "束");
    assert_eq!(detail.summary.member_count, 2);
    let keys: Vec<&str> = detail
        .members
        .iter()
        .map(|member| member.external_key.as_str())
        .collect();
    assert_eq!(keys, vec!["w2", "w1"]);
    let first = &detail.members[0];
    assert!(!first.missing);
    assert_eq!(first.title, "後編");
    assert_eq!(first.author_name, "作者");
    assert_eq!(first.text_length, 4);
    assert_eq!(first.note.as_deref(), Some("覚え書き"));
    assert_eq!(first.work.as_ref().map(|card| card.id), Some(w2));
    let editions: Vec<LibraryId> = first.editions.iter().map(|card| card.id).collect();
    assert_eq!(editions, vec![other]);
    assert!(detail.members[1].editions.is_empty());
}

#[test]
fn an_unknown_collection_is_unavailable_and_an_unmigrated_legacy_id_finds_nothing() {
    let temp = temp();
    let library = Library::create(&temp.0).unwrap();
    assert!(matches!(
        library.collection_detail(LibraryId::now_v7()),
        Err(Error::ResourceUnavailable)
    ));
    assert_eq!(
        library.collection_by_legacy_id("collection-old").unwrap(),
        None
    );
}
