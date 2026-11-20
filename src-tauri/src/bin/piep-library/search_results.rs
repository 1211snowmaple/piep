//! 新しい棚の検索索引と旧棚の全文索引に同じ問い合わせを投げ、一致する作品・関連度・順位を
//! 突き合わせる。旧棚の索引は旧棚の文書から一時の場所へ作る（本番の索引は開かない）。
//!
//! 問い合わせは棚の中身から決まった規則で作る（タグ、作者名、題と本文の一部、除外つき）。
//! 出力は件数と問い合わせの番号だけで、問い合わせの文字列は書かない。

use piep_lib::database::tantivy_index;
use piep_lib::database::Database;
use piep_lib::library::{EntityKind, Library};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

type Error = Box<dyn std::error::Error>;
type Identity = (String, String);

fn chars_at(text: &str, at: usize, length: usize) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < at + length {
        return None;
    }
    let piece: String = chars[at..at + length].iter().collect();
    (!piece.trim().is_empty()
        && piece
            .chars()
            .all(|c| !c.is_whitespace() && c != '"' && c != '-'))
    .then_some(piece)
}

pub fn run(old_db: &Path, work: &Path, new_root: &Path, modulus: i64) -> Result<Value, Error> {
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
        let mut statement = sql.prepare("SELECT id,source,source_id FROM downloads ORDER BY id")?;
        let rows = statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_, _>>()?;
        rows
    };
    let old_identity: HashMap<i64, Identity> = downloads
        .iter()
        .map(|(id, source, key)| (*id, (source.clone(), key.clone())))
        .collect();

    // 旧棚の索引を、旧棚の文書から一時の場所へ作る。
    let old_storage = work.join("old-index");
    std::fs::create_dir_all(&old_storage)?;
    let old = Database::open(old_db, &work.join("old-storage"))?;
    let new = Library::open(new_root)?;
    // 文書の違い（題の末尾の空白・説明の前後の空白・前の版のシリーズ）が順位の違いの
    // 原因かを確かめるときは、旧棚の側にその三つだけ新しい棚の値を入れて作る。
    let align = std::env::var_os("PIEP_ALIGN_DOCUMENTS").is_some();
    let mut aligned = 0usize;
    let t = Instant::now();
    let mut queries: Vec<String> = Vec::new();
    let mut reordered_tags = 0usize;
    {
        // 旧棚のタグの並びは連結の順（タグの内部番号の順）で、新しい棚には無い。並びは
        // 「タグ全体と同じ文字列」の一致と、タグの境をまたぐ n-gram を変える。仕組みの
        // 違いだけを見るため、旧棚の側も名前の順に並べ替えて作る（並べ替えた数は数える）。
        let tag_sql = rusqlite::Connection::open_with_flags(
            old_db,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let mut tag_names = tag_sql.prepare(
            "SELECT t.name FROM download_tags dt JOIN tags t ON t.id=dt.tag_id WHERE dt.download_id=?1",
        )?;
        let mut writer = tantivy_index::bulk_writer(&old_storage)?;
        for (index, (id, _, _)) in downloads.iter().enumerate() {
            let Some(mut doc) = old.search_index_document_for_comparison(*id)? else {
                continue;
            };
            let mut names: Vec<String> = tag_names
                .query_map([id], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            names.sort();
            let sorted = names.join(" ");
            if sorted != doc.tags {
                reordered_tags += 1;
                doc.tags = sorted;
            }
            if align {
                let (_, source, key) = &downloads[index];
                if let Some(current) = new
                    .entity_by_key(EntityKind::Work, source, key)?
                    .and_then(|work| new.search_document(work).transpose())
                    .transpose()?
                {
                    let series = current.series_titles.join(" ");
                    if doc.title != current.title
                        || doc.excerpt != current.excerpt
                        || doc.series_title != series
                    {
                        aligned += 1;
                        doc.title = current.title;
                        doc.excerpt = current.excerpt;
                        doc.series_title = series;
                    }
                }
            }
            if (*id) % modulus == 0 {
                let tags: Vec<&str> = doc.tags.split_whitespace().collect();
                if let Some(tag) = tags.first() {
                    queries.push((*tag).to_owned());
                }
                if tags.len() >= 2 {
                    queries.push(format!("{} {}", tags[0], tags[1]));
                    queries.push(format!("{} -{}", tags[0], tags[1]));
                }
                if !doc.author_name.trim().is_empty() && !doc.author_name.contains('"') {
                    queries.push(format!("\"{}\"", doc.author_name.trim()));
                }
                for (at, length) in [(0, 2), (3, 3)] {
                    if let Some(piece) = chars_at(&doc.title, at, length) {
                        queries.push(piece);
                    }
                }
                let body_len = doc.body.chars().count();
                for (at, length) in [(body_len / 3, 2), (body_len / 2, 4), (body_len * 2 / 3, 6)] {
                    if let Some(piece) = chars_at(&doc.body, at, length) {
                        queries.push(piece);
                    }
                }
                if let (Some(a), Some(b)) = (
                    chars_at(&doc.body, body_len / 4, 3),
                    chars_at(&doc.excerpt, 0, 2),
                ) {
                    queries.push(format!("{a} -{b}"));
                }
            }
            let prepared = tantivy_index::prepare_document(&old_storage, &doc)?;
            writer.upsert(prepared)?;
            if index % 2000 == 1999 {
                writer.commit()?;
            }
        }
        writer.commit()?;
    }
    // 新しい棚はまとめて作ったあとに区切りを一つにまとめる。旧棚も同じにして比べる。
    tantivy_index::optimize_segments(&old_storage)?;
    queries.sort();
    queries.dedup();
    let old_build_seconds = t.elapsed().as_secs_f64();

    // 新しい棚の索引を同期する（初回は全部作る）。
    let t = Instant::now();
    let report = new.sync_search_index(&mut |_, _| {})?;
    let new_build_seconds = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let again = new.sync_search_index(&mut |_, _| {})?;
    let resync_seconds = t.elapsed().as_secs_f64();

    let new_identity: HashMap<Vec<u8>, Identity> = {
        let conn = rusqlite::Connection::open_with_flags(
            new_root.join("library.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let mut statement =
            conn.prepare("SELECT work_id,provider,external_key FROM work_current_projection")?;
        let rows = statement
            .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?
            .collect::<Result<_, _>>()?;
        rows
    };

    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut examples: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    let mut note = |kind: &'static str, detail: Value| {
        *counts.entry(kind).or_default() += 1;
        let list = examples.entry(kind).or_default();
        if list.len() < 8 {
            list.push(detail);
        }
    };
    let (mut old_ms, mut new_ms, mut max_delta, mut hits_total) = (0.0, 0.0, 0f32, 0usize);
    let mut new_raw_ms = 0.0;
    for (number, query) in queries.iter().enumerate() {
        let t = Instant::now();
        let found = Arc::new(Mutex::new(Vec::<(i64, f32)>::new()));
        let sink = found.clone();
        tantivy_index::visit_matching_download_scores(&old_storage, query, move |batch| {
            sink.lock().unwrap().extend_from_slice(batch);
            Ok(())
        })?;
        let mut old_hits: Vec<(Identity, f32)> = found
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(id, score)| old_identity.get(id).map(|i| (i.clone(), *score)))
            .collect();
        old_ms += t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let mut new_hits: Vec<(Identity, f32)> = new
            .search_text(query)?
            .into_iter()
            .filter_map(|hit| {
                new_identity
                    .get(hit.work.as_bytes().as_slice())
                    .map(|i| (i.clone(), hit.score))
            })
            .collect();
        new_ms += t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        tantivy_index::visit_matching_download_scores(
            &new_root.join("derived"),
            query,
            |_| Ok(()),
        )?;
        new_raw_ms += t.elapsed().as_secs_f64() * 1000.0;
        hits_total += old_hits.len();
        let order = |hits: &mut Vec<(Identity, f32)>| {
            hits.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)))
        };
        order(&mut old_hits);
        order(&mut new_hits);
        let old_set: BTreeMap<&Identity, f32> = old_hits.iter().map(|(i, s)| (i, *s)).collect();
        let new_set: BTreeMap<&Identity, f32> = new_hits.iter().map(|(i, s)| (i, *s)).collect();
        if old_set.keys().ne(new_set.keys()) {
            let only_old = old_set.keys().filter(|i| !new_set.contains_key(*i)).count();
            let only_new = new_set.keys().filter(|i| !old_set.contains_key(*i)).count();
            note(
                "set",
                json!({"query": number, "old": old_hits.len(), "new": new_hits.len(), "only_old": only_old, "only_new": only_new}),
            );
            continue;
        }
        let mut worst = 0f32;
        for (identity, score) in &old_set {
            let delta = (score - new_set[identity]).abs() / score.abs().max(1e-6);
            worst = worst.max(delta);
        }
        max_delta = max_delta.max(worst);
        if worst > 1e-4 {
            note(
                "score",
                json!({"query": number, "hits": old_hits.len(), "relative_delta": worst}),
            );
        }
        // 順位は、新しい棚の並びが旧棚の関連度で見て下がらないかで見る。索引の区切り方が
        // 違うと関連度の和が最後の桁でずれるので、その幅の中の入れ替わりは同点として扱う。
        let inverted = new_hits.windows(2).position(|pair| {
            let (upper, lower) = (old_set[&pair[0].0], old_set[&pair[1].0]);
            upper < lower - lower.abs() * 1e-5
        });
        if let Some(at) = inverted {
            note(
                "order",
                json!({"query": number, "hits": old_hits.len(), "first_difference": at}),
            );
        }
        let exact_order = old_hits
            .iter()
            .map(|(i, _)| i)
            .eq(new_hits.iter().map(|(i, _)| i));
        if !exact_order {
            note("ties_reordered", json!({"query": number}));
        }
    }
    Ok(json!({
        "downloads": downloads.len(),
        "queries": queries.len(),
        "old_documents_with_reordered_tags": reordered_tags,
        "old_documents_aligned_to_new": aligned,
        "hits_total": hits_total,
        "old_build_seconds": old_build_seconds,
        "new_build_seconds": new_build_seconds,
        "new_report": format!("{report:?}"),
        "resync_seconds": resync_seconds,
        "resync_report": format!("{again:?}"),
        "old_query_ms_total": old_ms,
        "new_query_ms_total": new_ms,
        "new_index_only_ms_total": new_raw_ms,
        "old_segments": tantivy_index::searchable_segment_count(&old_storage)?,
        "new_segments": tantivy_index::searchable_segment_count(&new_root.join("derived"))?,
        "max_relative_score_delta": max_delta,
        "mismatches": counts,
        "examples": examples,
        "seconds": started.elapsed().as_secs_f64(),
    }))
}
