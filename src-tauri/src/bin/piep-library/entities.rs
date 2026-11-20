//! 作者・シリーズの画面を、旧棚の画面と同じ引き方で全員・全シリーズについて比べる。
//! 旧棚は写しを開く（旧 DB は開くときに後片付けを書き込む）。新しい棚は読むだけ。
//!
//! 比べるもの：頭（名前・説明・リンク・画像の有無・完結・話数・作品数・版の番号）、
//! 作品に付いたタグと数、作品の並び（画面の既定の並び。シリーズは `series_order` の昇順、
//! 作者は `source_created_at` の降順）、作者のシリーズの一覧（既定の並び）、作者・シリーズに
//! 関わるコレクション、ライブラリの「作者・クリエイター」「シリーズ」タブの一覧（並び順・向き・
//! 監視・完結・作品数・作品の絞り込み・語の組み合わせごとに、最後のページまで）。
//! 名前や題は出力に出さない（違いは ID と項目名だけ）。

use piep_lib::database::{
    models::{EntityFacetScope, SearchV2Params, WorkCollectionSummary},
    Database,
};
use piep_lib::library::{EntityKind, Library, WorkFilter, WorkListQuery, WorkSort};
use piep_lib::library_entity::SeriesSort;
use piep_lib::library_entity_list::{CollectionSummary, EntityListQuery, EntityScope, WatchScope};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Instant;

type Error = Box<dyn std::error::Error>;
type Key = (String, String);

fn old_sequence(old: &Database, params: SearchV2Params) -> Result<Vec<Key>, Error> {
    let mut params = SearchV2Params {
        projection: Some("bulk".into()),
        limit: Some(200),
        ..params
    };
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    loop {
        let page = old.search_downloads_v2(&params)?;
        for item in page.items {
            let key = (item.source, item.source_id);
            if seen.insert(key.clone()) {
                items.push(key);
            }
        }
        params.cursor = page.next_cursor;
        if params.cursor.is_none() {
            return Ok(items);
        }
    }
}

fn new_sequence(
    new: &Library,
    filter: WorkFilter,
    sort: WorkSort,
    descending: bool,
) -> Result<Vec<Key>, Error> {
    let mut query = WorkListQuery {
        sort,
        descending,
        limit: 500,
        filter,
        ..WorkListQuery::default()
    };
    let mut items = Vec::new();
    loop {
        let page = new.list_works(&query)?;
        items.extend(
            page.items
                .iter()
                .map(|card| (card.provider.clone(), card.external_key.clone())),
        );
        match page.next {
            Some(next) => query.after = Some(next),
            None => return Ok(items),
        }
    }
}

