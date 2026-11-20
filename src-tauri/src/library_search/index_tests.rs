use super::super::TextSearch;
use super::*;
use crate::library::{Capture, EntityKind, TagProvenance, TagUpdate, UserTag, WorkListQuery};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        crate::database::tantivy_index::release_runtime(&self.0.join("derived"));
        crate::database::tantivy_index::release_runtime(&self.0.join("derived").join("history"));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn capture(key: &str, title: &str, body: &str, acquired: i64) -> Capture {
    Capture {
        acquired_at: Some(acquired),
        request_id: LibraryId::now_v7(),
        provider: "pixiv".into(),
        external_key: key.into(),
        kind: EntityKind::Work,
        route: "test".into(),
        raw: serde_json::to_vec(&json!({"key": key, "body": body}))
            .unwrap()
            .into(),
        metadata: BTreeMap::from([
            ("title".into(), json!(title)),
            ("author_name".into(), json!("作者")),
            ("caption".into(), json!("")),
            ("tags".into(), json!(["物語"])),
        ]),
        field_states: BTreeMap::new(),
        body: Some(body.into()),
        media: vec![],
        attachments: vec![],
    }
}

fn found(library: &Library, query: &str) -> Vec<LibraryId> {
    library
        .search_text(query)
        .unwrap()
        .into_iter()
        .map(|hit| hit.work)
        .collect()
}

#[test]
fn a_word_does_not_match_across_the_border_of_two_tags_through_their_readings() {
    let temp = Temp(std::env::temp_dir().join(format!("piep-search-tags-{}", LibraryId::now_v7())));
    let library = Library::create(&temp.0).unwrap();
    let mut tagged = capture("1", "一", "本文", 1_000);
    tagged.metadata.insert("tags".into(), json!(["雨", "雪"]));
    let work = library.capture(tagged).unwrap().entity_id;
    library.sync_search_index(&mut |_, _| {}).unwrap();
    // 一つのタグの読みには当たる。
    assert_eq!(found(&library, "あめ"), vec![work]);
    assert_eq!(found(&library, "ゆき"), vec![work]);
    // 二つのタグの読みをつないだ「あめゆき」の境目の「めゆ」には当たらない。
    assert_eq!(found(&library, "めゆ"), Vec::<LibraryId>::new());
}

#[test]
fn the_index_follows_saves_tags_and_removals_and_rebuilds_when_lost() {
    let temp =
        Temp(std::env::temp_dir().join(format!("piep-search-index-{}", LibraryId::now_v7())));
    let library = Library::create(&temp.0).unwrap();
    let apple = library
        .capture(capture("1", "一", "林檎の話を書く", 1_000))
        .unwrap()
        .entity_id;
    let orange = library
        .capture(capture("2", "二", "蜜柑の話を書く", 2_000))
        .unwrap()
        .entity_id;
    let grape = library
        .capture(capture("3", "三", "葡萄", 3_000))
        .unwrap()
        .entity_id;
    let mut quiet = |_: usize, _: usize| {};

    let first = library.sync_search_index(&mut quiet).unwrap();
    assert!(first.reset);
    assert_eq!((first.indexed, first.removed, first.skipped), (3, 0, 0));
    assert_eq!(found(&library, "林檎"), vec![apple]);
    let mut both = found(&library, "話");
    both.sort();
    let mut expected = vec![apple, orange];
    expected.sort();
    assert_eq!(both, expected);
    assert_eq!(found(&library, "話 -林檎"), vec![orange]);

    // 変わっていなければ何もしない。
    let again = library.sync_search_index(&mut quiet).unwrap();
    assert_eq!(again, SearchIndexReport::default());

    // 利用者のタグは文書に入る。付けた作品だけを作り直す。
    library
        .replace_user_tags(TagUpdate {
            request_id: LibraryId::now_v7(),
            work: grape,
            user: library.id,
            expected_generation: 0,
            tags: vec![UserTag {
                name: "果物".into(),
                provenance: TagProvenance::Manual,
            }],
        })
        .unwrap();
    assert!(
        found(&library, "果物").is_empty(),
        "同期の前は古い文書のまま"
    );
    let tagged = library.sync_search_index(&mut quiet).unwrap();
    assert_eq!(tagged.indexed, 1);
    assert_eq!(found(&library, "果物"), vec![grape]);

    // 改稿した作品は、新しい本文で引ける。古い本文の語ではもう出ない。
    library
        .capture(capture("2", "二", "檸檬の話を書く", 4_000))
        .unwrap();
    let revised = library.sync_search_index(&mut quiet).unwrap();
    assert_eq!(revised.indexed, 1);
    assert!(found(&library, "蜜柑").is_empty());
    assert_eq!(found(&library, "檸檬"), vec![orange]);

    // 棚にない作品の文書は消す。
    library
        .conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO search_documents(work_id,recipe) VALUES(?1,?2)",
            params![LibraryId::now_v7().as_bytes().as_slice(), RECIPE],
        )
        .unwrap();
    let pruned = library.sync_search_index(&mut quiet).unwrap();
    assert_eq!((pruned.removed, pruned.indexed), (1, 0));

    // 索引の場所が消えたら、空から作り直す。
    crate::database::tantivy_index::release_runtime(&temp.0.join("derived"));
    std::fs::remove_dir_all(temp.0.join("derived")).unwrap();
    let rebuilt = library.sync_search_index(&mut quiet).unwrap();
    assert!(rebuilt.reset);
    assert_eq!(rebuilt.indexed, 3);
    assert_eq!(found(&library, "檸檬"), vec![orange]);
    assert_eq!(found(&library, "果物"), vec![grape]);
    crate::database::tantivy_index::release_runtime(&temp.0.join("derived"));
}

