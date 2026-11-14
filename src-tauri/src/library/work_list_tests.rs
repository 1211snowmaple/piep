use super::*;
use crate::library::{
    Capture, CapturedAttachment, CapturedMedia, EditBlock, EditContent, EditRequest, EntityKind,
    TagProvenance, TagUpdate, UserStateChange, UserTag,
};
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
            filter: WorkFilter {
                provider: Some("fanbox".into()),
                ..WorkFilter::default()
            },
            ..WorkListQuery::default()
        })
        .unwrap();
    assert_eq!(page.total, Some(1));
    assert_eq!(page.items[0].id, ids[3]);
    library
        .set_favorite(LibraryId::now_v7(), ids[5], library.id, 0, true)
        .unwrap();
    let favorites = WorkListQuery {
        filter: WorkFilter {
            favorite: Some(true),
            ..WorkFilter::default()
        },
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
                        filter: WorkFilter {
                            provider,
                            ..WorkFilter::default()
                        },
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
                    let page = PageSql::new(&query, library.id, 60).unwrap();
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
                        plan.contains("USING INDEX work_list_")
                            && !plan.contains("TEMP B-TREE")
                            // 続きのページは索引の途中から読む。頭からなめると後ろのページほど遅い。
                            && (!after || plan.contains("SEARCH ")),
                        "{sort:?} {descending} {after}: {plan}"
                    );
                }
            }
        }
    }
}

fn cover(bytes: &[u8]) -> CapturedMedia {
    CapturedMedia {
        slot: "cover".into(),
        role: "cover".into(),
        source_key: Some("cover-a".into()),
        source_url: Some("https://example.invalid/a.png".into()),
        quality: "original".into(),
        original_name: "a.png".into(),
        mime_type: "image/png".into(),
        bytes: Some(bytes.to_vec().into()),
    }
}

