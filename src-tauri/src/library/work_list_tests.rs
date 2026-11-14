use super::*;
use crate::library::{Capture, EditBlock, EditContent, EditRequest, EntityKind};
use serde_json::json;
use std::{cmp::Ordering, collections::BTreeMap, fs, path::PathBuf};

const SORTS: [WorkSort; 9] = [
    WorkSort::Saved,
    WorkSort::Published,
    WorkSort::Updated,
    WorkSort::Title,
    WorkSort::Author,
    WorkSort::Length,
    WorkSort::Size,
    WorkSort::Media,
    WorkSort::Version,
];

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("piep-work-list-{}", LibraryId::now_v7())))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Work<'a> {
    provider: &'a str,
    key: &'a str,
    title: &'a str,
    author: &'a str,
    created: Option<&'a str>,
    updated: Option<&'a str>,
    acquired: i64,
    body: &'a str,
}

fn capture(work: &Work<'_>) -> Capture {
    let mut metadata = BTreeMap::from([
        ("title".into(), json!(work.title)),
        ("author_name".into(), json!(work.author)),
        ("caption".into(), json!(format!("{} の説明", work.title))),
        ("tags".into(), json!(["b", "a"])),
    ]);
    if let Some(date) = work.created {
        metadata.insert("create_date".into(), json!(date));
    }
    if let Some(date) = work.updated {
        metadata.insert("update_date".into(), json!(date));
    }
    Capture {
        acquired_at: Some(work.acquired),
        request_id: LibraryId::now_v7(),
        provider: work.provider.into(),
        external_key: work.key.into(),
        kind: EntityKind::Work,
        route: "test".into(),
        raw: serde_json::to_vec(&json!({"key": work.key, "title": work.title}))
            .unwrap()
            .into(),
        metadata,
        field_states: BTreeMap::new(),
        body: Some(work.body.into()),
        media: vec![],
        attachments: vec![],
    }
}

/// 同じ値・大文字小文字だけ違う題・公開日のない作品・取得元の更新日を混ぜた棚。
fn shelf(library: &Library) -> Vec<LibraryId> {
    let works = [
        Work {
            provider: "pixiv",
            key: "1",
            title: "b",
            author: "Y",
            created: Some("2024-01-01T00:00:00+09:00"),
            updated: None,
            acquired: 1_000,
            body: "12345",
        },
        Work {
            provider: "pixiv",
            key: "2",
            title: "A",
            author: "x",
            created: Some("2023-01-01T00:00:00+09:00"),
            updated: None,
            acquired: 3_000,
            body: "123",
        },
        Work {
            provider: "pixiv",
            key: "3",
            title: "a",
            author: "X",
            created: None,
            updated: None,
            acquired: 2_000,
            body: "12345",
        },
        Work {
            provider: "fanbox",
            key: "4",
            title: "\u{3000} c\r\n",
            author: "y",
            created: Some("2022-01-01T00:00:00Z"),
            updated: Some("2025-06-01T00:00:00Z"),
            acquired: 5_000,
            body: "1",
        },
        Work {
            provider: "pixiv",
            key: "5",
            title: "B",
            author: "Y",
            created: Some("2024-01-01T00:00:00+09:00"),
            updated: None,
            acquired: 4_000,
            body: "123",
        },
        Work {
            provider: "pixiv",
            key: "6",
            title: "",
            author: "",
            created: Some("2021-01-01T00:00:00+09:00"),
            updated: None,
            acquired: 4_000,
            body: "",
        },
        Work {
            provider: "pixiv",
            key: "7",
            title: "日本語",
            author: "作者",
            created: Some("2020-01-01T00:00:00+09:00"),
            updated: None,
            acquired: 6_000,
            body: "日本語の本文",
        },
    ];
    let ids: Vec<_> = works
        .iter()
        .map(|work| library.capture(capture(work)).unwrap().entity_id)
        .collect();
    // 二つ目の改稿で版の番号が進み、保存日はいまの改稿の日になる。
    let mut changed = capture(&works[0]);
    changed.body = Some("1234567".into());
    changed.acquired_at = Some(7_000);
    library.capture(changed).unwrap();
    ids
}

