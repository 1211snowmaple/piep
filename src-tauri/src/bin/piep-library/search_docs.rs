//! 新しい棚の検索の文書を、旧棚が全文索引に入れる文書と作品ごとに突き合わせる。
//! 旧棚は写しを開き、原本の JSON と添付は旧棚の保存先から読むだけ（書き込まない）。

use piep_lib::database::Database;
use piep_lib::library::{EntityKind, Library};
use piep_lib::library_search::source_url;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

type Error = Box<dyn std::error::Error>;

/// 食い違いの見本に載せる、最初に違う位置の前後。
fn difference(old: &str, new: &str) -> Value {
    let old_chars: Vec<char> = old.chars().collect();
    let new_chars: Vec<char> = new.chars().collect();
    let at = old_chars
        .iter()
        .zip(&new_chars)
        .position(|(a, b)| a != b)
        .unwrap_or(old_chars.len().min(new_chars.len()));
    let window = |chars: &[char]| -> String {
        let from = at.saturating_sub(20);
        chars[from..(at + 40).min(chars.len())].iter().collect()
    };
    json!({
        "at": at,
        "old_len": old_chars.len(),
        "new_len": new_chars.len(),
        "old": window(&old_chars),
        "new": window(&new_chars),
    })
}

/// 空白で区切った語を並べ替えたもの。旧棚はタグやシリーズ名を順の決まらない連結で持つ。
fn words(value: &str) -> Vec<String> {
    let mut words: Vec<String> = value.split_whitespace().map(str::to_owned).collect();
    words.sort();
    words
}

pub fn run(old_db: &Path, storage: &Path, new_root: &Path, modulus: i64) -> Result<Value, Error> {
    let started = Instant::now();
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
    let downloads: Vec<(i64, String, String)> = {
        let sql = rusqlite::Connection::open_with_flags(
            old_db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let mut statement =
            sql.prepare("SELECT id,source,source_id FROM downloads WHERE id % ?1 = 0 ORDER BY id")?;
        let rows = statement
            .query_map([modulus], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_, _>>()?;
        rows
    };
    let old = Database::open(old_db, storage)?;
    let new = Library::open(new_root)?;

    let mut mismatches: BTreeMap<&str, (usize, Vec<Value>)> = BTreeMap::new();
    let mut note = |field: &'static str, id: i64, key: &str, detail: Value| {
        let entry = mismatches.entry(field).or_default();
        entry.0 += 1;
        if entry.1.len() < 8 {
            entry
                .1
                .push(json!({"download": id, "key": key, "detail": detail}));
        }
    };
    let (mut compared, mut missing_old, mut missing_new, mut errors) = (0, 0, 0, Vec::new());
    let (mut old_ms, mut new_ms) = (0.0, 0.0);
    for (id, source, key) in &downloads {
        let t = Instant::now();
        let old_doc = match old.search_index_document_for_comparison(*id) {
            Ok(Some(doc)) => doc,
            Ok(None) => {
                missing_old += 1;
                continue;
            }
            Err(error) => {
                errors.push(json!({"download": id, "side": "old", "error": error}));
                continue;
            }
        };
        old_ms += t.elapsed().as_secs_f64() * 1000.0;
        let Some(work) = new.entity_by_key(EntityKind::Work, source, key)? else {
            missing_new += 1;
            continue;
        };
        let t = Instant::now();
        let new_doc = match new.search_document(work) {
            Ok(Some(doc)) => doc,
            Ok(None) => {
                missing_new += 1;
                continue;
            }
            Err(error) => {
                errors.push(json!({"download": id, "side": "new", "error": error.to_string()}));
                continue;
            }
        };
        new_ms += t.elapsed().as_secs_f64() * 1000.0;
        compared += 1;
        // 旧棚の作品の行の説明は、取得の経路によって前後の空白が落ちている。版の原本には
        // 残っているので、新しい棚は原本のまま持つ。前後の空白は索引の語にならないので、
        // 両端を落として比べ、空白だけの違いは別に数える。
        if old_doc.excerpt != new_doc.excerpt && old_doc.excerpt.trim() == new_doc.excerpt.trim() {
            note("excerpt_edge_whitespace_only", *id, key, json!(null));
        }
        let mut exact = |field: &'static str, old: &str, new: &str| {
            if old != new {
                note(field, *id, key, difference(old, new));
            }
        };
        exact("title", &old_doc.title, &new_doc.title);
        exact("author_name", &old_doc.author_name, &new_doc.author_name);
        exact("author_id", &old_doc.author_id, &new_doc.author_key);
        exact("excerpt", old_doc.excerpt.trim(), new_doc.excerpt.trim());
        exact("body", &old_doc.body, &new_doc.body);
        exact(
            "source_url",
            &old_doc.source_url,
            &source_url(
                &new_doc.provider,
                &new_doc.external_key,
                &new_doc.author_key,
            ),
        );
        let mut sets = |field: &'static str, old: Vec<String>, new: Vec<String>| {
            if old != new {
                note(field, *id, key, json!({"old": old, "new": new}));
            }
        };
        sets("tags", words(&old_doc.tags), words(&new_doc.tags.join(" ")));
        sets(
            "series_title",
            words(&old_doc.series_title),
            words(&new_doc.series_titles.join(" ")),
        );
        sets(
            "asset_kinds",
            words(&old_doc.asset_kinds),
            words(&new_doc.asset_kinds.join(" ")),
        );
        if old_doc.text_length != new_doc.text_length {
            note(
                "text_length",
                *id,
                key,
                json!({"old": old_doc.text_length, "new": new_doc.text_length}),
            );
        }
    }
    Ok(json!({
        "downloads": downloads.len(),
        "compared": compared,
        "missing_old": missing_old,
        "missing_new": missing_new,
        "errors": errors,
        "old_ms_total": old_ms,
        "new_ms_total": new_ms,
        "seconds": started.elapsed().as_secs_f64(),
        "mismatches": mismatches
            .into_iter()
            .map(|(field, (count, examples))| (field.to_owned(), json!({"count": count, "examples": examples})))
            .collect::<serde_json::Map<_, _>>(),
    }))
}