/// 絞り込みのための棚。作品の鍵と、その作品に持たせた性質。
///
/// - a: タグ R・S、作者 p1（版 1 で「旧名」、版 2 で「新名」）、シリーズ s1、表紙、改稿あり、公開日 2024
/// - b: タグ S、作者 p2、PDF と PNG の添付、公開日 2023
/// - c: fanbox、利用者のタグ U、お気に入りと監視
/// - d: タグ R、作者 p1（新名）、シリーズ s1、保留（未完）の候補
/// - e: 有効な編集、判断待ちの改稿の候補
/// - f: 外した新着の候補
/// - g: 旧棚の「修復」の版で欠けた題を埋めた（版は 2 だが作者の改稿ではない）
fn filter_shelf(library: &Library) -> BTreeMap<&'static str, LibraryId> {
    let base = |provider: &str, key: &str, body: &str, acquired: i64| {
        let mut c = capture(&Work {
            provider,
            key,
            title: key,
            author: "",
            created: None,
            updated: None,
            acquired,
            body,
        });
        c.metadata.insert("tags".into(), json!([]));
        c
    };
    let mut ids = BTreeMap::new();
    let mut a = base("pixiv", "a", "12345", 1_000);
    a.metadata.insert("tags".into(), json!(["R", "S"]));
    a.metadata.insert("author_key".into(), json!("p1"));
    a.metadata.insert("author_name".into(), json!("旧名"));
    a.metadata.insert("series_id".into(), json!("s1"));
    a.metadata.insert("series_title".into(), json!("続き物"));
    a.metadata
        .insert("create_date".into(), json!("2024-01-01T00:00:00Z"));
    a.media.push(cover(b"cover of a"));
    let mut a2 = base("pixiv", "a", "1234567", 9_000);
    a2.metadata = a.metadata.clone();
    a2.metadata.insert("author_name".into(), json!("新名"));
    a2.media.push(cover(b"cover of a"));
    ids.insert("a", library.capture(a).unwrap().entity_id);
    library.capture(a2).unwrap();
    let mut b = base("pixiv", "b", "12", 2_000);
    b.metadata.insert("tags".into(), json!(["S"]));
    b.metadata.insert("author_key".into(), json!("p2"));
    b.metadata.insert("author_name".into(), json!("別人"));
    b.metadata
        .insert("create_date".into(), json!("2023-01-01T00:00:00Z"));
    for (slot, mime) in [("doc", "application/pdf"), ("pic", "image/png")] {
        b.attachments.push(CapturedAttachment {
            slot: slot.into(),
            original_name: format!("{slot}.bin"),
            mime_type: Some(mime.into()),
            bytes: format!("{slot} of b").into_bytes().into(),
        });
    }
    ids.insert("b", library.capture(b).unwrap().entity_id);
    ids.insert(
        "c",
        library
            .capture(base("fanbox", "c", "123", 3_000))
            .unwrap()
            .entity_id,
    );
    let mut d = base("pixiv", "d", "1", 4_000);
    d.metadata.insert("tags".into(), json!(["R"]));
    d.metadata.insert("author_key".into(), json!("p1"));
    d.metadata.insert("author_name".into(), json!("新名"));
    d.metadata.insert("series_id".into(), json!("s1"));
    d.metadata.insert("series_title".into(), json!("続き物"));
    d.metadata.insert("series_position".into(), json!(2));
    ids.insert("d", library.capture(d).unwrap().entity_id);
    ids.insert(
        "e",
        library
            .capture(base("pixiv", "e", "1234", 5_000))
            .unwrap()
            .entity_id,
    );
    ids.insert(
        "f",
        library
            .capture(base("pixiv", "f", "", 6_000))
            .unwrap()
            .entity_id,
    );
    let mut g = base("pixiv", "g", "123456", 10_000);
    g.metadata.insert("title".into(), json!(""));
    ids.insert("g", library.capture(g).unwrap().entity_id);
    let repaired = library
        .capture_with_legacy_classification(base("pixiv", "g", "123456", 11_000), Some("repair"))
        .unwrap();
    assert!(repaired.created_revision);
    library
        .replace_user_tags(TagUpdate {
            request_id: LibraryId::now_v7(),
            work: ids["c"],
            user: library.id,
            expected_generation: 0,
            tags: vec![UserTag {
                name: "U".into(),
                provenance: TagProvenance::Manual,
            }],
        })
        .unwrap();
    library
        .set_user_state(
            LibraryId::now_v7(),
            ids["c"],
            library.id,
            0,
            UserStateChange {
                favorite: Some(true),
                watching: Some(true),
            },
        )
        .unwrap();
    let content = EditContent {
        format: "piep-edit-v1".into(),
        title: None,
        blocks: vec![],
        media: vec![],
        provenance: None,
    };
    library
        .save_edit(EditRequest {
            request_id: LibraryId::now_v7(),
            work: ids["e"],
            user: library.id,
            base: library.current_snapshot(ids["e"]).unwrap(),
            parent: None,
            expected: 0,
            content: &content,
            activate: true,
        })
        .unwrap();
    let conn = library.conn.lock().unwrap();
    for (work, kind, decision, hold) in [
        ("d", "new", "held", Some("incomplete")),
        ("e", "revision", "pending", None),
        ("f", "new", "dismissed", None),
    ] {
        conn.execute(
            "INSERT INTO discovery_candidates(id,work_id,kind,title,decision,hold_kind,payload_file_id,first_seen_at,updated_at)
             VALUES(?1,?2,?3,'候補',?4,?5,(SELECT id FROM content_files LIMIT 1),0,0)",
            rusqlite::params![
                LibraryId::now_v7().as_bytes().as_slice(),
                ids[work].as_bytes().as_slice(),
                kind,
                decision,
                hold
            ],
        )
        .unwrap();
    }
    ids
}

