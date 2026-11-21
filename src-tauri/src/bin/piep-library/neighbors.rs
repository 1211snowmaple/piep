//! 読む画面の前後の作品を、旧棚の読む画面と同じ引き方で全作品について比べる。
//! 旧棚は写しを開く（旧 DB は開くときに後片付けを書き込む）。新しい棚は読むだけ。
//!
//! 旧棚の側は、読む画面（`ReaderPage.tsx`）が使う問い合わせをそのまま使う。シリーズは
//! `series_order` の昇順、作者は `source_created_at` の昇順、コレクションは `get_work_collection`
//! の並び（棚にある作品だけ）。

use piep_lib::database::{models::SearchParams, Database};
use piep_lib::library::{Library, NeighborKind, WorkCard, WorkListQuery, WorkNeighbors};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::Instant;

type Error = Box<dyn std::error::Error>;
type Key = (String, String);

/// 一つの並びでの位置と両隣。題は比べない（作品の中身を出力に出さない）。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Row {
    ordered: bool,
    position: usize,
    total: usize,
    missing: usize,
    previous: Option<Key>,
    next: Option<Key>,
}

fn adjacent(items: &[Key], current: &Key, ordered: bool, missing: usize) -> Option<Row> {
    let index = items.iter().position(|item| item == current)?;
    let previous = index.checked_sub(1).map(|i| items[i].clone());
    let next = items.get(index + 1).cloned();
    (previous.is_some() || next.is_some()).then(|| Row {
        ordered,
        position: index + 1,
        total: items.len(),
        missing,
        previous,
        next,
    })
}

fn key_of(card: &Option<WorkCard>) -> Option<Key> {
    card.as_ref()
        .map(|c| (c.provider.clone(), c.external_key.clone()))
}

fn new_row(row: &WorkNeighbors) -> Row {
    Row {
        ordered: row.ordered,
        position: row.position,
        total: row.total,
        missing: row.missing,
        previous: key_of(&row.previous),
        next: key_of(&row.next),
    }
}

