//! 旧棚の一覧・読書・表紙と、新しい棚の同じものを並べて、順序・値・時間を比べる。
//! 旧棚は写しを開く（旧 DB は開くときに後片付けを書き込む）。新しい棚は読むだけ。

use piep_lib::database::{models::DownloadEntry, models::SearchV2Params, Database};
use piep_lib::library::{Library, ResourceScope, WorkCard, WorkListQuery, WorkSort};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

type Error = Box<dyn std::error::Error>;
const PAGE: usize = 80;
const SORTS: [(&str, WorkSort); 9] = [
    ("date", WorkSort::Saved),
    ("published", WorkSort::Published),
    ("updated", WorkSort::Updated),
    ("title", WorkSort::Title),
    ("author", WorkSort::Author),
    ("length", WorkSort::Length),
    ("size", WorkSort::Size),
    ("assets", WorkSort::Media),
    ("version", WorkSort::Version),
];

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn stats(mut v: Vec<f64>) -> Value {
    v.sort_by(|a, b| a.total_cmp(b));
    let at = |q: f64| {
        v.get(((v.len().max(1) as f64 - 1.0) * q).round() as usize)
            .copied()
            .unwrap_or(0.0)
    };
    json!({"n": v.len(), "p50": at(0.5), "p95": at(0.95), "max": at(1.0), "sum": v.iter().sum::<f64>()})
}

fn time(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis())
}

fn old_params(sort: &str, desc: bool, limit: usize, cursor: Option<String>) -> SearchV2Params {
    SearchV2Params {
        sort_by: Some(sort.into()),
        sort_order: Some(if desc { "desc" } else { "asc" }.into()),
        limit: Some(limit as i64),
        cursor,
        ..SearchV2Params::default()
    }
}

fn old_key(name: &str, d: &DownloadEntry) -> Value {
    let saved = time(&d.downloaded_at);
    let created = d.source_created_at.as_deref().and_then(time);
    let updated = d.source_updated_at.as_deref().and_then(time);
    match name {
        "date" => saved.into(),
        "published" => created.or(saved).into(),
        "updated" => updated.or(created).or(saved).into(),
        "title" => d.title.trim_matches(' ').into(),
        "author" => d.author_name.clone().into(),
        "length" => d.text_length.into(),
        "size" => d.file_size_bytes.into(),
        "assets" => d.asset_count.into(),
        _ => d.current_version.into(),
    }
}

fn new_key(sort: WorkSort, c: &WorkCard) -> Value {
    match sort {
        WorkSort::Saved => c.saved_at.into(),
        WorkSort::Published => c.published_at.unwrap_or(c.saved_at).into(),
        WorkSort::Updated => c.updated_at.or(c.published_at).unwrap_or(c.saved_at).into(),
        WorkSort::Title => c.title.clone().into(),
        WorkSort::Author => c.author_name.clone().into(),
        WorkSort::Length => c.text_length.into(),
        WorkSort::Size => c.logical_size.into(),
        WorkSort::Media => c.media_count.into(),
        WorkSort::Version => c.version_number.into(),
    }
}

fn repeat(mut f: impl FnMut() -> Result<(), Error>) -> Result<Vec<f64>, Error> {
    let mut v = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        f()?;
        v.push(ms(started.elapsed()));
    }
    Ok(v)
}

type Key = (String, String);