#[test]
fn every_filter_selects_the_works_with_that_property_and_pages_through_them() {
    let temp = Temp::new();
    let library = Library::create(&temp.0).unwrap();
    let ids = filter_shelf(&library);
    let entity = |kind: &str, key: &str| -> LibraryId {
        let conn = library.reader().unwrap();
        let bytes: Vec<u8> = conn
            .query_row(
                "SELECT entity_id FROM external_identities WHERE kind=?1 AND external_key=?2",
                [kind, key],
                |r| r.get(0),
            )
            .unwrap();
        LibraryId::from_slice(&bytes).unwrap()
    };
    let keys = |filter: WorkFilter| -> Vec<String> {
        let query = WorkListQuery {
            filter,
            ..WorkListQuery::default()
        };
        let full = all(&library, &query);
        let page = library.list_works(&query).unwrap();
        assert_eq!(page.total, Some(full.len() as u64), "{query:?}");
        // 絞った一覧も、一件ずつのページで同じ順に最後まで引ける。
        assert_eq!(
            paged(&library, &query, 1)
                .iter()
                .map(|c| c.id)
                .collect::<Vec<_>>(),
            full.iter().map(|c| c.id).collect::<Vec<_>>()
        );
        let mut keys: Vec<String> = full.into_iter().map(|c| c.external_key).collect();
        keys.sort();
        keys
    };
    let f = WorkFilter::default;
    let names = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let date = |field, from, until| Some(DateRange { field, from, until });
    let cases: Vec<(&str, WorkFilter, &[&str])> = vec![
        ("全部", f(), &["a", "b", "c", "d", "e", "f", "g"]),
        (
            "取得元",
            WorkFilter {
                provider: Some("fanbox".into()),
                ..f()
            },
            &["c"],
        ),
        (
            "お気に入り",
            WorkFilter {
                favorite: Some(true),
                ..f()
            },
            &["c"],
        ),
        (
            "監視",
            WorkFilter {
                watching: Some(true),
                ..f()
            },
            &["c"],
        ),
        (
            "監視なし",
            WorkFilter {
                watching: Some(false),
                ..f()
            },
            &["a", "b", "d", "e", "f", "g"],
        ),
        (
            "編集",
            WorkFilter {
                edited: Some(true),
                ..f()
            },
            &["e"],
        ),
        (
            "表紙",
            WorkFilter {
                has_cover: Some(true),
                ..f()
            },
            &["a"],
        ),
        (
            "表紙なし",
            WorkFilter {
                has_cover: Some(false),
                ..f()
            },
            &["b", "c", "d", "e", "f", "g"],
        ),
        (
            "シリーズ",
            WorkFilter {
                in_series: Some(true),
                ..f()
            },
            &["a", "d"],
        ),
        (
            "単発",
            WorkFilter {
                in_series: Some(false),
                ..f()
            },
            &["b", "c", "e", "f", "g"],
        ),
        (
            "画像か添付",
            WorkFilter {
                media: Some(MediaFilter::Any),
                ..f()
            },
            &["a", "b"],
        ),
        (
            "画像も添付もない",
            WorkFilter {
                media: Some(MediaFilter::None),
                ..f()
            },
            &["c", "d", "e", "f", "g"],
        ),
        // 画像の添付は画像に数える（旧棚と同じく MIME で分ける）。
        (
            "画像",
            WorkFilter {
                media: Some(MediaFilter::Images),
                ..f()
            },
            &["a", "b"],
        ),
        (
            "添付",
            WorkFilter {
                media: Some(MediaFilter::Files),
                ..f()
            },
            &["b"],
        ),
        (
            "画像と添付",
            WorkFilter {
                media: Some(MediaFilter::ImagesAndFiles),
                ..f()
            },
            &["b"],
        ),
        (
            "改稿",
            WorkFilter {
                revision: Some(RevisionFilter::Revised),
                ..f()
            },
            &["a", "e"],
        ),
        (
            "改稿の候補",
            WorkFilter {
                revision: Some(RevisionFilter::PendingRevision),
                ..f()
            },
            &["e"],
        ),
        (
            "保存した改稿",
            WorkFilter {
                revision: Some(RevisionFilter::SavedRevisions),
                ..f()
            },
            &["a"],
        ),
        (
            "初版",
            WorkFilter {
                revision: Some(RevisionFilter::FirstVersion),
                ..f()
            },
            &["b", "c", "d", "e", "f", "g"],
        ),
        (
            "要確認なし",
            WorkFilter {
                acquisition: Some(AcquisitionFilter::Complete),
                ..f()
            },
            &["a", "b", "c", "e", "g"],
        ),
        (
            "要確認",
            WorkFilter {
                acquisition: Some(AcquisitionFilter::Attention),
                ..f()
            },
            &["d"],
        ),
        (
            "未完",
            WorkFilter {
                acquisition: Some(AcquisitionFilter::Incomplete),
                ..f()
            },
            &["d"],
        ),
        (
            "制限",
            WorkFilter {
                acquisition: Some(AcquisitionFilter::Restricted),
                ..f()
            },
            &[],
        ),
        (
            "非表示",
            WorkFilter {
                acquisition: Some(AcquisitionFilter::Dismissed),
                ..f()
            },
            &["f"],
        ),
        (
            "文字数",
            WorkFilter {
                text_length: CountRange {
                    min: Some(3),
                    max: Some(4),
                },
                ..f()
            },
            &["c", "e"],
        ),
        (
            "画像の数",
            WorkFilter {
                media_count: CountRange {
                    min: Some(2),
                    max: None,
                },
                ..f()
            },
            &["b"],
        ),
        (
            "容量",
            WorkFilter {
                logical_size: CountRange {
                    min: Some(u64::MAX),
                    max: None,
                },
                ..f()
            },
            &[],
        ),
        // 公開日のない作品は保存日で絞る（並べ替えと同じ値）。
        (
            "公開日",
            WorkFilter {
                date: date(
                    DateField::Published,
                    Some(1_600_000_000_000),
                    Some(1_700_000_000_000),
                ),
                ..f()
            },
            &["b"],
        ),
        (
            "公開日の後",
            WorkFilter {
                date: date(DateField::Published, Some(1_700_000_000_000), None),
                ..f()
            },
            &["a"],
        ),
        (
            "保存日",
            WorkFilter {
                date: date(DateField::Saved, Some(4_000), Some(9_000)),
                ..f()
            },
            &["d", "e", "f"],
        ),
        (
            "タグすべて",
            WorkFilter {
                tags: names(&["R", "S"]),
                ..f()
            },
            &["a"],
        ),
        (
            "タグどれか",
            WorkFilter {
                tags: names(&["R", "S"]),
                any_tag: true,
                ..f()
            },
            &["a", "b", "d"],
        ),
        (
            "利用者のタグ",
            WorkFilter {
                tags: names(&["U"]),
                ..f()
            },
            &["c"],
        ),
        (
            "タグの空白",
            WorkFilter {
                tags: names(&[" R ", ""]),
                ..f()
            },
            &["a", "d"],
        ),
        (
            "タグを除く",
            WorkFilter {
                excluded_tags: names(&["S", "U"]),
                ..f()
            },
            &["d", "e", "f", "g"],
        ),
        // 前の版で名乗った名前でも、その作者の作品を引ける。
        (
            "前の名前",
            WorkFilter {
                authors: names(&["旧名"]),
                ..f()
            },
            &["a", "d"],
        ),
        (
            "今の名前",
            WorkFilter {
                authors: names(&["新名"]),
                ..f()
            },
            &["a", "d"],
        ),
        (
            "作者を除く",
            WorkFilter {
                excluded_authors: names(&["新名", "別人"]),
                ..f()
            },
            &["c", "e", "f", "g"],
        ),
        (
            "作者",
            WorkFilter {
                person: Some(entity("person", "p1")),
                ..f()
            },
            &["a", "d"],
        ),
        (
            "シリーズの作品",
            WorkFilter {
                series: Some(entity("series", "s1")),
                ..f()
            },
            &["a", "d"],
        ),
        (
            "この作品だけ",
            WorkFilter {
                works: Some(vec![ids["a"], ids["c"]]),
                ..f()
            },
            &["a", "c"],
        ),
        (
            "空の作品の集まり",
            WorkFilter {
                works: Some(vec![]),
                ..f()
            },
            &[],
        ),
        (
            "重ねがけ",
            WorkFilter {
                provider: Some("pixiv".into()),
                tags: names(&["R"]),
                revision: Some(RevisionFilter::FirstVersion),
                ..f()
            },
            &["d"],
        ),
    ];
    for (name, filter, expected) in cases {
        assert_eq!(keys(filter), expected, "{name}");
    }
    // 修復で分けた版も版の番号には数える（旧棚の版の表示と同じ）。
    assert_eq!(library.work_card(ids["g"]).unwrap().version_number, 2);
    library.verify().unwrap();
}
