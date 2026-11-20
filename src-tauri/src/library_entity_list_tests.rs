use super::*;
use crate::library::{Capture, CollectionMember, CollectionSave, WorkFilter};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::PathBuf};

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp() -> Temp {
    Temp(std::env::temp_dir().join(format!("piep-entity-list-{}", LibraryId::now_v7())))
}

fn capture(kind: EntityKind, key: &str, metadata: Value, body: Option<&str>) -> Capture {
    Capture {
        acquired_at: Some(1_000),
        request_id: LibraryId::now_v7(),
        provider: "pixiv".into(),
        external_key: key.into(),
        kind,
        route: "test".into(),
        raw: serde_json::to_vec(&json!({"key": key})).unwrap().into(),
        metadata: serde_json::from_value::<BTreeMap<String, Value>>(metadata).unwrap(),
        field_states: BTreeMap::new(),
        body: body.map(Into::into),
        media: vec![],
        attachments: vec![],
    }
}

/// 作者 p1 の作品 w1〜w3（w1・w2 はシリーズ s1）、作者 p2 の作品 w4（s1）と w5（s2）、
/// 名前の載らない作者 p3 の作品 w6。p1 だけがプロフィールを持ち、s1 は完結している。
fn shelf(library: &Library) -> BTreeMap<&'static str, LibraryId> {
    let mut works = BTreeMap::new();
    let mut work = |key: &'static str,
                    author: &str,
                    name: &str,
                    created: &str,
                    series: Option<(&str, i64)>,
                    tags: &[&str],
                    body: &str| {
        let mut metadata = json!({
            "title": format!("{key} の題"),
            "author_name": name,
            "author_key": author,
            "create_date": created,
            "tags": tags,
        });
        if let Some((id, position)) = series {
            metadata["series_id"] = json!(id);
            metadata["series_title"] = json!(format!("{id} の題"));
            metadata["series_position"] = json!(position);
        }
        library
            .capture(capture(EntityKind::Work, key, metadata, Some(body)))
            .unwrap();
        works.insert(
            key,
            library
                .entity_by_key(EntityKind::Work, "pixiv", key)
                .unwrap()
                .unwrap(),
        );
    };
    work(
        "w1",
        "p1",
        "一の名",
        "2024-01-01T00:00:00Z",
        Some(("s1", 2)),
        &["a"],
        "本文",
    );
    work(
        "w2",
        "p1",
        "一の名",
        "2025-01-01T00:00:00Z",
        Some(("s1", 1)),
        &["b"],
        "本文本文",
    );
    work(
        "w3",
        "p1",
        "一の名",
        "2023-01-01T00:00:00Z",
        None,
        &["a"],
        "本",
    );
    work(
        "w4",
        "p2",
        "Beta",
        "2022-01-01T00:00:00Z",
        Some(("s1", 3)),
        &["a"],
        "本文",
    );
    work(
        "w5",
        "p2",
        "Beta",
        "2021-01-01T00:00:00Z",
        Some(("s2", 1)),
        &["b"],
        "本文",
    );
    work("w6", "p3", "", "2020-01-01T00:00:00Z", None, &["a"], "本文");
    library
        .capture(capture(
            EntityKind::Person,
            "p1",
            json!({"display_name": "Alpha", "description": "紹介の文"}),
            None,
        ))
        .unwrap();
    library
        .capture(capture(
            EntityKind::Series,
            "s1",
            json!({"title": "続き物", "description": "あらすじ", "is_concluded": true}),
            None,
        ))
        .unwrap();
    works
}

fn query(kind: EntityKind) -> EntityListQuery {
    EntityListQuery {
        kind,
        query: String::new(),
        filter: WorkFilter::default(),
        scope: EntityScope::default(),
        sort: SeriesSort::WorkCount,
        ascending: false,
        offset: 0,
        limit: 50,
    }
}

fn keys(page: &EntityListPage) -> Vec<&str> {
    page.items
        .iter()
        .map(|item| item.external_key.as_str())
        .collect()
}

