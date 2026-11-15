//! 作者・シリーズの画面を、旧棚の画面と同じ引き方で全員・全シリーズについて比べる。
//! 旧棚は写しを開く（旧 DB は開くときに後片付けを書き込む）。新しい棚は読むだけ。
//!
//! 比べるもの：頭（名前・説明・リンク・画像の有無・完結・話数・作品数・版の番号）、
//! 作品に付いたタグと数、作品の並び（画面の既定の並び。シリーズは `series_order` の昇順、
//! 作者は `source_created_at` の降順）、作者のシリーズの一覧（既定の並び）。
//! 名前や題は出力に出さない（違いは ID と項目名だけ）。

use piep_lib::database::{models::SearchV2Params, Database};
use piep_lib::library::{EntityKind, Library, WorkFilter, WorkListQuery, WorkSort};
use piep_lib::library_entity::SeriesSort;
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
        "seconds": started.elapsed().as_secs_f64(),
    }))
}