fn key(card: &WorkCard, sort: WorkSort) -> SortValue {
    match sort {
        WorkSort::Saved => SortValue::Number(card.saved_at),
        WorkSort::Published => SortValue::Number(card.published_at.unwrap_or(card.saved_at)),
        WorkSort::Updated => SortValue::Number(
            card.updated_at
                .or(card.published_at)
                .unwrap_or(card.saved_at),
        ),
        // SQLite の NOCASE は ASCII だけを畳む。
        WorkSort::Title => SortValue::Text(card.title.to_ascii_lowercase()),
        WorkSort::Author => SortValue::Text(card.author_name.to_ascii_lowercase()),
        WorkSort::Length => SortValue::Number(card.text_length as i64),
        WorkSort::Size => SortValue::Number(card.logical_size as i64),
        WorkSort::Media => SortValue::Number(card.media_count as i64),
        WorkSort::Version => SortValue::Number(card.version_number as i64),
    }
}

fn compare(a: &SortValue, b: &SortValue) -> Ordering {
    match (a, b) {
        (SortValue::Number(a), SortValue::Number(b)) => a.cmp(b),
        (SortValue::Text(a), SortValue::Text(b)) => a.as_bytes().cmp(b.as_bytes()),
        _ => panic!("鍵の型が混ざっています"),
    }
}

fn all(library: &Library, query: &WorkListQuery) -> Vec<WorkCard> {
    let mut query = query.clone();
    query.limit = PAGE_LIMIT;
    let page = library.list_works(&query).unwrap();
    assert!(page.next.is_none());
    page.items
}

fn paged(library: &Library, query: &WorkListQuery, limit: usize) -> Vec<WorkCard> {
    let mut query = query.clone();
    query.limit = limit;
    let mut cards = Vec::new();
    loop {
        let page = library.list_works(&query).unwrap();
        assert!(page.items.len() <= limit);
        cards.extend(page.items);
        match page.next {
            Some(next) => {
                query.after = Some(next);
                query.count = false;
            }
            None => return cards,
        }
    }
}

#[test]
fn every_sort_pages_through_the_shelf_in_the_order_of_one_full_sort_with_ties_in_saving_order() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let ids = shelf(&library);
    for sort in SORTS {
        for descending in [false, true] {
            let query = WorkListQuery {
                sort,
                descending,
                ..WorkListQuery::default()
            };
            let full = all(&library, &query);
            assert_eq!(full.len(), ids.len(), "{sort:?}");
            for pair in full.windows(2) {
                let order = compare(&key(&pair[0], sort), &key(&pair[1], sort))
                    .then(pair[0].id.cmp(&pair[1].id));
                let expected = if descending {
                    Ordering::Greater
                } else {
                    Ordering::Less
                };
                assert_eq!(order, expected, "{sort:?} {descending}: {pair:?}");
            }
            for limit in [1, 2, 3] {
                let pages = paged(&library, &query, limit);
                assert_eq!(
                    pages.iter().map(|c| c.id).collect::<Vec<_>>(),
                    full.iter().map(|c| c.id).collect::<Vec<_>>(),
                    "{sort:?} {descending} {limit}"
                );
            }
        }
    }
    // 題は前後の空白（改行・全角の空白を含む）を除き、空の題も並ぶ。公開日のない作品は保存日で並ぶ。
    let first = all(&library, &WorkListQuery::default());
    let card = |id: LibraryId| first.iter().find(|c| c.id == id).unwrap();
    assert_eq!(card(ids[3]).title, "c");
    assert_eq!(card(ids[2]).published_at, None);
    assert_eq!(card(ids[3]).updated_at, Some(1_748_736_000_000));
    assert_eq!(card(ids[1]).published_at, Some(1_672_498_800_000));
    // 改稿した作品は、いまの改稿の保存日と版、いまの本文の文字数を持つ。
    assert_eq!(
        (
            card(ids[0]).saved_at,
            card(ids[0]).version_number,
            card(ids[0]).text_length
        ),
        (7_000, 2, 7)
    );
    assert_eq!(card(ids[6]).text_length, 6);
    assert_eq!(card(ids[6]).tags, vec!["a", "b"]);
    assert_eq!(card(ids[6]).excerpt.as_deref(), Some("日本語 の説明"));
    assert_eq!(card(ids[6]).external_key, "7");
    library.verify().unwrap();
}

