use super::*;
use crate::library::{Capture, EntityKind, WorkFilter};
use crate::library_search::SearchIndexReport;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        crate::search_engine::tantivy_index::release_runtime(&self.0.join("derived"));
        crate::search_engine::tantivy_index::release_runtime(
            &self.0.join("derived").join("history"),
        );
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn work(
    provider: &str,
    key: &str,
    author: &str,
    series: Option<(&str, &str)>,
    body: &str,
    acquired: i64,
) -> Capture {
    let mut metadata = BTreeMap::from([
        ("title".into(), json!(format!("題{key}"))),
        ("author_name".into(), json!(author)),
        ("author_key".into(), json!(format!("a-{author}"))),
        ("caption".into(), json!("")),
        ("tags".into(), json!(["物語"])),
    ]);
    if let Some((id, title)) = series {
        metadata.insert("series_id".into(), json!(id));
        metadata.insert("series_title".into(), json!(title));
    }
    Capture {
        acquired_at: Some(acquired),
        request_id: LibraryId::now_v7(),
        provider: provider.into(),
        external_key: key.into(),
        kind: EntityKind::Work,
        route: "test".into(),
        raw: serde_json::to_vec(&json!({"key": key, "body": body}))
            .unwrap()
            .into(),
        metadata,
        field_states: BTreeMap::new(),
        body: Some(body.into()),
        media: vec![],
        attachments: vec![],
    }
}

fn ids(page: &SearchPage) -> Vec<LibraryId> {
    page.page.items.iter().map(|card| card.id).collect()
}

fn relevance() -> WorkListQuery {
    WorkListQuery {
        sort: WorkSort::Relevance,
        ..Default::default()
    }
}

#[test]
fn names_of_one_author_or_one_series_narrow_to_their_works() {
    let temp =
        Temp(std::env::temp_dir().join(format!("piep-search-intent-{}", LibraryId::now_v7())));
    let library = Library::create(&temp.0).unwrap();
    let save = |c: Capture| library.capture(c).unwrap().entity_id;
    let alice_1 = save(work("pixiv", "1", "Alice", None, "海の話", 1_000));
    let alice_2 = save(work(
        "pixiv",
        "2",
        "Alice",
        Some(("s1", "灯台")),
        "山の話",
        2_000,
    ));
    // 本文に作者名が出てくるだけの作品は、作者として解釈したときは入らない。
    let mentions = save(work("pixiv", "3", "Bob", None, "Alice が来た", 3_000));
    let lighthouse = save(work(
        "pixiv",
        "4",
        "Bob",
        Some(("s1", "灯台")),
        "灯台の話",
        4_000,
    ));
    // 同じ題で取得元の違うシリーズ。
    let tide_pixiv = save(work(
        "pixiv",
        "5",
        "Carol",
        Some(("s2", "潮")),
        "潮の話",
        5_000,
    ));
    let tide_fanbox = save(work(
        "fanbox",
        "6",
        "Dave",
        Some(("s9", "潮")),
        "潮の話",
        6_000,
    ));
    // 作者名とシリーズ名の両方にある名前（Carol）。
    save(work(
        "pixiv",
        "7",
        "Erin",
        Some(("s3", "Carol")),
        "灯の話",
        7_000,
    ));
    library.sync_search_index(&mut |_, _| {}).unwrap();

    // 作者名は大文字小文字を問わない。保存日の新しい順に並ぶ。
    let author = library
        .search_works(&TextSearch::current(" alice "), &relevance())
        .unwrap();
    assert_eq!(
        author.intent,
        Some(EntityIntent::Author {
            label: "Alice".into()
        })
    );
    assert_eq!(ids(&author), vec![alice_2, alice_1]);
    assert_eq!(author.page.total, Some(2));

    // 一つのシリーズの題。
    let series = library
        .search_works(&TextSearch::current("灯台"), &relevance())
        .unwrap();
    let Some(EntityIntent::Series {
        label,
        series: Some(_),
    }) = &series.intent
    else {
        panic!("シリーズとして解釈する: {:?}", series.intent)
    };
    assert_eq!(label, "灯台");
    assert_eq!(ids(&series), vec![lighthouse, alice_2]);

    // 同じ題のシリーズが複数なら、その題のシリーズすべて。
    let tide = library
        .search_works(&TextSearch::current("潮"), &relevance())
        .unwrap();
    assert_eq!(
        tide.intent,
        Some(EntityIntent::Series {
            label: "潮".into(),
            series: None
        })
    );
    assert_eq!(ids(&tide), vec![tide_fanbox, tide_pixiv]);
    // ほかの絞り込みとも重なる。
    let narrowed = library
        .search_works(
            &TextSearch::current("潮"),
            &WorkListQuery {
                filter: WorkFilter {
                    provider: Some("pixiv".into()),
                    ..Default::default()
                },
                ..relevance()
            },
        )
        .unwrap();
    assert_eq!(ids(&narrowed), vec![tide_pixiv]);

    // 作者名とシリーズ名の両方に当たる名前、式の書き方を含むもの、作者がもう絞られているときは
    // ふつうの本文の検索。
    for (text, filter) in [
        ("Carol", WorkFilter::default()),
        ("灯", WorkFilter::default()),
        ("alic", WorkFilter::default()),
        ("\"Alice\"", WorkFilter::default()),
        ("Alice -海", WorkFilter::default()),
        (
            "Alice",
            WorkFilter {
                authors: vec!["Bob".into()],
                ..Default::default()
            },
        ),
    ] {
        let page = library
            .search_works(
                &TextSearch::current(text),
                &WorkListQuery {
                    filter,
                    ..relevance()
                },
            )
            .unwrap();
        assert_eq!(page.intent, None, "{text}");
    }
    let mentioned = library
        .search_works(
            &TextSearch::current("Alice"),
            &WorkListQuery {
                filter: WorkFilter {
                    authors: vec!["Bob".into()],
                    ..Default::default()
                },
                ..relevance()
            },
        )
        .unwrap();
    assert_eq!(ids(&mentioned), vec![mentions]);
}

