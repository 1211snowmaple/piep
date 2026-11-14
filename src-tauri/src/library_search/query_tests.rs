use super::*;
use crate::library::{Capture, EntityKind, WorkListQuery, WorkSort};
use crate::library_search::{SearchPage, TextSearch};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[test]
fn reads_the_old_shelf_filters_and_leaves_the_rest_as_words() {
    let parts = parse_query(
        "海 tag:\"長編 小説\" -tag:悲恋 author:Alice -creator:Bob series:pixiv:12 \
         pixiv:34 series:\"灯台\" source:FANBOX id:56 title:題 -series_title:副題 foo:bar -海辺",
    );
    assert_eq!(parts.text, "海 題 -副題 foo:bar -海辺");
    assert_eq!(parts.tags, ["長編 小説"]);
    assert_eq!(parts.excluded_tags, ["悲恋"]);
    assert_eq!(parts.authors, ["Alice"]);
    assert_eq!(parts.excluded_authors, ["Bob"]);
    assert_eq!(
        parts.series,
        [
            SeriesRef::Key {
                provider: "pixiv".into(),
                key: "12".into()
            },
            SeriesRef::Key {
                provider: "pixiv".into(),
                key: "34".into()
            },
            SeriesRef::KeyOrTitle("灯台".into()),
        ]
    );
    assert_eq!(parts.providers, ["fanbox"]);
    assert_eq!(
        parts.works,
        [WorkRef {
            provider: None,
            key: "56".into()
        }]
    );
    // 除外を書けない項目の除外と、空の値。
    let parts = parse_query("-series:灯台 -source:pixiv -id:1 tag: author:\"\"");
    assert_eq!(parts.text, "-series:灯台 -source:pixiv -id:1");
    assert!(parts.is_plain());
    assert!(parse_query("ただの 語").is_plain());
}