#[test]
fn authors_are_grouped_from_named_works_and_ordered_by_their_count() {
    let temp = temp();
    let library = Library::create(&temp.0).unwrap();
    shelf(&library);
    let page = library.list_entities(&query(EntityKind::Person)).unwrap();
    // 名前の載らない作品だけの p3 は出ない。
    assert_eq!(keys(&page), vec!["p1", "p2"]);
    assert_eq!(page.total, 2);
    let first = &page.items[0];
    assert_eq!(first.name, "Alpha");
    assert_eq!(first.description.as_deref(), Some("紹介の文"));
    assert_eq!(first.work_count, 3);
    assert_eq!(first.total_text_length, 7);
    // 最も新しく公開した作品。
    assert_eq!(first.sample_title.as_deref(), Some("w2 の題"));
    assert!(first.updated_at.is_some());
    // プロフィールのない作者は作品に載った名前。
    assert_eq!(page.items[1].name, "Beta");
    assert_eq!(page.items[1].updated_at, None);
    assert_eq!(page.items[1].is_concluded, None);
}

#[test]
fn work_filters_narrow_both_the_entities_and_their_counts() {
    let temp = temp();
    let library = Library::create(&temp.0).unwrap();
    shelf(&library);
    let mut request = query(EntityKind::Person);
    request.filter.tags = vec!["a".into()];
    let page = library.list_entities(&request).unwrap();
    assert_eq!(keys(&page), vec!["p1", "p2"]);
    assert_eq!(page.items[0].work_count, 2);
    assert_eq!(page.items[1].work_count, 1);
    request.filter.tags = vec!["b".into()];
    request.filter.text_length.min = Some(4);
    let page = library.list_entities(&request).unwrap();
    assert_eq!(keys(&page), vec!["p1"]);
    assert_eq!(page.items[0].work_count, 1);
}

#[test]
fn names_and_descriptions_are_searched_without_ascii_case() {
    let temp = temp();
    let library = Library::create(&temp.0).unwrap();
    shelf(&library);
    let mut request = query(EntityKind::Person);
    request.query = "bet".into();
    assert_eq!(keys(&library.list_entities(&request).unwrap()), vec!["p2"]);
    request.query = "紹介".into();
    assert_eq!(keys(&library.list_entities(&request).unwrap()), vec!["p1"]);
    request.query = "ない語".into();
    let page = library.list_entities(&request).unwrap();
    assert!(page.items.is_empty());
    assert_eq!(page.total, 0);
}

#[test]
fn series_are_scoped_by_conclusion_and_size_and_sorted_by_name() {
    let temp = temp();
    let library = Library::create(&temp.0).unwrap();
    shelf(&library);
    let page = library.list_entities(&query(EntityKind::Series)).unwrap();
    assert_eq!(keys(&page), vec!["s1", "s2"]);
    let s1 = &page.items[0];
    // 別の作者の回も数える。
    assert_eq!(s1.work_count, 3);
    assert_eq!(s1.name, "続き物");
    assert_eq!(s1.is_concluded, Some(true));
    // 最初の回。
    assert_eq!(s1.sample_title.as_deref(), Some("w2 の題"));
    assert_eq!(page.items[1].name, "s2 の題");
    // 完結を知らない s2 は、完結にも連載中にも入らない。
    let mut request = query(EntityKind::Series);
    request.scope.concluded = Some(false);
    assert!(library.list_entities(&request).unwrap().items.is_empty());
    request.scope.concluded = Some(true);
    assert_eq!(keys(&library.list_entities(&request).unwrap()), vec!["s1"]);
    let mut request = query(EntityKind::Series);
    request.scope.min_work_count = Some(2);
    assert_eq!(keys(&library.list_entities(&request).unwrap()), vec!["s1"]);
    // 1 以下は条件なしと同じ。
    request.scope.min_work_count = Some(1);
    assert_eq!(library.list_entities(&request).unwrap().total, 2);
    let mut request = query(EntityKind::Series);
    request.sort = SeriesSort::Name;
    request.ascending = true;
    assert_eq!(
        keys(&library.list_entities(&request).unwrap()),
        vec!["s2", "s1"]
    );
}

#[test]
fn pages_split_one_order_and_the_total_does_not_depend_on_the_page() {
    let temp = temp();
    let library = Library::create(&temp.0).unwrap();
    shelf(&library);
    let mut request = query(EntityKind::Person);
    request.sort = SeriesSort::SourceCreatedAt;
    request.limit = 1;
    let head = library.list_entities(&request).unwrap();
    request.offset = 1;
    let tail = library.list_entities(&request).unwrap();
    assert_eq!(head.total, 2);
    assert_eq!(tail.total, 2);
    assert_eq!(keys(&head), vec!["p1"]);
    assert_eq!(keys(&tail), vec!["p2"]);
    request.offset = 2;
    assert!(library.list_entities(&request).unwrap().items.is_empty());
}