#[test]
fn decorations_show_where_and_why_each_work_matched() {
    let temp =
        Temp(std::env::temp_dir().join(format!("piep-search-marks-{}", LibraryId::now_v7())));
    let library = Library::create(&temp.0).unwrap();
    let body = format!(
        "{}林檎を拾った。{}",
        "前置き。".repeat(30),
        "後書き。".repeat(30)
    );
    let apple = library
        .capture(work("pixiv", "1", "作者", None, &body, 1_000))
        .unwrap()
        .entity_id;
    let tagged = library
        .capture(work("pixiv", "2", "作者", None, "何もない", 2_000))
        .unwrap()
        .entity_id;
    library.sync_search_index(&mut |_, _| {}).unwrap();
    // 同期のあとで保存した作品は、索引に文書がないので飛ばす。
    let unindexed = library
        .capture(work("pixiv", "3", "作者", None, "林檎", 3_000))
        .unwrap()
        .entity_id;

    let marks = library
        .search_decorations("林檎", &[now(apple), now(unindexed)])
        .unwrap();
    assert_eq!(marks.len(), 1);
    let mark = &marks[0];
    assert_eq!(mark.work, apple);
    assert_eq!(mark.match_fields, vec!["body".to_string()]);
    assert!(mark.field_score > 0.0);
    assert!(mark
        .score_reasons
        .iter()
        .all(|reason| reason.field == "body" && reason.term == "林檎"));
    let highlight = &mark.highlights[0];
    assert_eq!(highlight.field, "body");
    let matched: Vec<&str> = highlight
        .segments
        .iter()
        .filter(|segment| segment.matched)
        .map(|segment| segment.text.as_str())
        .collect();
    assert_eq!(matched, ["林檎"]);
    // 抜粋は一致の前後だけ（本文全体ではない）。
    assert!(highlight.text.chars().count() < body.chars().count());

    // タグに当たる語は、タグの一致として示す。
    let tags = library.search_decorations("物語", &[now(tagged)]).unwrap();
    assert_eq!(tags[0].match_fields, vec!["tags".to_string()]);
    // 除く語だけの問い合わせには付けるものがない。
    assert!(library
        .search_decorations("-林檎", &[now(apple)])
        .unwrap()
        .is_empty());
}

fn now(work: LibraryId) -> SearchMatch {
    SearchMatch {
        work,
        score: 0.0,
        matched_version: None,
        historical_matches: 0,
        semantic: None,
    }
}