#[test]
fn filters_count_only_matching_works_and_favorites_update_the_list_at_once() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let ids = shelf(&library);
    let page = library
        .list_works(&WorkListQuery {
            provider: Some("fanbox".into()),
            ..WorkListQuery::default()
        })
        .unwrap();
    assert_eq!(page.total, Some(1));
    assert_eq!(page.items[0].id, ids[3]);
    library
        .set_favorite(LibraryId::now_v7(), ids[5], library.id, 0, true)
        .unwrap();
    let favorites = WorkListQuery {
        favorite: Some(true),
        limit: 1,
        ..WorkListQuery::default()
    };
    let page = library.list_works(&favorites).unwrap();
    assert_eq!(page.total, Some(1));
    assert!(page.items[0].favorite);
    assert!(page.next.is_none());
    // 別の利用者のお気に入りは、この棚の一覧に混ざらない。
    library
        .set_favorite(LibraryId::now_v7(), ids[4], LibraryId::now_v7(), 0, true)
        .unwrap();
    assert_eq!(library.list_works(&favorites).unwrap().total, Some(1));
    // 続きの位置の型が鍵と違えば、推測で読み替えない。
    let wrong = WorkListQuery {
        sort: WorkSort::Title,
        after: Some(WorkCursor {
            value: SortValue::Number(1),
            work: ids[0],
        }),
        ..WorkListQuery::default()
    };
    assert!(library.list_works(&wrong).is_err());
    library.verify().unwrap();
}

#[test]
fn an_active_edit_title_is_what_the_title_sort_uses_and_the_list_is_rebuilt_when_stale() {
    let temp = Temp::new();
    let ids;
    {
        let library = Library::create(&temp.0).unwrap();
        ids = shelf(&library);
        let base = library.current_snapshot(ids[6]).unwrap();
        let content = EditContent {
            format: "piep-edit-v1".into(),
            title: Some("  0 編集した題 ".into()),
            blocks: vec![EditBlock {
                id: LibraryId::now_v7(),
                ordinal: 0,
                kind: "paragraph".into(),
                text: Some("編集".into()),
                attrs: json!({}),
                media_slot: None,
            }],
            media: vec![],
            provenance: None,
        };
        let edit = library
            .save_edit(EditRequest {
                request_id: LibraryId::now_v7(),
                work: ids[6],
                user: library.id,
                base,
                parent: None,
                expected: 0,
                content: &content,
                activate: true,
            })
            .unwrap();
        let title = WorkListQuery {
            sort: WorkSort::Title,
            descending: false,
            ..WorkListQuery::default()
        };
        let first = &library.list_works(&title).unwrap().items[1];
        // 空の題が先頭、次に編集の題（前後の空白を除く）。
        assert_eq!(
            (first.id, first.title.as_str(), first.edit_id),
            (ids[6], "0 編集した題", Some(edit))
        );
        library.verify().unwrap();
        // 一覧の値を壊すと検証で見つかる。
        library
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE work_current_projection SET text_length=text_length+1 WHERE work_id=?1",
                [ids[2].as_bytes().as_slice()],
            )
            .unwrap();
        assert!(matches!(library.verify(), Err(Error::Integrity(_))));
        // 行が欠けたり作り方が古かったりすれば、次に開いたときに作り直す。
        let conn = library.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM work_current_projection WHERE work_id=?1",
            [ids[0].as_bytes().as_slice()],
        )
        .unwrap();
        conn.execute("UPDATE work_current_projection SET recipe=1", [])
            .unwrap();
    }
    let library = Library::open(&temp.0).unwrap();
    library.verify().unwrap();
    let page = library
        .list_works(&WorkListQuery {
            sort: WorkSort::Title,
            descending: false,
            ..WorkListQuery::default()
        })
        .unwrap();
    assert_eq!(page.total, Some(ids.len() as u64));
    assert_eq!(page.items[1].title, "0 編集した題");
}

#[test]
fn every_page_reads_through_an_index_instead_of_sorting_the_whole_shelf() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let ids = shelf(&library);
    let conn = library.reader().unwrap();
    for sort in SORTS {
        for descending in [false, true] {
            for after in [false, true] {
                for provider in [None, Some("pixiv".to_string())] {
                    let query = WorkListQuery {
                        sort,
                        descending,
                        provider,
                        after: after.then(|| WorkCursor {
                            value: if sort.is_text() {
                                SortValue::Text("m".into())
                            } else {
                                SortValue::Number(3)
                            },
                            work: ids[0],
                        }),
                        ..WorkListQuery::default()
                    };
                    let page = PageSql::new(&query, 60).unwrap();
                    let plan: Vec<String> = conn
                        .prepare(&format!("EXPLAIN QUERY PLAN {}", page.page))
                        .unwrap()
                        .query_map(
                            rusqlite::params_from_iter(page.values.iter().map(|v| v.as_ref())),
                            |r| r.get(3),
                        )
                        .unwrap()
                        .collect::<std::result::Result<_, _>>()
                        .unwrap();
                    let plan = plan.join(" / ");
                    assert!(
                        plan.contains("USING INDEX work_list_") && !plan.contains("TEMP B-TREE"),
                        "{sort:?} {descending} {after}: {plan}"
                    );
                }
            }
        }
    }
}