fn old_links(json: Option<&str>) -> Vec<String> {
    let Some(value) = json.and_then(|json| serde_json::from_str::<Value>(json).ok()) else {
        return Vec::new();
    };
    value
        .as_array()
        .map(|links| {
            links
                .iter()
                .filter_map(|link| match link {
                    Value::String(url) => Some(url.clone()),
                    Value::Object(object) => {
                        object.get("url").and_then(Value::as_str).map(str::to_owned)
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn blank(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.trim().is_empty())
}

struct Report {
    counts: BTreeMap<&'static str, usize>,
    mismatches: BTreeMap<&'static str, Vec<Value>>,
}

impl Report {
    fn count(&mut self, name: &'static str) {
        *self.counts.entry(name).or_default() += 1;
    }

    fn differ(&mut self, field: &'static str, kind: &str, key: &Key, detail: Value) {
        self.count(field);
        let list = self.mismatches.entry(field).or_default();
        if list.len() < 20 {
            list.push(json!({"entity": format!("{kind} {} {}", key.0, key.1), "detail": detail}));
        }
    }

    fn same<T: PartialEq + std::fmt::Debug>(
        &mut self,
        field: &'static str,
        kind: &str,
        key: &Key,
        old: T,
        new: T,
        show: bool,
    ) {
        if old != new {
            let detail = if show {
                json!({"old": format!("{old:?}"), "new": format!("{new:?}")})
            } else {
                Value::Null
            };
            self.differ(field, kind, key, detail);
        }
    }
}

/// 並びの違いを、顔ぶれの違いと順の違いに分けて数える。順の違いは最初に食い違う位置だけを出す。
fn sequences(
    report: &mut Report,
    field: &'static str,
    order: &'static str,
    kind: &str,
    key: &Key,
    old: &[Key],
    new: &[Key],
) {
    if old == new {
        return;
    }
    let old_set: BTreeSet<&Key> = old.iter().collect();
    let new_set: BTreeSet<&Key> = new.iter().collect();
    if old_set != new_set {
        report.differ(
            field,
            kind,
            key,
            json!({
                "old_only": old_set.difference(&new_set).take(5).map(|k| format!("{} {}", k.0, k.1)).collect::<Vec<_>>(),
                "new_only": new_set.difference(&old_set).take(5).map(|k| format!("{} {}", k.0, k.1)).collect::<Vec<_>>(),
            }),
        );
    } else {
        let at = old.iter().zip(new).position(|(a, b)| a != b).unwrap_or(0);
        report.differ(
            order,
            kind,
            key,
            json!({
                "at": at,
                "len": old.len(),
                "old": old[at..(at + 3).min(old.len())].iter().map(|k| format!("{} {}", k.0, k.1)).collect::<Vec<_>>(),
                "new": new[at..(at + 3).min(new.len())].iter().map(|k| format!("{} {}", k.0, k.1)).collect::<Vec<_>>(),
            }),
        );
    }
}

/// 画面の「保存データ」の欄。旧棚は最新の版の JSON のファイル、新しい棚は今の版の取得元の応答。
fn same_profile(
    report: &mut Report,
    old: &Database,
    new: &Library,
    kind: &'static str,
    key: &Key,
    head: &piep_lib::library_entity::EntityHead,
) -> Result<(), Error> {
    let old_json = old
        .list_entity_versions(kind, &key.0, &key.1)?
        .first()
        .filter(|version| !version.json_path.trim().is_empty())
        .and_then(|version| std::fs::read_to_string(&version.json_path).ok())
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok());
    let new_json = match head.snapshot {
        Some(snapshot) => new
            .entity_profile(head.id, snapshot)?
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok()),
        None => None,
    };
    report.count(if kind == "person" {
        "person_profiles"
    } else {
        "series_profiles"
    });
    if old_json != new_json {
        let field = if kind == "person" {
            "person_profile_json"
        } else {
            "series_profile_json"
        };
        report.differ(
            field,
            kind,
            key,
            json!({"old": old_json.is_some(), "new": new_json.is_some()}),
        );
    }
    Ok(())
}

/// 旧棚の日時の文字（RFC 3339 か SQLite の `YYYY-MM-DD HH:MM:SS`、UTC）をミリ秒にする。
fn old_time(value: Option<&str>) -> Option<i64> {
    let value = value?.trim();
    if let Ok(date) = chrono::DateTime::parse_from_rfc3339(value) {
        return Some(date.timestamp_millis());
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(date) = chrono::NaiveDateTime::parse_from_str(value, format) {
            return Some(date.and_utc().timestamp_millis());
        }
    }
    None
}

/// 作者・シリーズに関わるコレクションを、並び（旧棚の ID）と中身で比べる。
fn same_collections(
    report: &mut Report,
    kind: &'static str,
    key: &Key,
    old: Vec<WorkCollectionSummary>,
    new: Vec<CollectionSummary>,
) {
    report.count(labelled(kind, "collection_lists"));
    let old_ids: Vec<Key> = old.iter().map(|c| (String::new(), c.id.clone())).collect();
    let new_ids: Vec<Key> = new
        .iter()
        .map(|c| {
            (
                String::new(),
                c.legacy_id.clone().unwrap_or_else(|| c.id.to_string()),
            )
        })
        .collect();
    sequences(
        report,
        labelled(kind, "collections"),
        labelled(kind, "collection_order"),
        kind,
        key,
        &old_ids,
        &new_ids,
    );
    let by_id: BTreeMap<&str, &CollectionSummary> = new
        .iter()
        .filter_map(|c| c.legacy_id.as_deref().map(|id| (id, c)))
        .collect();
    for old in &old {
        let Some(new) = by_id.get(old.id.as_str()) else {
            continue;
        };
        let at = (String::from("collection"), old.id.clone());
        report.count("collection_rows");
        report.same(
            "collection_name",
            kind,
            &at,
            old.name.as_str(),
            new.name.as_str(),
            false,
        );
        report.same(
            "collection_description",
            kind,
            &at,
            blank(old.description.as_deref()),
            blank(new.description.as_deref()),
            false,
        );
        report.same(
            "collection_kind",
            kind,
            &at,
            old.collection_kind == "ordered",
            new.ordered,
            true,
        );
        report.same(
            "collection_members",
            kind,
            &at,
            old.member_count as u64,
            new.member_count,
            true,
        );
        report.same(
            "collection_available",
            kind,
            &at,
            old.available_count as u64,
            new.available_count,
            true,
        );
        report.same(
            "collection_text_length",
            kind,
            &at,
            old.total_text_length as u64,
            new.total_text_length,
            true,
        );
        report.same(
            "collection_revision",
            kind,
            &at,
            old.revision,
            new.revision,
            true,
        );
        report.same(
            "collection_cover_mode",
            kind,
            &at,
            old.cover_mode.as_str(),
            new.cover_mode.as_str(),
            true,
        );
        report.same(
            "collection_name_source",
            kind,
            &at,
            old.name_source.as_str(),
            new.name_source.as_str(),
            true,
        );
        report.same(
            "collection_cover",
            kind,
            &at,
            old.cover_path.is_some(),
            new.cover.is_some(),
            true,
        );
        report.same(
            "collection_cover_image",
            kind,
            &at,
            old.cover_image_path.is_some(),
            new.cover_image.is_some(),
            true,
        );
        report.same(
            "collection_created_at",
            kind,
            &at,
            old_time(Some(&old.created_at)),
            Some(new.created_at),
            true,
        );
        report.same(
            "collection_updated_at",
            kind,
            &at,
            old_time(Some(&old.updated_at)),
            Some(new.updated_at),
            true,
        );
        let old_tiles: Vec<(String, String, bool)> = old
            .cover_tiles
            .iter()
            .map(|t| {
                (
                    t.source.clone(),
                    t.source_id.clone(),
                    t.cover_path.is_some(),
                )
            })
            .collect();
        let new_tiles: Vec<(String, String, bool)> = new
            .tiles
            .iter()
            .map(|t| {
                (
                    t.provider.clone(),
                    t.external_key.clone(),
                    t.cover.is_some(),
                )
            })
            .collect();
        report.same("collection_tiles", kind, &at, old_tiles, new_tiles, false);
        let old_titles: Vec<(&str, &str)> = old
            .cover_tiles
            .iter()
            .map(|t| (t.title.trim(), t.author_name.trim()))
            .collect();
        let new_titles: Vec<(&str, &str)> = new
            .tiles
            .iter()
            .map(|t| (t.title.trim(), t.author_name.trim()))
            .collect();
        report.same(
            "collection_tile_titles",
            kind,
            &at,
            old_titles,
            new_titles,
            false,
        );
    }
}

/// 作者・シリーズ・全体（`all`）ごとの数え名。
fn labelled(kind: &str, what: &str) -> &'static str {
    Box::leak(format!("{kind}_{what}").into_boxed_str())
}

/// ライブラリの「コレクション」タブの一覧と、コレクションの画面の中身（評価文書 6.23）。
fn collections(
    report: &mut Report,
    old: &Database,
    new: &Library,
    ms: &mut Vec<f64>,
) -> Result<(), Error> {
    let key: Key = (String::new(), String::from("all"));
    let old_list = old.list_work_collections()?;
    let timer = Instant::now();
    let new_list = new.list_collections()?;
    ms.push(timer.elapsed().as_secs_f64() * 1000.0);
    same_collections(report, "all", &key, old_list.clone(), new_list);
    for summary in &old_list {
        report.count("collection_details");
        let at = (String::from("collection"), summary.id.clone());
        let Some(target) = new.collection_by_legacy_id(&summary.id)? else {
            report.differ("collection_detail_missing", "all", &at, Value::Null);
            continue;
        };
        let old_detail = old.get_work_collection(&summary.id)?;
        let timer = Instant::now();
        let new_detail = new.collection_detail(target)?;
        ms.push(timer.elapsed().as_secs_f64() * 1000.0);
        let old_keys: Vec<Key> = old_detail
            .members
            .iter()
            .map(|m| (m.source.clone(), m.source_id.clone()))
            .collect();
        let new_keys: Vec<Key> = new_detail
            .members
            .iter()
            .map(|m| (m.provider.clone(), m.external_key.clone()))
            .collect();
        sequences(
            report,
            "collection_member_keys",
            "collection_member_order",
            "all",
            &at,
            &old_keys,
            &new_keys,
        );
        let by_key: BTreeMap<Key, &piep_lib::library_collections::CollectionMember> = new_detail
            .members
            .iter()
            .map(|m| ((m.provider.clone(), m.external_key.clone()), m))
            .collect();
        for member in &old_detail.members {
            let member_key = (member.source.clone(), member.source_id.clone());
            let Some(new_member) = by_key.get(&member_key) else {
                continue;
            };
            report.count("collection_member_rows");
            let at = (
                format!("{}:{}", summary.id, member.source),
                member.source_id.clone(),
            );
            report.same(
                "member_missing",
                "all",
                &at,
                member.missing,
                new_member.missing,
                true,
            );
            report.same(
                "member_work",
                "all",
                &at,
                member.work.is_some(),
                new_member.work.is_some(),
                true,
            );
            report.same(
                "member_title",
                "all",
                &at,
                member.title.trim(),
                new_member.title.trim(),
                false,
            );
            report.same(
                "member_author",
                "all",
                &at,
                member.author_name.trim(),
                new_member.author_name.trim(),
                false,
            );
            report.same(
                "member_text_length",
                "all",
                &at,
                member.text_length.max(0) as u64,
                new_member.text_length,
                true,
            );
            report.same(
                "member_cover",
                "all",
                &at,
                member.cover_path.is_some(),
                new_member.cover.is_some(),
                true,
            );
            report.same(
                "member_role",
                "all",
                &at,
                member.member_role.as_str(),
                new_member.role.as_str(),
                true,
            );
            report.same(
                "member_added_by",
                "all",
                &at,
                member.added_by.as_str(),
                new_member.added_by.as_str(),
                true,
            );
            report.same(
                "member_pinned",
                "all",
                &at,
                member.pinned,
                new_member.pinned,
                true,
            );
            report.same(
                "member_note",
                "all",
                &at,
                blank(member.note.as_deref()),
                blank(new_member.note.as_deref()),
                false,
            );
            let old_editions: Vec<Key> = member
                .editions
                .iter()
                .map(|e| (e.source.clone(), e.source_id.clone()))
                .collect();
            let new_editions: Vec<Key> = new_member
                .editions
                .iter()
                .map(|e| (e.provider.clone(), e.external_key.clone()))
                .collect();
            report.same(
                "member_editions",
                "all",
                &at,
                old_editions,
                new_editions,
                false,
            );
            if let (Some(old_work), Some(new_work)) = (&member.work, &new_member.work) {
                report.same(
                    "member_work_title",
                    "all",
                    &at,
                    old_work.title.trim(),
                    new_work.title.trim(),
                    false,
                );
                report.same(
                    "member_work_text_length",
                    "all",
                    &at,
                    old_work.text_length.max(0) as u64,
                    new_work.text_length,
                    true,
                );
            }
        }
    }
    Ok(())
}

/// ライブラリのタブの一覧の一つの組み合わせ。
struct ListCase {
    label: String,
    query: Option<String>,
    filters: Option<SearchV2Params>,
    filter: WorkFilter,
    scope: Option<EntityFacetScope>,
    new_scope: EntityScope,
    sort: &'static str,
    order: &'static str,
}

const LIST_SORTS: [(&str, SeriesSort); 9] = [
    ("work_count", SeriesSort::WorkCount),
    ("downloaded_at", SeriesSort::DownloadedAt),
    ("source_created_at", SeriesSort::SourceCreatedAt),
    ("source_updated_at", SeriesSort::SourceUpdatedAt),
    ("name", SeriesSort::Name),
    ("text_length", SeriesSort::TextLength),
    ("file_size_bytes", SeriesSort::FileSizeBytes),
    ("asset_count", SeriesSort::AssetCount),
    ("current_version", SeriesSort::CurrentVersion),
];

/// 旧棚の一覧の合計（文字数と、修復を除いた改稿の数）を、旧棚の `search_entity_facets` と同じ式で。
fn old_totals(old_db: &Path, kind: &str) -> Result<BTreeMap<Key, (u64, u64)>, Error> {
    let conn =
        rusqlite::Connection::open_with_flags(old_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let revisions = "SUM(CASE WHEN EXISTS(SELECT 1 FROM download_versions v WHERE v.download_id=d.id AND v.version>1) THEN (SELECT COUNT(*) FROM download_versions v WHERE v.download_id=d.id AND v.version>1 AND v.change_kind<>'repair') ELSE MAX(d.current_version-1,0) END)";
    let sql = if kind == "person" {
        format!("SELECT d.source,d.author_id,SUM(d.text_length),{revisions} FROM downloads d WHERE d.author_id IS NOT NULL AND d.author_id<>'' AND d.author_name IS NOT NULL AND d.author_name<>'' GROUP BY d.source,d.author_id")
    } else {
        format!("SELECT ds.series_source,ds.series_key,SUM(d.text_length),{revisions} FROM download_series ds JOIN downloads d ON d.id=ds.download_id GROUP BY ds.series_source,ds.series_key")
    };
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map([], |r| {
        Ok((
            (r.get(0)?, r.get(1)?),
            (r.get::<_, i64>(2)? as u64, r.get::<_, i64>(3)? as u64),
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// ライブラリの「作者・クリエイター」「シリーズ」タブを、最後のページまで比べる。
fn lists(
    report: &mut Report,
    old: &Database,
    old_db: &Path,
    new: &Library,
    ms: &mut Vec<f64>,
) -> Result<(), Error> {
    // 絞り込みの例に使う、作品の多いタグ（名前は出さない）。
    let tag: Option<String> = rusqlite::Connection::open_with_flags(
        new.root().join("library.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?
    .query_row(
        "SELECT t.name FROM work_current_projection p JOIN snapshot_tags st ON st.snapshot_id=p.snapshot_id JOIN tags t ON t.id=st.tag_id GROUP BY t.id ORDER BY count(*) DESC,t.name LIMIT 1 OFFSET 5",
        [],
        |r| r.get(0),
    )
    .ok();
    let mut cases = Vec::new();
    for (sort, _) in LIST_SORTS {
        for order in ["desc", "asc"] {
            cases.push(ListCase {
                label: format!("{sort} {order}"),
                query: None,
                filters: None,
                filter: WorkFilter::default(),
                scope: None,
                new_scope: EntityScope::default(),
                sort,
                order,
            });
        }
    }
    let plain = |label: &str,
                 query: Option<&str>,
                 filters: Option<SearchV2Params>,
                 filter: WorkFilter,
                 scope: Option<EntityFacetScope>,
                 new_scope: EntityScope| ListCase {
        label: label.into(),
        query: query.map(str::to_owned),
        filters,
        filter,
        scope,
        new_scope,
        sort: "work_count",
        order: "desc",
    };
    for (name, old_watch, new_watch) in [
        ("watched", "watched", WatchScope::Watched),
        ("paused", "paused", WatchScope::Paused),
        ("unwatched", "unwatched", WatchScope::Unwatched),
    ] {
        cases.push(plain(
            &format!("watch {name}"),
            None,
            None,
            WorkFilter::default(),
            Some(EntityFacetScope {
                watch: Some(old_watch.into()),
                ..EntityFacetScope::default()
            }),
            EntityScope {
                watch: Some(new_watch),
                ..EntityScope::default()
            },
        ));
    }
    for concluded in [true, false] {
        cases.push(plain(
            &format!("concluded {concluded}"),
            None,
            None,
            WorkFilter::default(),
            Some(EntityFacetScope {
                concluded: Some(concluded),
                ..EntityFacetScope::default()
            }),
            EntityScope {
                concluded: Some(concluded),
                ..EntityScope::default()
            },
        ));
    }
    cases.push(plain(
        "min 3",
        None,
        None,
        WorkFilter::default(),
        Some(EntityFacetScope {
            min_work_count: Some(3),
            ..EntityFacetScope::default()
        }),
        EntityScope {
            min_work_count: Some(3),
            ..EntityScope::default()
        },
    ));
    cases.push(plain(
        "favorite",
        None,
        Some(SearchV2Params {
            favorite: Some(true),
            ..SearchV2Params::default()
        }),
        WorkFilter {
            favorite: Some(true),
            ..WorkFilter::default()
        },
        None,
        EntityScope::default(),
    ));
    cases.push(plain(
        "fanbox",
        None,
        Some(SearchV2Params {
            source: Some("fanbox".into()),
            ..SearchV2Params::default()
        }),
        WorkFilter {
            provider: Some("fanbox".into()),
            ..WorkFilter::default()
        },
        None,
        EntityScope::default(),
    ));
    cases.push(plain(
        "long",
        None,
        Some(SearchV2Params {
            min_char_count: Some(20_000),
            ..SearchV2Params::default()
        }),
        WorkFilter {
            text_length: piep_lib::library::CountRange {
                min: Some(20_000),
                max: None,
            },
            ..WorkFilter::default()
        },
        None,
        EntityScope::default(),
    ));
    if let Some(tag) = tag {
        cases.push(plain(
            "tag",
            None,
            Some(SearchV2Params {
                tags_include: Some(vec![tag.clone()]),
                ..SearchV2Params::default()
            }),
            WorkFilter {
                tags: vec![tag],
                ..WorkFilter::default()
            },
            None,
            EntityScope::default(),
        ));
    }
    for (label, word) in [("word a", "a"), ("word no", "の"), ("word upper", "A")] {
        cases.push(plain(
            label,
            Some(word),
            None,
            WorkFilter::default(),
            None,
            EntityScope::default(),
        ));
    }

    for kind in ["person", "series"] {
        let entity_kind = if kind == "person" {
            EntityKind::Person
        } else {
            EntityKind::Series
        };
        for case in &cases {
            if kind == "person" && case.label.starts_with("concluded") {
                continue;
            }
            let at: Key = (kind.to_owned(), case.label.clone());
            report.count("list_cases");
            let mut old_rows = Vec::new();
            loop {
                let page = old.search_entity_facets(
                    kind,
                    case.query.as_deref(),
                    200,
                    old_rows.len() as i64,
                    case.filters.as_ref(),
                    Some(case.sort),
                    Some(case.order),
                    case.scope.as_ref(),
                )?;
                let done = page.len() < 200;
                old_rows.extend(page);
                if done {
                    break;
                }
            }
            let old_total = old.count_entity_facets(
                kind,
                case.query.as_deref(),
                case.filters.as_ref(),
                case.scope.as_ref(),
            )?;
            let sort = LIST_SORTS
                .iter()
                .find(|(name, _)| *name == case.sort)
                .unwrap()
                .1;
            let mut new_rows = Vec::new();
            let mut new_total: usize;
            loop {
                let timer = Instant::now();
                let page = new.list_entities(&EntityListQuery {
                    kind: entity_kind,
                    query: case.query.clone().unwrap_or_default(),
                    filter: case.filter.clone(),
                    scope: case.new_scope,
                    sort,
                    ascending: case.order == "asc",
                    offset: new_rows.len(),
                    limit: 200,
                })?;
                ms.push(timer.elapsed().as_secs_f64() * 1000.0);
                new_total = page.total;
                let done = page.items.len() < 200;
                new_rows.extend(page.items);
                if done {
                    break;
                }
            }
            report.same("list_total", kind, &at, old_total as usize, new_total, true);
            report.same(
                "list_total_rows",
                kind,
                &at,
                old_rows.len(),
                new_rows.len(),
                true,
            );
            let old_keys: Vec<Key> = old_rows
                .iter()
                .map(|r| (r.source.clone(), r.source_key.clone()))
                .collect();
            let new_keys: Vec<Key> = new_rows
                .iter()
                .map(|r| (r.provider.clone(), r.external_key.clone()))
                .collect();
            sequences(
                report,
                "list_members",
                "list_order",
                kind,
                &at,
                &old_keys,
                &new_keys,
            );
            // 中身は並べ替えの無い一つの組み合わせで見れば足りる。
            if case.label != "work_count desc" {
                continue;
            }
            // 旧棚の一覧の行は合計を持たないので、旧棚の同じ式で引く。
            let old_row_totals = old_totals(old_db, kind)?;
            let by_key: BTreeMap<Key, &piep_lib::library_entity_list::EntityRow> = new_rows
                .iter()
                .map(|r| ((r.provider.clone(), r.external_key.clone()), r))
                .collect();
            for old_row in &old_rows {
                let key = (old_row.source.clone(), old_row.source_key.clone());
                let Some(new_row) = by_key.get(&key) else {
                    continue;
                };
                report.count(if kind == "person" {
                    "list_person_rows"
                } else {
                    "list_series_rows"
                });
                report.same(
                    "list_name",
                    kind,
                    &key,
                    old_row.display_name.as_str(),
                    new_row.name.as_str(),
                    false,
                );
                report.same(
                    "list_count",
                    kind,
                    &key,
                    old_row.count as u64,
                    new_row.work_count,
                    true,
                );
                report.same(
                    "list_description",
                    kind,
                    &key,
                    blank(old_row.description.as_deref()),
                    blank(new_row.description.as_deref()),
                    false,
                );
                report.same(
                    "list_concluded",
                    kind,
                    &key,
                    old_row.is_concluded,
                    new_row.is_concluded,
                    true,
                );
                report.same(
                    "list_sample",
                    kind,
                    &key,
                    old_row.sample_title.as_deref().map(str::trim),
                    new_row.sample_title.as_deref().map(str::trim),
                    false,
                );
                report.same(
                    "list_icon",
                    kind,
                    &key,
                    old_row.icon_path.is_some(),
                    new_row.icon.is_some(),
                    true,
                );
                let new_banner = if kind == "person" {
                    new_row.banner.is_some()
                } else {
                    new_row.cover.is_some()
                };
                report.same(
                    "list_banner",
                    kind,
                    &key,
                    old_row.banner_path.is_some(),
                    new_banner,
                    true,
                );
                let new_cover = if kind == "person" {
                    new_row.icon.is_some() || new_row.banner.is_some()
                } else {
                    new_row.cover.is_some()
                };
                report.same(
                    "list_cover",
                    kind,
                    &key,
                    old_row.cover_path.is_some(),
                    new_cover,
                    true,
                );
                report.same(
                    "list_latest_saved",
                    kind,
                    &key,
                    old_time(old_row.latest_downloaded_at.as_deref()),
                    Some(new_row.latest_saved_at),
                    true,
                );
                report.same(
                    "list_text_length",
                    kind,
                    &key,
                    old_row_totals.get(&key).map(|t| t.0),
                    Some(new_row.total_text_length),
                    true,
                );
                report.same(
                    "list_revisions",
                    kind,
                    &key,
                    old_row_totals.get(&key).map(|t| t.1),
                    Some(new_row.total_revisions),
                    true,
                );
                report.same(
                    "list_latest_updated",
                    kind,
                    &key,
                    old_time(old_row.latest_source_updated_at.as_deref()),
                    Some(new_row.latest_updated_at),
                    true,
                );
            }
        }
    }
    Ok(())
}

pub fn run(old_db: &Path, storage: &Path, new_root: &Path) -> Result<Value, Error> {
    {
        let conn = rusqlite::Connection::open(old_db)?;
        let journal: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='download_save_journal'",
            [],
            |r| r.get(0),
        )?;
        if journal > 0 {
            conn.execute("DELETE FROM download_save_journal", [])?;
        }
    }
    let old = Database::open(old_db, storage)?;
    let new = Library::open(new_root)?;
    let mut report = Report {
        counts: BTreeMap::new(),
        mismatches: BTreeMap::new(),
    };
    let started = Instant::now();
    let mut head_ms = Vec::new();
    let mut series_ms = Vec::new();

    for person in old.list_people()? {
        let key: Key = (person.source.clone(), person.source_key.clone());
        report.count("people");
        let timer = Instant::now();
        let Some(head) = new.entity_head(EntityKind::Person, &key.0, &key.1)? else {
            report.differ("person_missing", "person", &key, Value::Null);
            continue;
        };
        head_ms.push(timer.elapsed().as_secs_f64() * 1000.0);
        report.same(
            "person_name",
            "person",
            &key,
            person.display_name.as_str(),
            head.name.as_str(),
            false,
        );
        report.same(
            "person_description",
            "person",
            &key,
            blank(person.description.as_deref()),
            blank(head.description.as_deref()),
            false,
        );
        report.same(
            "person_links",
            "person",
            &key,
            old_links(person.links_json.as_deref()).len(),
            head.links.len(),
            true,
        );
        report.same(
            "person_icon",
            "person",
            &key,
            person.icon_path.is_some(),
            head.icon.is_some(),
            true,
        );
        report.same(
            "person_banner",
            "person",
            &key,
            person.cover_path.is_some(),
            head.banner.is_some(),
            true,
        );
        report.same(
            "person_work_count",
            "person",
            &key,
            person.work_count.unwrap_or(0) as u64,
            head.work_count,
            true,
        );
        report.same(
            "person_version",
            "person",
            &key,
            Some(person.current_version),
            head.version_number,
            true,
        );
        report.same(
            "person_versions",
            "person",
            &key,
            person.current_version as usize,
            head.versions.len(),
            true,
        );
        same_profile(&mut report, &old, &new, "person", &key, &head)?;

        same_collections(
            &mut report,
            "person",
            &key,
            old.list_collections_for_person(&key.0, &key.1)?,
            new.entity_collections(EntityKind::Person, head.id)?,
        );

        let old_tags: Vec<(String, i64)> = old
            .list_entity_tags("person", &key.0, &key.1, 200)?
            .into_iter()
            .map(|tag| (tag.name, tag.count))
            .collect();
        let new_tags: Vec<(String, i64)> = new
            .entity_tags(EntityKind::Person, head.id, 200)?
            .into_iter()
            .map(|tag| (tag.name, tag.count as i64))
            .collect();
        if old_tags != new_tags {
            let old_set: BTreeSet<_> = old_tags.iter().collect();
            let new_set: BTreeSet<_> = new_tags.iter().collect();
            report.differ(
                if old_set == new_set { "person_tag_order" } else { "person_tags" },
                "person",
                &key,
                json!({"old": old_tags.len(), "new": new_tags.len(), "differing": old_set.symmetric_difference(&new_set).count()}),
            );
        }

        let old_works = old_sequence(
            &old,
            SearchV2Params {
                person_source: Some(key.0.clone()),
                person_key: Some(key.1.clone()),
                sort_by: Some("source_created_at".into()),
                sort_order: Some("desc".into()),
                ..SearchV2Params::default()
            },
        )?;
        let new_works = new_sequence(
            &new,
            WorkFilter {
                person: Some(head.id),
                ..WorkFilter::default()
            },
            WorkSort::Published,
            true,
        )?;
        sequences(
            &mut report,
            "person_works",
            "person_work_order",
            "person",
            &key,
            &old_works,
            &new_works,
        );

        let timer = Instant::now();
        let old_series =
            old.list_entity_series_paged(&key.0, &key.1, None, None, None, 200, None)?;
        let new_series =
            new.person_series(head.id, "", SeriesSort::SourceCreatedAt, false, 0, 200)?;
        series_ms.push(timer.elapsed().as_secs_f64() * 1000.0);
        report.same(
            "person_series_total",
            "person",
            &key,
            old_series.total as usize,
            new_series.total,
            true,
        );
        let old_keys: Vec<Key> = old_series
            .items
            .iter()
            .map(|s| (s.source.clone(), s.source_key.clone()))
            .collect();
        let new_keys: Vec<Key> = new_series
            .items
            .iter()
            .map(|s| (s.provider.clone(), s.external_key.clone()))
            .collect();
        sequences(
            &mut report,
            "person_series",
            "person_series_order",
            "person",
            &key,
            &old_keys,
            &new_keys,
        );
        for (old_item, new_item) in old_series.items.iter().zip(&new_series.items) {
            if (old_item.source.as_str(), old_item.source_key.as_str())
                != (new_item.provider.as_str(), new_item.external_key.as_str())
            {
                continue;
            }
            let series_key = (new_item.provider.clone(), new_item.external_key.clone());
            report.count("person_series_rows");
            report.same(
                "person_series_name",
                "series",
                &series_key,
                old_item.display_name.as_str(),
                new_item.name.as_str(),
                false,
            );
            report.same(
                "person_series_count",
                "series",
                &series_key,
                old_item.count as u64,
                new_item.work_count,
                true,
            );
            report.same(
                "person_series_cover",
                "series",
                &series_key,
                old_item.cover_path.is_some(),
                new_item.cover.is_some(),
                true,
            );
            report.same(
                "person_series_concluded",
                "series",
                &series_key,
                old_item.is_concluded,
                new_item.is_concluded,
                true,
            );
            report.same(
                "person_series_description",
                "series",
                &series_key,
                blank(old_item.description.as_deref()),
                blank(new_item.description.as_deref()),
                false,
            );
            // 旧棚は題の前後の空白を残す。新しい棚は保存のときに落とす（一覧の比較と同じ）。
            report.same(
                "person_series_sample",
                "series",
                &series_key,
                old_item.sample_title.as_deref().map(str::trim),
                new_item.sample_title.as_deref().map(str::trim),
                false,
            );
        }
    }

    for series in old.list_series()? {
        let key: Key = (series.source.clone(), series.source_key.clone());
        report.count("series");
        let timer = Instant::now();
        let Some(head) = new.entity_head(EntityKind::Series, &key.0, &key.1)? else {
            report.differ("series_missing", "series", &key, Value::Null);
            continue;
        };
        head_ms.push(timer.elapsed().as_secs_f64() * 1000.0);
        report.same(
            "series_name",
            "series",
            &key,
            series.title.as_str(),
            head.name.as_str(),
            false,
        );
        report.same(
            "series_description",
            "series",
            &key,
            blank(series.description.as_deref()),
            blank(head.description.as_deref()),
            false,
        );
        report.same(
            "series_cover",
            "series",
            &key,
            series.cover_path.is_some(),
            head.cover.is_some(),
            true,
        );
        report.same(
            "series_concluded",
            "series",
            &key,
            series.is_concluded,
            head.is_concluded,
            true,
        );
        report.same(
            "series_published_count",
            "series",
            &key,
            series.published_content_count,
            head.published_content_count,
            true,
        );
        report.same(
            "series_work_count",
            "series",
            &key,
            series.work_count.unwrap_or(0) as u64,
            head.work_count,
            true,
        );
        report.same(
            "series_version",
            "series",
            &key,
            Some(series.current_version),
            head.version_number,
            true,
        );
        same_profile(&mut report, &old, &new, "series", &key, &head)?;

        same_collections(
            &mut report,
            "series",
            &key,
            old.list_collections_for_series(&key.0, &key.1)?,
            new.entity_collections(EntityKind::Series, head.id)?,
        );

        let old_tags: Vec<(String, i64)> = old
            .list_entity_tags("series", &key.0, &key.1, 200)?
            .into_iter()
            .map(|tag| (tag.name, tag.count))
            .collect();
        let new_tags: Vec<(String, i64)> = new
            .entity_tags(EntityKind::Series, head.id, 200)?
            .into_iter()
            .map(|tag| (tag.name, tag.count as i64))
            .collect();
        if old_tags != new_tags {
            let old_set: BTreeSet<_> = old_tags.iter().collect();
            let new_set: BTreeSet<_> = new_tags.iter().collect();
            report.differ(
                if old_set == new_set { "series_tag_order" } else { "series_tags" },
                "series",
                &key,
                json!({"old": old_tags.len(), "new": new_tags.len(), "differing": old_set.symmetric_difference(&new_set).count()}),
            );
        }

        let old_works = old_sequence(
            &old,
            SearchV2Params {
                series_source: Some(key.0.clone()),
                series_key: Some(key.1.clone()),
                sort_by: Some("series_order".into()),
                sort_order: Some("asc".into()),
                ..SearchV2Params::default()
            },
        )?;
        let new_works = new_sequence(
            &new,
            WorkFilter {
                series: Some(head.id),
                ..WorkFilter::default()
            },
            WorkSort::SeriesOrder,
            false,
        )?;
        sequences(
            &mut report,
            "series_works",
            "series_work_order",
            "series",
            &key,
            &old_works,
            &new_works,
        );
    }

    let mut list_ms = Vec::new();
    lists(&mut report, &old, old_db, &new, &mut list_ms)?;
    let mut collection_ms = Vec::new();
    collections(&mut report, &old, &new, &mut collection_ms)?;

    let percentile = |values: &mut Vec<f64>, q: f64| {
        values.sort_by(|a, b| a.total_cmp(b));
        values
            .get(((values.len().max(1) as f64 - 1.0) * q).round() as usize)
            .copied()
            .unwrap_or(0.0)
    };
    Ok(json!({
        "counts": report.counts,
        "mismatches": report.mismatches,
        "head_ms": {"p50": percentile(&mut head_ms, 0.5), "p99": percentile(&mut head_ms, 0.99)},
        "person_series_ms": {"p50": percentile(&mut series_ms, 0.5), "p99": percentile(&mut series_ms, 0.99)},
        "list_page_ms": {"p50": percentile(&mut list_ms, 0.5), "p99": percentile(&mut list_ms, 0.99)},
        "collection_ms": {"p50": percentile(&mut collection_ms, 0.5), "p99": percentile(&mut collection_ms, 0.99)},
        "seconds": started.elapsed().as_secs_f64(),
    }))
}
