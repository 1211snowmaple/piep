use super::*;
use crate::library::{Capture, SortValue, WorkCursor, WorkSort};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, path::PathBuf};

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp() -> Temp {
    Temp(std::env::temp_dir().join(format!("piep-entity-{}", LibraryId::now_v7())))
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

/// 作者 p1 の作品 w1〜w4（w1・w2・w3 はシリーズ s1、w4 は s2）と、別の作者 p2 の作品 w5（s1）。
fn shelf(library: &Library) {
    let work = |key: &str,
                author: &str,
                created: &str,
                series: Option<(&str, Option<i64>)>,
                tags: &[&str]| {
        let mut metadata = json!({
            "title": key,
            "author_name": format!("{author} の名"),
            "author_key": author,
            "create_date": created,
            "tags": tags,
        });
        if let Some((id, position)) = series {
            metadata["series_id"] = json!(id);
            metadata["series_title"] = json!(format!("{id} の題"));
            if let Some(position) = position {
                metadata["series_position"] = json!(position);
            }
        }
        library
            .capture(capture(EntityKind::Work, key, metadata, Some("本文")))
            .unwrap();
    };
    work(
        "w1",
        "p1",
        "2024-01-01T00:00:00Z",
        Some(("s1", Some(2))),
        &["a", "b"],
    );
    work(
        "w2",
        "p1",
        "2025-01-01T00:00:00Z",
        Some(("s1", Some(1))),
        &["b"],
    );
    // 話数のない回は、話数のある回の後ろ。
    work(
        "w3",
        "p1",
        "2023-01-01T00:00:00Z",
        Some(("s1", None)),
        &["c", "b"],
    );
    work(
        "w4",
        "p1",
        "2022-01-01T00:00:00Z",
        Some(("s2", Some(1))),
        &["a"],
    );
    work(
        "w5",
        "p2",
        "2021-01-01T00:00:00Z",
        Some(("s1", Some(3))),
        &["z"],
    );
    library
        .capture(capture(
            EntityKind::Person,
            "p1",
            json!({"display_name": "作者一", "description": "紹介", "links": ["https://example.test/a"]}),
            None,
        ))
        .unwrap();
    library
        .capture(capture(
            EntityKind::Series,
            "s1",
            json!({"title": "続き物", "description": "あらすじ", "is_concluded": true, "published_content_count": 4}),
            None,
        ))
        .unwrap();
}

#[test]
fn the_head_of_an_author_and_a_series_reads_their_profile_and_counts_their_works() {
    let temp = temp();
    let library = Library::create(&temp.0).unwrap();
    shelf(&library);
    let person = library
        .entity_head(EntityKind::Person, "pixiv", "p1")
        .unwrap()
        .unwrap();
    assert_eq!(person.kind, "person");
    assert_eq!(person.name, "作者一");
    assert_eq!(person.description.as_deref(), Some("紹介"));
    assert_eq!(person.links, vec!["https://example.test/a".to_string()]);
    assert_eq!(person.work_count, 4);
    assert_eq!(person.version_number, Some(1));
    assert_eq!(person.versions.len(), 1);
    assert!(person.snapshot.is_some());
    let series = library
        .entity_head(EntityKind::Series, "pixiv", "s1")
        .unwrap()
        .unwrap();
    assert_eq!(series.name, "続き物");
    assert_eq!(series.is_concluded, Some(true));
    assert_eq!(series.published_content_count, Some(4));
    // 別の作者の回も数える。
    assert_eq!(series.work_count, 4);
    assert!(library
        .entity_head(EntityKind::Person, "pixiv", "missing")
        .unwrap()
        .is_none());
}

#[test]
fn an_author_known_only_from_works_takes_the_name_on_the_works() {
    let temp = temp();
    let library = Library::create(&temp.0).unwrap();
    shelf(&library);
    let person = library
        .entity_head(EntityKind::Person, "pixiv", "p2")
        .unwrap()
        .unwrap();
    assert_eq!(person.name, "p2 の名");
    assert_eq!(person.snapshot, None);
    assert_eq!(person.version_number, None);
    assert!(person.versions.is_empty());
    assert_eq!(person.work_count, 1);
    let series = library
        .entity_head(EntityKind::Series, "pixiv", "s2")
        .unwrap()
        .unwrap();
    assert_eq!(series.name, "s2 の題");
}

#[test]
fn tags_are_counted_by_work_most_first_then_by_name() {
    let temp = temp();
    let library = Library::create(&temp.0).unwrap();
    shelf(&library);
    let person = library
        .entity_by_key(EntityKind::Person, "pixiv", "p1")
        .unwrap()
        .unwrap();
    let tags: Vec<(String, u64)> = library
        .entity_tags(EntityKind::Person, person, 10)
        .unwrap()
        .into_iter()
        .map(|tag| (tag.name, tag.count))
        .collect();
    assert_eq!(
        tags,
        vec![("b".into(), 3), ("a".into(), 2), ("c".into(), 1)]
    );
    let series = library
        .entity_by_key(EntityKind::Series, "pixiv", "s1")
        .unwrap()
        .unwrap();
    let first = library.entity_tags(EntityKind::Series, series, 1).unwrap();
    assert_eq!(
        first,
        vec![TagCount {
            name: "b".into(),
            count: 3
        }]
    );
}

#[test]
fn the_series_order_follows_positions_then_dates_across_pages() {
    let temp = temp();
    let library = Library::create(&temp.0).unwrap();
    shelf(&library);
    let series = library
        .entity_by_key(EntityKind::Series, "pixiv", "s1")
        .unwrap()
        .unwrap();
    let mut query = WorkListQuery {
        sort: WorkSort::SeriesOrder,
        descending: false,
        limit: 2,
        filter: WorkFilter {
            series: Some(series),
            ..WorkFilter::default()
        },
        ..WorkListQuery::default()
    };
    let mut keys = Vec::new();
    loop {
        let page = library.list_works(&query).unwrap();
        keys.extend(page.items.iter().map(|card| card.external_key.clone()));
        match page.next {
            Some(next) => {
                assert!(matches!(next.value, SortValue::Text(_)));
                query.after = Some(next);
            }
            None => break,
        }
    }
    assert_eq!(keys, vec!["w2", "w1", "w5", "w3"]);
    // 逆順も同じ鍵で続きを引ける。
    query.descending = true;
    query.after = None;
    query.limit = 10;
    let reversed: Vec<String> = library
        .list_works(&query)
        .unwrap()
        .items
        .into_iter()
        .map(|card| card.external_key)
        .collect();
    assert_eq!(reversed, vec!["w3", "w5", "w1", "w2"]);
    // 続きの位置が数だと断る。
    query.after = Some(WorkCursor {
        value: SortValue::Number(1),
        work: series,
    });
    assert!(library.list_works(&query).is_err());
}

#[test]
fn an_authors_series_are_aggregated_filtered_sorted_and_paged() {
    let temp = temp();
    let library = Library::create(&temp.0).unwrap();
    shelf(&library);
    let person = library
        .entity_by_key(EntityKind::Person, "pixiv", "p1")
        .unwrap()
        .unwrap();
    let page = library
        .person_series(person, "", SeriesSort::SourceCreatedAt, false, 0, 10)
        .unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.next, None);
    let first = &page.items[0];
    assert_eq!(first.external_key, "s1");
    assert_eq!(first.name, "続き物");
    // この作者の回だけを数える（別の作者の w5 は入れない）。
    assert_eq!(first.work_count, 3);
    assert_eq!(first.description.as_deref(), Some("あらすじ"));
    assert_eq!(first.is_concluded, Some(true));
    assert_eq!(first.sample_title.as_deref(), Some("w2"));
    assert_eq!(first.total_text_length, 6);
    assert_eq!(page.items[1].external_key, "s2");
    // シリーズの版のない s2 は作品に載った題。
    assert_eq!(page.items[1].name, "s2 の題");
    assert_eq!(page.items[1].is_concluded, None);
    // 名前は既定で昇順、ASCII は大文字小文字を区別しない。
    let named = library
        .person_series(person, "", SeriesSort::Name, true, 0, 10)
        .unwrap();
    assert_eq!(named.items[0].external_key, "s2");
    // 作品の多い順。
    let counted = library
        .person_series(person, "", SeriesSort::WorkCount, true, 0, 10)
        .unwrap();
    assert_eq!(counted.items[0].external_key, "s2");
    // 名前か説明に語を含むもの。
    let found = library
        .person_series(person, "あらす", SeriesSort::SourceCreatedAt, false, 0, 10)
        .unwrap();
    assert_eq!(found.total, 1);
    assert_eq!(found.items[0].external_key, "s1");
    let found = library
        .person_series(person, "S2 の", SeriesSort::SourceCreatedAt, false, 0, 10)
        .unwrap();
    assert_eq!(found.items[0].external_key, "s2");
    // ページを分けても同じ並び。
    let head = library
        .person_series(person, "", SeriesSort::SourceCreatedAt, false, 0, 1)
        .unwrap();
    assert_eq!(head.next, Some(1));
    let tail = library
        .person_series(person, "", SeriesSort::SourceCreatedAt, false, 1, 1)
        .unwrap();
    assert_eq!(head.items[0].external_key, "s1");
    assert_eq!(tail.items[0].external_key, "s2");
    assert_eq!(tail.next, None);
}