#[test]
fn watch_registrations_scope_the_list_and_mark_each_row() {
    let temp = temp();
    let library = Library::create(&temp.0).unwrap();
    shelf(&library);
    let p1 = library
        .entity_by_key(EntityKind::Person, "pixiv", "p1")
        .unwrap()
        .unwrap();
    library
        .conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO watch_targets(id,entity_id,display_name,enabled,created_at,updated_at) VALUES(?1,?2,'p1',0,1,1)",
            params![
                LibraryId::now_v7().as_bytes().as_slice(),
                p1.as_bytes().as_slice()
            ],
        )
        .unwrap();
    let page = library.list_entities(&query(EntityKind::Person)).unwrap();
    assert_eq!(page.items[0].watch, Some("paused"));
    assert_eq!(page.items[1].watch, None);
    let mut request = query(EntityKind::Person);
    request.scope.watch = Some(WatchScope::Paused);
    assert_eq!(keys(&library.list_entities(&request).unwrap()), vec!["p1"]);
    request.scope.watch = Some(WatchScope::Watched);
    assert!(library.list_entities(&request).unwrap().items.is_empty());
    request.scope.watch = Some(WatchScope::Unwatched);
    assert_eq!(keys(&library.list_entities(&request).unwrap()), vec!["p2"]);
}

#[test]
fn collections_holding_an_authors_or_a_series_works_are_summarised() {
    let temp = temp();
    let library = Library::create(&temp.0).unwrap();
    let works = shelf(&library);
    let member = |key: &str| CollectionMember {
        work_id: works[key],
        title_at_add: format!("{key} を足した時の題"),
        author_at_add: "足した時の作者".into(),
        role: "main".into(),
        added_by: "manual".into(),
        pinned: false,
        note: None,
    };
    let first = LibraryId::now_v7();
    library
        .save_collection(CollectionSave {
            request_id: LibraryId::now_v7(),
            id: first,
            expected_generation: 0,
            title: "一つ目".into(),
            description: Some("説明".into()),
            ordered: true,
            members: vec![member("w2"), member("w4"), member("w1")],
        })
        .unwrap();
    let second = LibraryId::now_v7();
    library
        .save_collection(CollectionSave {
            request_id: LibraryId::now_v7(),
            id: second,
            expected_generation: 0,
            title: "二つ目".into(),
            description: None,
            ordered: false,
            members: vec![member("w5")],
        })
        .unwrap();
    let p1 = library
        .entity_by_key(EntityKind::Person, "pixiv", "p1")
        .unwrap()
        .unwrap();
    let found = library.entity_collections(EntityKind::Person, p1).unwrap();
    assert_eq!(found.len(), 1);
    let summary = &found[0];
    assert_eq!(summary.id, first);
    assert_eq!(summary.name, "一つ目");
    assert!(summary.ordered);
    assert_eq!(summary.member_count, 3);
    assert_eq!(summary.available_count, 3);
    // 別の作者の回も含めた、コレクション全体の字数。
    assert_eq!(summary.total_text_length, 8);
    assert_eq!(summary.revision, 1);
    assert_eq!(summary.cover_mode, "mosaic");
    assert!(summary.updated_at >= summary.created_at);
    let tiles: Vec<&str> = summary
        .tiles
        .iter()
        .map(|tile| tile.external_key.as_str())
        .collect();
    assert_eq!(tiles, vec!["w2", "w4", "w1"]);
    assert_eq!(summary.tiles[0].title, "w2 の題");
    // シリーズ s1 は一つ目に、作者 p2 は両方に入っている。新しく変えた順。
    let s1 = library
        .entity_by_key(EntityKind::Series, "pixiv", "s1")
        .unwrap()
        .unwrap();
    assert_eq!(
        library
            .entity_collections(EntityKind::Series, s1)
            .unwrap()
            .len(),
        1
    );
    let p2 = library
        .entity_by_key(EntityKind::Person, "pixiv", "p2")
        .unwrap()
        .unwrap();
    let both: Vec<LibraryId> = library
        .entity_collections(EntityKind::Person, p2)
        .unwrap()
        .into_iter()
        .map(|summary| summary.id)
        .collect();
    assert_eq!(both.len(), 2);
    assert!(both.contains(&first) && both.contains(&second));
}
