//! 読む画面のページの切り方と、ページの中の行の目印を、旧棚と新しい棚で全作品について比べる。
//! 旧棚は写しを開く（旧 DB は開くときに後片付けを書き込む）。新しい棚は読むだけ。
//!
//! 読書位置としおりは「何ページ目の、何番目の行の目印（`readerAnchor.ts` の
//! `br, p, h2, h3, hr, img, figure, blockquote, li`）」で憶えている。旧棚の作品の位置を新しい棚の
//! 同じ作品で使うには、ページの数と、各ページの目印の並びが同じでなければならない。

use piep_lib::database::Database;
use piep_lib::library::{Library, ResourceScope, WorkListQuery};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::Path;
use std::time::Instant;

type Error = Box<dyn std::error::Error>;
type Key = (String, String);

fn marks(re: &Regex, html: &str) -> Vec<String> {
    re.captures_iter(html)
        .map(|c| c[1].to_ascii_lowercase())
        .collect()
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
    // 大文字小文字を問わず、属性の前で切れる要素名だけ（`<pre>` を `<p` と数えない）。
    let mark = Regex::new(r"(?i)<(br|p|h2|h3|hr|img|figure|blockquote|li)[\s/>]")?;
    // 画像の場所（旧棚はファイルのパス、新しい棚はファイルの識別子）は違って当然なので外して比べる。
    let place = Regex::new(r#"\s(?:data-local-path|src)="[^"]*""#)?;

    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut examples: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    let mut note = |counts: &mut BTreeMap<&str, usize>, name: &'static str, value: Value| {
        *counts.entry(name).or_default() += 1;
        let list = examples.entry(name).or_default();
        if list.len() < 20 {
            list.push(value);
        }
    };
    // 調べる作品を絞る（`取得元:キー` をカンマで）。絞ったときは食い違う箇所の前後を書き出す。
    let only: Vec<String> = std::env::var("PIEP_READER_PAGES_KEYS")
        .map(|keys| keys.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    let mut dump = std::env::var_os("PIEP_READER_PAGES_DUMP")
        .map(|path| std::fs::File::create(path).map(std::io::BufWriter::new))
        .transpose()?;
    let mut query = WorkListQuery {
        limit: 500,
        ..WorkListQuery::default()
    };
    let started = Instant::now();
    loop {
        let page = new.list_works(&query)?;
        for card in &page.items {
            let key: Key = (card.provider.clone(), card.external_key.clone());
            if !only.is_empty() && !only.contains(&format!("{}:{}", key.0, key.1)) {
                continue;
            }
            let work = format!("{} {}", key.0, key.1);
            *counts.entry("works").or_default() += 1;
            let Some(&old_id) = ids.get(&key) else {
                note(&mut counts, "not_in_old_shelf", json!(work));
                continue;
            };
            let scope = match card.edit_id {
                Some(edit) => ResourceScope::Edit(edit),
                None => ResourceScope::Snapshot(card.snapshot_id),
            };
            let rendered = match new.render_document(card.id, scope, &card.provider) {
                Ok(rendered) => rendered,
                Err(error) => {
                    note(
                        &mut counts,
                        "new_error",
                        json!({"work": work, "error": error.to_string()}),
                    );
                    continue;
                }
            };
            let first = match old.get_reader_content_page(old_id, None, 0, false) {
                Ok(first) => first,
                Err(error) => {
                    note(
                        &mut counts,
                        "old_error",
                        json!({"work": work, "error": error}),
                    );
                    continue;
                }
            };
            // 旧棚のページの切り方（ページごとの目印の数）を書き出す。ページの切り方を変える前後で
            // 書き出して比べると、旧棚で読書位置がずれる作品の数が分かる。
            if let Some(dump) = dump.as_mut() {
                let mut per_page = Vec::with_capacity(first.page_count);
                for index in 0..first.page_count {
                    let html = if index == 0 {
                        first.html.clone()
                    } else {
                        old.get_reader_content_page(old_id, None, index, false)?
                            .html
                    };
                    per_page.push(marks(&mark, &html).len().to_string());
                }
                writeln!(dump, "{}\t{}\t{}", key.0, key.1, per_page.join(","))?;
            }
            // 本文の無い作品: 旧棚は空の 1 ページ、新しい棚はページなし。どちらも 1 ページとして読む。
            let new_pages = rendered.pages.len().max(1);
            if !only.is_empty() && first.page_count != new_pages {
                let tail = |html: &str| {
                    html[html.floor_char_boundary(html.len().saturating_sub(400))..].to_string()
                };
                for index in 0..first.page_count {
                    let html = old
                        .get_reader_content_page(old_id, None, index, false)?
                        .html;
                    eprintln!(
                        "== {work} old page {index} len {}\n{}",
                        html.len(),
                        tail(&html)
                    );
                }
                for (index, html) in rendered.pages.iter().enumerate() {
                    eprintln!(
                        "== {work} new page {index} len {}\n{}",
                        html.len(),
                        tail(html)
                    );
                }
            }
            if first.page_count != new_pages {
                note(
                    &mut counts,
                    "page_count",
                    json!({"work": work, "old": first.page_count, "new": new_pages}),
                );
                continue;
            }
            if first.source_page_starts != rendered.source_page_starts {
                note(
                    &mut counts,
                    "source_page_starts",
                    json!({"work": work, "old": first.source_page_starts, "new": rendered.source_page_starts}),
                );
            }
            *counts.entry("pages").or_default() += first.page_count;
            let mut work_marks = true;
            let mut work_html = true;
            for index in 0..first.page_count {
                let old_html = if index == 0 {
                    first.html.clone()
                } else {
                    old.get_reader_content_page(old_id, None, index, false)?
                        .html
                };
                let new_html = rendered.pages.get(index).cloned().unwrap_or_default();
                let (old_marks, new_marks) = (marks(&mark, &old_html), marks(&mark, &new_html));
                if work_marks && old_marks != new_marks {
                    work_marks = false;
                    let at = old_marks
                        .iter()
                        .zip(&new_marks)
                        .position(|(a, b)| a != b)
                        .unwrap_or(old_marks.len().min(new_marks.len()));
                    note(
                        &mut counts,
                        "marks",
                        json!({"work": work, "page": index, "old": old_marks.len(), "new": new_marks.len(), "first_difference": at,
                            "old_around": old_marks.iter().skip(at.saturating_sub(2)).take(5).collect::<Vec<_>>(),
                            "new_around": new_marks.iter().skip(at.saturating_sub(2)).take(5).collect::<Vec<_>>()}),
                    );
                }
                let (old_plain, new_plain) = (
                    place.replace_all(&old_html, ""),
                    place.replace_all(&new_html, ""),
                );
                if !only.is_empty() && old_plain != new_plain {
                    let at = old_plain
                        .bytes()
                        .zip(new_plain.bytes())
                        .position(|(a, b)| a != b)
                        .unwrap_or(old_plain.len().min(new_plain.len()));
                    let around = |text: &str| {
                        let from = text.floor_char_boundary(at.saturating_sub(300));
                        let to = text.floor_char_boundary((at + 300).min(text.len()));
                        text[from..to].to_string()
                    };
                    eprintln!(
                        "== {work} page {index} at {at} (old {} new {})\n-- old\n{}\n-- new\n{}",
                        old_plain.len(),
                        new_plain.len(),
                        around(&old_plain),
                        around(&new_plain)
                    );
                }
                if work_html && old_plain != new_plain {
                    work_html = false;
                    *counts.entry("html_differs").or_default() += 1;
                }
            }
            if work_marks {
                *counts.entry("marks_equal").or_default() += 1;
            }
        }
        match page.next {
            Some(next) => query.after = Some(next),
            None => break,
        }
    }
    if let Some(dump) = dump.as_mut() {
        dump.flush()?;
    }
    Ok(json!({
        "counts": counts,
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "examples": examples,
    }))
}