#[test]
fn work_urls_become_the_work_and_its_provider() {
    let parts = parse_query(
        "https://www.pixiv.net/novel/show.php?id=101&lang=ja https://www.pixiv.net/novels/102 \
         https://alice.fanbox.cc/posts/103 url:https://www.fanbox.cc/@bob/posts/104 \
         https://example.com/posts/105 https://www.pixiv.net/users/106",
    );
    let found: Vec<(Option<&str>, &str)> = parts
        .works
        .iter()
        .map(|work| (work.provider, work.key.as_str()))
        .collect();
    assert_eq!(
        found,
        [
            (Some("pixiv"), "101"),
            (Some("pixiv"), "102"),
            (Some("fanbox"), "103"),
            (Some("fanbox"), "104")
        ]
    );
    assert_eq!(parts.providers, ["pixiv", "fanbox"]);
    // 作品でない URL は語のまま。
    assert_eq!(
        parts.text,
        "https://example.com/posts/105 https://www.pixiv.net/users/106"
    );
}

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        crate::database::tantivy_index::release_runtime(&self.0.join("derived"));
        crate::database::tantivy_index::release_runtime(&self.0.join("derived").join("history"));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn work(
    provider: &str,
    key: &str,
    author: &str,
    tags: &[&str],
    series: Option<(&str, &str)>,
    body: &str,
) -> Capture {
    let mut metadata = BTreeMap::from([
        ("title".into(), json!(format!("題{key}"))),
        ("author_name".into(), json!(author)),
        ("author_key".into(), json!(format!("a-{author}"))),
        ("caption".into(), json!("")),
        ("tags".into(), json!(tags)),
    ]);
    if let Some((id, title)) = series {
        metadata.insert("series_id".into(), json!(id));
        metadata.insert("series_title".into(), json!(title));
    }
    Capture {
        acquired_at: Some(key.parse::<i64>().unwrap() * 1_000),
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

#[test]
fn filters_written_in_the_query_narrow_the_list() {
    let temp =
        Temp(std::env::temp_dir().join(format!("piep-search-query-{}", LibraryId::now_v7())));
    let library = Library::create(&temp.0).unwrap();
    let save = |c: Capture| library.capture(c).unwrap().entity_id;
    let first = save(work(
        "pixiv",
        "1",
        "Alice",
        &["海", "長編 小説"],
        Some(("10", "灯台")),
        "海の話",
    ));
    let second = save(work(
        "pixiv",
        "2",
        "Alice",
        &["山"],
        Some(("10", "灯台")),
        "山の話。番号 3 の話",
    ));
    let third = save(work("pixiv", "3", "Bob", &["海"], None, "海の話"));
    let fanbox = save(work(
        "fanbox",
        "4",
        "Bob",
        &["海"],
        Some(("10", "灯台")),
        "海の話",
    ));
    library.sync_search_index(&mut |_, _| {}).unwrap();
    let search = |text: &str| {
        library
            .search_works(
                &TextSearch::current(text),
                &WorkListQuery {
                    sort: WorkSort::Relevance,
                    ..Default::default()
                },
            )
            .unwrap()
    };
    let sorted = |mut ids: Vec<LibraryId>| {
        ids.sort();
        ids
    };

    // 書き方だけなら絞り込んだ一覧（保存日の新しい順）。本文の一致はない。
    let tagged = search("tag:海");
    assert_eq!(ids(&tagged), vec![fanbox, third, first]);
    assert!(tagged.matches.is_empty());
    assert_eq!(ids(&search("tag:\"長編 小説\"")), vec![first]);
    // 引用符がなければ「小説」は本文の語（タグの欄で当たる）。
    assert_eq!(ids(&search("tag:海 -tag:長編 小説")), vec![first]);
    assert_eq!(
        ids(&search("tag:海 -tag:\"長編 小説\"")),
        vec![fanbox, third]
    );
    // 語と一緒なら、本文の一致を絞り込む。
    let words = search("海 author:Bob");
    assert_eq!(sorted(ids(&words)), sorted(vec![third, fanbox]));
    assert_eq!(words.matches.len(), 2);
    assert_eq!(ids(&search("海 -author:Bob")), vec![first]);
    // 取得元・シリーズ。同じ題でも取得元が違えば別のシリーズ。
    assert_eq!(ids(&search("source:fanbox")), vec![fanbox]);
    assert_eq!(ids(&search("series:pixiv:10")), vec![second, first]);
    assert_eq!(ids(&search("pixiv:10")), vec![second, first]);
    assert_eq!(ids(&search("series:灯台")), vec![fanbox, second, first]);
    assert_eq!(
        ids(&search("series:灯台 source:pixiv")),
        vec![second, first]
    );
    assert_eq!(ids(&search("series:無い")), Vec::<LibraryId>::new());
    // ID と URL は取得元の ID がそのまま一致する作品だけ。
    assert_eq!(ids(&search("id:3")), vec![third]);
    assert_eq!(
        ids(&search(
            "https://www.pixiv.net/novel/show.php?id=3 https://www.pixiv.net/novels/1"
        )),
        vec![third, first]
    );
    assert_eq!(
        ids(&search("https://bob.fanbox.cc/posts/3")),
        Vec::<LibraryId>::new()
    );
    // 画面の絞り込みとぶつかる取得元は、両方を満たす作品（なし）。
    let clash = library
        .search_works(
            &TextSearch::current("source:fanbox"),
            &WorkListQuery {
                filter: crate::library::WorkFilter {
                    provider: Some("pixiv".into()),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
    assert!(clash.page.items.is_empty());
    // 書き方の作者を絞ったら、残る語を作者の名前として解釈しない。
    assert_eq!(search("Alice author:Bob").intent, None);
    // 抜粋は残る語だけで作る。
    let marks = library
        .search_decorations("海 tag:海", &search("海 tag:海").matches)
        .unwrap();
    assert!(marks
        .iter()
        .all(|mark| mark.score_reasons.iter().all(|reason| reason.term == "海")));
}