#[test]
fn all_versions_finds_works_by_their_past_revisions_once_each() {
    let temp = Temp(std::env::temp_dir().join(format!("piep-search-past-{}", LibraryId::now_v7())));
    let library = Library::create(&temp.0).unwrap();
    // 各作品の改稿を古い順に保存する。
    let save = |key: &str, bodies: &[&str]| {
        let mut id = None;
        for (index, body) in bodies.iter().enumerate() {
            let at = 1_000 * (index as i64 + 1) + key.parse::<i64>().unwrap();
            id = Some(
                library
                    .capture(work("pixiv", key, "作者", None, body, at))
                    .unwrap()
                    .entity_id,
            );
        }
        id.unwrap()
    };
    let replaced = save("1", &["林檎の話", "蜜柑の話"]);
    let only_now = save("2", &["林檎"]);
    let both = save("3", &["林檎と林檎", "林檎と葡萄"]);
    let oldest = save("4", &["林檎", "桃", "桃と栗"]);
    let twice = save("5", &["林檎", "林檎です", "桜"]);
    let report = library.sync_search_index(&mut |_, _| {}).unwrap();
    assert_eq!(report.indexed, 5);
    assert!(report.history.reset);
    assert_eq!(report.history.indexed, 6, "過去の改稿は 1+1+2+2");

    let sorted = |mut ids: Vec<LibraryId>| {
        ids.sort();
        ids
    };
    let current = library
        .search_works(&TextSearch::current("林檎"), &relevance())
        .unwrap();
    assert_eq!(sorted(ids(&current)), sorted(vec![only_now, both]));
    assert!(current.matches.iter().all(|m| m.matched_version.is_none()));

    let all = library
        .search_works(&TextSearch::all_versions("林檎"), &relevance())
        .unwrap();
    assert_eq!(all.page.total, Some(5), "作品ごとに一件");
    assert_eq!(
        sorted(ids(&all)),
        sorted(vec![replaced, only_now, both, oldest, twice])
    );
    assert_eq!(
        all.matches.iter().map(|m| m.work).collect::<Vec<_>>(),
        ids(&all),
        "一致はページと同じ順"
    );
    let of = |work: LibraryId| all.matches.iter().find(|m| m.work == work).unwrap();
    assert_eq!(
        (
            of(replaced).matched_version,
            of(replaced).historical_matches
        ),
        (Some(1), 1)
    );
    assert_eq!(
        (
            of(only_now).matched_version,
            of(only_now).historical_matches
        ),
        (None, 0)
    );
    // いまの版でも一致すれば、いまの版の一致として扱う（過去の一致は数だけ）。
    assert_eq!(
        (of(both).matched_version, of(both).historical_matches),
        (None, 1)
    );
    assert_eq!(of(oldest).matched_version, Some(1));
    assert_eq!(of(twice).historical_matches, 2);
    assert!(matches!(of(twice).matched_version, Some(1 | 2)));
    // 関連度の順に並び、過去の改稿だけの一致は重みが掛かる。
    assert!(all
        .matches
        .windows(2)
        .all(|pair| pair[0].score >= pair[1].score));
    let current_scores: Vec<f64> = current.matches.iter().map(|m| m.score).collect();
    assert!(current_scores.contains(&of(both).score));

    // 除く語は、それぞれの版の文書で判定する。
    let excluded = library
        .search_works(&TextSearch::all_versions("林檎 -葡萄"), &relevance())
        .unwrap();
    assert_eq!(
        sorted(ids(&excluded)),
        sorted(vec![replaced, only_now, both, oldest, twice]),
        "葡萄のない最初の改稿で一致する"
    );
    // ほかの鍵で並べても、過去の改稿の一致は絞り込みとして効く。
    let by_title = library
        .search_works(
            &TextSearch::all_versions("林檎"),
            &WorkListQuery {
                sort: WorkSort::Title,
                descending: false,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        ids(&by_title),
        vec![replaced, only_now, both, oldest, twice]
    );
    assert_eq!(by_title.matches[0].matched_version, Some(1));

    // 抜粋は一致した改稿の文書から作る（いまの本文には林檎がない）。
    let marks = library.search_decorations("林檎", &all.matches).unwrap();
    let mark = marks.iter().find(|m| m.work == replaced).unwrap();
    assert_eq!(mark.match_fields, vec!["body".to_string()]);
    assert!(mark.highlights[0].text.contains("林檎"));

    // 改稿すると、前の版が過去の改稿の索引に入る。
    library
        .capture(work("pixiv", "2", "作者", None, "柿", 9_000))
        .unwrap();
    let revised = library.sync_search_index(&mut |_, _| {}).unwrap();
    assert_eq!((revised.indexed, revised.history.indexed), (1, 1));
    assert!(!revised.history.reset);
    let after = library
        .search_works(&TextSearch::all_versions("林檎"), &relevance())
        .unwrap();
    let moved = after.matches.iter().find(|m| m.work == only_now).unwrap();
    assert_eq!(moved.matched_version, Some(1));
    // 変わっていなければ何もしない。
    assert_eq!(
        library.sync_search_index(&mut |_, _| {}).unwrap(),
        SearchIndexReport::default()
    );
}