#[test]
fn search_results_page_by_relevance_and_combine_with_filters_and_sorts() {
    use crate::library::{UserStateChange, WorkFilter, WorkSort};
    let temp = Temp(std::env::temp_dir().join(format!("piep-search-list-{}", LibraryId::now_v7())));
    let library = Library::create(&temp.0).unwrap();
    // 林檎の回数を変えて関連度に差をつける。偶数番はお気に入りにする。
    let mut works = Vec::new();
    for i in 0..12 {
        let body = format!("{}蜜柑", "林檎の歌。".repeat(i + 1));
        let title = format!("題{:02}", 11 - i);
        let work = library
            .capture(capture(&i.to_string(), &title, &body, 1_000 + i as i64))
            .unwrap()
            .entity_id;
        if i % 2 == 0 {
            library
                .set_user_state(
                    LibraryId::now_v7(),
                    work,
                    library.id,
                    0,
                    UserStateChange {
                        favorite: Some(true),
                        watching: None,
                    },
                )
                .unwrap();
        }
        works.push(work);
    }
    library
        .capture(capture("other", "別", "葡萄だけ", 9_000))
        .unwrap();
    library.sync_search_index(&mut |_, _| {}).unwrap();
    let expected = found(&library, "林檎");
    assert_eq!(expected.len(), 12);

    // 関連度の順で 5 件ずつ読み、続きの位置で抜けも重なりもなくつながる。
    let mut query = WorkListQuery {
        sort: WorkSort::Relevance,
        limit: 5,
        ..Default::default()
    };
    let mut paged = Vec::new();
    let mut scores = Vec::new();
    loop {
        let page = library
            .search_works(&TextSearch::current("林檎"), &query)
            .unwrap()
            .page;
        if query.after.is_none() {
            assert_eq!(page.total, Some(12));
        }
        paged.extend(page.items.iter().map(|card| card.id));
        if let Some(next) = &page.next {
            if let crate::library::SortValue::Number(bits) = next.value {
                scores.push(f64::from_bits(bits as u64));
            }
        }
        match page.next {
            Some(next) => query.after = Some(next),
            None => break,
        }
    }
    assert_eq!(paged, expected, "索引の関連度の順と同じ");
    assert!(scores.windows(2).all(|pair| pair[0] >= pair[1]));

    // 絞り込みと組み合わせる。
    let favorites = library
        .search_works(
            &TextSearch::current("林檎"),
            &WorkListQuery {
                sort: WorkSort::Relevance,
                limit: 50,
                filter: WorkFilter {
                    favorite: Some(true),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap()
        .page;
    let want: Vec<LibraryId> = expected
        .iter()
        .copied()
        .filter(|work| works.iter().position(|w| w == work).unwrap() % 2 == 0)
        .collect();
    assert_eq!(favorites.total, Some(6));
    assert_eq!(
        favorites.items.iter().map(|c| c.id).collect::<Vec<_>>(),
        want
    );

    // ほかの鍵で並べるときは一致が絞り込みになる。題の昇順で、別の作品は入らない。
    let by_title = library
        .search_works(
            &TextSearch::current("林檎"),
            &WorkListQuery {
                sort: WorkSort::Title,
                descending: false,
                limit: 4,
                ..Default::default()
            },
        )
        .unwrap()
        .page;
    assert_eq!(by_title.total, Some(12));
    let titles: Vec<&str> = by_title.items.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, ["題00", "題01", "題02", "題03"]);

    // 一致がなければ空。関連度の順は本文の検索の外では使えない。
    let none = library
        .search_works(&TextSearch::current("檸檬"), &WorkListQuery::default())
        .unwrap()
        .page;
    assert_eq!((none.total, none.items.len()), (Some(0), 0));
    assert!(library
        .list_works(&WorkListQuery {
            sort: WorkSort::Relevance,
            ..Default::default()
        })
        .is_err());
    crate::database::tantivy_index::release_runtime(&temp.0.join("derived"));
}