/// 旧棚の読む画面と同じく、並びを最後までたどる。
fn old_sequence(old: &Database, params: SearchParams) -> Result<Vec<Key>, Error> {
    let mut params = SearchParams {
        sort_order: Some("asc".into()),
        projection: Some("bulk".into()),
        limit: Some(200),
        ..params
    };
    let mut items: Vec<Key> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    loop {
        let page = old.search_downloads(&params)?;
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

fn percentile(values: &mut [f64], q: f64) -> f64 {
    values.sort_by(|a, b| a.total_cmp(b));
    values
        .get(((values.len().max(1) as f64 - 1.0) * q).round() as usize)
        .copied()
        .unwrap_or(0.0)
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
    let ids: HashMap<Key, i64> = {
        let sql = rusqlite::Connection::open_with_flags(
            old_db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let mut statement = sql.prepare("SELECT source,source_id,id FROM downloads")?;
        let rows = statement
            .query_map([], |r| Ok(((r.get(0)?, r.get(1)?), r.get(2)?)))?
            .collect::<Result<_, _>>()?;
        rows
    };
    let old = Database::open(old_db, storage)?;
    let new = Library::open(new_root)?;

    let mut series_lists: HashMap<Key, Vec<Key>> = HashMap::new();
    let mut person_lists: HashMap<Key, Vec<Key>> = HashMap::new();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut mismatches: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    let mut times = Vec::new();
    let mut slowest = (0.0, None);
    let mut query = WorkListQuery {
        limit: 500,
        ..WorkListQuery::default()
    };
    let started = Instant::now();
    loop {
        let page = new.list_works(&query)?;
        for card in &page.items {
            let key: Key = (card.provider.clone(), card.external_key.clone());
            *counts.entry("works").or_default() += 1;
            let timer = Instant::now();
            let rows = new.work_neighbors(card.id)?;
            let took = timer.elapsed().as_secs_f64() * 1000.0;
            times.push(took);
            if took > slowest.0 {
                slowest = (took, Some(format!("{} {}", key.0, key.1)));
            }
            let Some(&old_id) = ids.get(&key) else {
                *counts.entry("not_in_old_shelf").or_default() += 1;
                continue;
            };
            let entry = old.get_download(old_id)?;

            // コレクション。旧棚の読む画面と同じく、棚にある作品だけで並べる。
            let mut old_collections = Vec::new();
            for summary in old.list_collections_for_work(&entry.source, &entry.source_id)? {
                let collection = old.get_work_collection(&summary.id)?;
                let items: Vec<Key> = collection
                    .members
                    .iter()
                    .filter(|m| m.download_id.is_some())
                    .map(|m| (m.source.clone(), m.source_id.clone()))
                    .collect();
                let missing = collection.members.len() - items.len();
                let ordered = collection.summary.collection_kind == "ordered";
                old_collections.extend(adjacent(&items, &key, ordered, missing));
            }
            old_collections.sort();
            let mut new_collections: Vec<Row> = rows
                .iter()
                .filter(|r| r.kind == NeighborKind::Collection)
                .map(new_row)
                .collect();
            new_collections.sort();

            let old_series = match entry.series_id.as_deref().filter(|s| !s.is_empty()) {
                Some(series) => {
                    let list_key = (entry.source.clone(), series.to_string());
                    if !series_lists.contains_key(&list_key) {
                        let list = old_sequence(
                            &old,
                            SearchParams {
                                series_source: Some(entry.source.clone()),
                                series_key: Some(series.to_string()),
                                sort_by: Some("series_order".into()),
                                ..SearchParams::default()
                            },
                        )?;
                        series_lists.insert(list_key.clone(), list);
                    }
                    adjacent(&series_lists[&list_key], &key, true, 0)
                }
                None => None,
            };
            let new_series: Vec<Row> = rows
                .iter()
                .filter(|r| r.kind == NeighborKind::Series)
                .map(new_row)
                .collect();

            let person = entry
                .person_id
                .clone()
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| entry.author_id.clone());
            let old_person = if person.is_empty() {
                None
            } else {
                let list_key = (entry.source.clone(), person.clone());
                if !person_lists.contains_key(&list_key) {
                    let list = old_sequence(
                        &old,
                        SearchParams {
                            person_source: Some(entry.source.clone()),
                            person_key: Some(person.clone()),
                            sort_by: Some("source_created_at".into()),
                            ..SearchParams::default()
                        },
                    )?;
                    person_lists.insert(list_key.clone(), list);
                }
                adjacent(&person_lists[&list_key], &key, false, 0)
            };
            let new_person: Vec<Row> = rows
                .iter()
                .filter(|r| r.kind == NeighborKind::Person)
                .map(new_row)
                .collect();

            let mut check = |name: &'static str, old: Vec<Row>, new: Vec<Row>| {
                *counts.entry(name).or_default() += old.len();
                if old != new {
                    *counts
                        .entry(match name {
                            "collection_rows" => "collection_mismatch",
                            "series_rows" => "series_mismatch",
                            _ => "person_mismatch",
                        })
                        .or_default() += 1;
                    let list = mismatches.entry(name).or_default();
                    if list.len() < 30 {
                        list.push(json!({
                            "work": format!("{} {}", key.0, key.1),
                            "old": format!("{old:?}"),
                            "new": format!("{new:?}"),
                        }));
                    }
                }
            };
            check("collection_rows", old_collections, new_collections);
            check("series_rows", old_series.into_iter().collect(), new_series);
            check("person_rows", old_person.into_iter().collect(), new_person);
        }
        match page.next {
            Some(next) => query.after = Some(next),
            None => break,
        }
    }
    let elapsed = started.elapsed().as_secs_f64();
    let p50 = percentile(&mut times, 0.5);
    let p95 = percentile(&mut times, 0.95);
    let p99 = percentile(&mut times, 0.99);
    Ok(json!({
        "counts": counts,
        "series_lists": series_lists.len(),
        "person_lists": person_lists.len(),
        "new_ms": {"p50": p50, "p95": p95, "p99": p99, "max": slowest.0, "slowest": slowest.1},
        "elapsed_seconds": elapsed,
        "mismatches": mismatches,
    }))
}