pub fn run(old_db: &Path, storage: &Path, new_root: &Path, samples: usize) -> Result<Value, Error> {
    // 写しの「中断した保存」の記録は本物の保存先を指す。復旧が本物を動かさないよう、写しから消す。
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
    let mut cards: HashMap<Key, WorkCard> = HashMap::new();
    let mut lists = Vec::new();
    for (name, sort) in SORTS {
        for desc in [true, false] {
            // 旧棚を最後まで。最初のページは冷えた状態の時間になる。
            let mut old_items: Vec<(Key, Value)> = Vec::new();
            let mut old_pages = Vec::new();
            let mut old_cursors = Vec::new();
            let mut cursor = None;
            loop {
                let started = Instant::now();
                let page =
                    old.search_downloads_v2(&old_params(name, desc, PAGE, cursor.clone()))?;
                old_pages.push(ms(started.elapsed()));
                old_cursors.push(cursor.clone());
                old_items.extend(
                    page.items
                        .iter()
                        .map(|d| ((d.source.clone(), d.source_id.clone()), old_key(name, d))),
                );
                cursor = page.next_cursor;
                if cursor.is_none() {
                    break;
                }
            }
            let mut new_items: Vec<(Key, Value)> = Vec::new();
            let mut new_pages = Vec::new();
            let mut new_cursors = Vec::new();
            let mut query = WorkListQuery {
                sort,
                descending: desc,
                limit: PAGE,
                ..WorkListQuery::default()
            };
            loop {
                let started = Instant::now();
                let page = new.list_works(&query)?;
                new_pages.push(ms(started.elapsed()));
                new_cursors.push(query.after.clone());
                for card in page.items {
                    let key = (card.provider.clone(), card.external_key.clone());
                    new_items.push((key.clone(), new_key(sort, &card)));
                    cards.entry(key).or_insert(card);
                }
                match page.next {
                    Some(next) => {
                        query.after = Some(next);
                        query.count = false;
                    }
                    None => break,
                }
            }
            // 温まった最初のページと、真ん中のページを 5 回ずつ。
            let old_middle_cursor = old_cursors[old_cursors.len() / 2].clone();
            let new_middle_cursor = new_cursors[new_cursors.len() / 2].clone();
            let old_first = repeat(|| {
                old.search_downloads_v2(&old_params(name, desc, PAGE, None))?;
                Ok(())
            })?;
            let old_middle = repeat(|| {
                old.search_downloads_v2(&old_params(name, desc, PAGE, old_middle_cursor.clone()))?;
                Ok(())
            })?;
            let new_first = repeat(|| {
                new.list_works(&WorkListQuery {
                    sort,
                    descending: desc,
                    limit: PAGE,
                    ..WorkListQuery::default()
                })?;
                Ok(())
            })?;
            let new_middle = repeat(|| {
                new.list_works(&WorkListQuery {
                    sort,
                    descending: desc,
                    limit: PAGE,
                    after: new_middle_cursor.clone(),
                    count: false,
                    ..WorkListQuery::default()
                })?;
                Ok(())
            })?;
            // 順序と値の突き合わせ。値が同じで順序だけ違えば、同じ値の中の並びの違い。
            let new_values: HashMap<&Key, &Value> = new_items.iter().map(|(k, v)| (k, v)).collect();
            let mut differences = Vec::new();
            let mut missing = Vec::new();
            for (key, value) in &old_items {
                match new_values.get(key) {
                    None => missing.push(json!(key)),
                    Some(other) if *other != value => {
                        differences.push(json!({"work": key, "old": value, "new": other}))
                    }
                    _ => (),
                }
            }
            let same_position = old_items
                .iter()
                .zip(&new_items)
                .filter(|(a, b)| a.0 == b.0)
                .count();
            let divergence = old_items
                .iter()
                .zip(&new_items)
                .position(|(a, b)| a.0 != b.0);
            lists.push(json!({
                "sort": name,
                "descending": desc,
                "old_count": old_items.len(),
                "new_count": new_items.len(),
                "same_order": old_items.iter().map(|i| &i.0).eq(new_items.iter().map(|i| &i.0)),
                "same_position": same_position,
                "first_divergence": divergence.map(|i| json!({"index": i, "old": old_items[i], "new": new_items[i]})),
                "value_differences": differences.len(),
                "value_difference_examples": differences.iter().take(5).collect::<Vec<_>>(),
                "missing_in_new": missing.len(),
                "missing_examples": missing.iter().take(5).collect::<Vec<_>>(),
                "old_ms": {"first_cold": old_pages[0], "first_warm": stats(old_first), "middle": stats(old_middle), "walk": stats(old_pages)},
                "new_ms": {"first_cold": new_pages[0], "first_warm": stats(new_first), "middle": stats(new_middle), "walk": stats(new_pages)},
            }));
            eprintln!(
                "{name} {}: 旧 {} 件・新 {} 件",
                if desc { "降順" } else { "昇順" },
                old_items.len(),
                new_items.len()
            );
        }
    }
    let (reading, covers) = compare_reading(&old, &new, &cards, samples)?;
    Ok(json!({"lists": lists, "reading": reading, "covers": covers}))
}

/// 読書と表紙。保存日順に等間隔で選んだ作品を、どちらも初めて開く状態で。
fn compare_reading(
    old: &Database,
    new: &Library,
    cards: &HashMap<Key, WorkCard>,
    samples: usize,
) -> Result<(Value, Value), Error> {
    let mut order = Vec::new();
    let mut cursor = None;
    loop {
        let page = old.search_downloads_v2(&old_params("date", true, 1000, cursor))?;
        order.extend(page.items);
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    let step = (order.len() / samples.max(1)).max(1);
    let (mut old_read, mut new_read, mut old_cover, mut new_cover) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut same, mut differ, mut one_side) = (0, 0, Vec::new());
    let mut failures = Vec::new();
    for entry in order.iter().step_by(step).take(samples) {
        let work = json!([entry.source, entry.source_id]);
        let Some(card) = cards.get(&(entry.source.clone(), entry.source_id.clone())) else {
            failures.push(json!({"work": work, "error": "新しい棚にありません"}));
            continue;
        };
        let started = Instant::now();
        let result = old
            .get_reader_metadata(entry.id)
            .and_then(|_| old.get_reader_content_page(entry.id, None, 0, false));
        old_read.push(ms(started.elapsed()));
        if let Err(error) = result {
            failures.push(json!({"work": work, "old_error": error}));
        }
        let scope = match card.edit_id {
            Some(edit) => ResourceScope::Edit(edit),
            None => ResourceScope::Snapshot(card.snapshot_id),
        };
        let started = Instant::now();
        let result = new
            .work_card(card.id)
            .and_then(|_| new.reader_document(card.id, scope));
        new_read.push(ms(started.elapsed()));
        if let Err(error) = result {
            failures.push(json!({"work": work, "new_error": error.to_string()}));
        }
        let old_bytes = match entry.cover_path.as_deref() {
            Some(path) => {
                let started = Instant::now();
                let bytes = std::fs::read(path).ok();
                old_cover.push(ms(started.elapsed()));
                bytes
            }
            None => None,
        };
        let new_bytes = match card.cover {
            Some(cover) => {
                let started = Instant::now();
                let file = new.file(cover)?;
                let mut bytes = Vec::new();
                new.objects.export(&file, &mut bytes)?;
                new_cover.push(ms(started.elapsed()));
                Some(bytes)
            }
            None => None,
        };
        match (old_bytes, new_bytes) {
            (Some(a), Some(b)) if Sha256::digest(&a) == Sha256::digest(&b) => same += 1,
            (Some(_), Some(_)) => differ += 1,
            (None, None) => (),
            (a, b) => one_side.push(json!({"work": work, "old": a.is_some(), "new": b.is_some()})),
        }
    }
    Ok((
        json!({"samples": old_read.len(), "old_ms": stats(old_read), "new_ms": stats(new_read), "failures": failures}),
        json!({"same_bytes": same, "different_bytes": differ, "one_side": one_side.len(), "one_side_examples": one_side.iter().take(5).collect::<Vec<_>>(), "old_ms": stats(old_cover), "new_ms": stats(new_cover)}),
    ))
}
