//! 保存した作品の本文の索引。圏外でも本文で探せるようにする。
//!
//! 本文はファイルが正で、索引はそこから作り直せる控えにする。保存・削除のたびに
//! 書き換え、検索の前には保存版の世代と見比べて、ずれていれば作り直す。
//! 日本語は語の切れ目が無いので、FTS5 の trigram で 3 文字ずつ引く。2 文字以下は
//! 索引を使えないので、本文を順に見る（保存した作品だけなので量は限られる）。

use super::super::device_db::records_error;
use super::*;
use regex::Regex;

const MAX_QUERY_CHARS: usize = 100;
const MAX_TERMS: usize = 8;
const MAX_HITS: usize = 500;
const SNIPPET_BEFORE: usize = 24;
const SNIPPET_AFTER: usize = 56;

/// 本文で見つかった作品。最初に見つかったページと、その前後の文。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextHit {
    work_id: i64,
    page: i64,
    snippet: String,
    /// 語を含むページの数。
    pages: i64,
}

/// 本文の HTML から、読める文字だけを取り出す。ルビの読みは検索の邪魔になるので外す。
pub(super) fn page_text(html: &str) -> String {
    static READING: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    static BLOCK: OnceLock<Regex> = OnceLock::new();
    static SPACE: OnceLock<Regex> = OnceLock::new();
    let reading =
        READING.get_or_init(|| Regex::new(r"(?is)<(rt|rp)\b[^>]*>.*?</(rt|rp)\s*>").unwrap());
    let block =
        BLOCK.get_or_init(|| Regex::new(r"(?i)<(br|/p|/div|/h[1-6]|/li|hr)\b[^>]*>").unwrap());
    let tag = TAG.get_or_init(|| Regex::new(r"(?s)<[^>]*>").unwrap());
    let space = SPACE.get_or_init(|| Regex::new(r"[ \t\r\n\u{3000}]+").unwrap());
    let text = reading.replace_all(html, "");
    let text = block.replace_all(&text, "\n");
    let text = tag.replace_all(&text, "");
    let text = decode_entities(&text);
    space.replace_all(text.trim(), " ").into_owned()
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some(end) = tail[..tail.len().min(12)].find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let name = &tail[1..end];
        let decoded = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(char) => {
                out.push(char);
                rest = &tail[end + 1..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 保存版の本文を索引に入れ直す。呼ぶ側が `state.lock` を持ち、保存版の切替と競合させない。
pub(super) fn index_work(state: &OfflineState, work_id: i64) -> Result<(), ClientError> {
    let (manifest, snapshot) = read_manifest(state, work_id)?;
    let generation = snapshot_generation(&snapshot)?;
    let mut texts = Vec::with_capacity(manifest.summary.page_count);
    for page in 0..manifest.summary.page_count {
        texts.push(page_text(
            &read_offline_page(state, work_id, page)?.page.html,
        ));
    }
    state.db.transaction(|tx| {
        tx.execute("DELETE FROM offline_text WHERE work_id = ?1", [work_id])
            .map_err(records_error)?;
        for (page, text) in texts.iter().enumerate() {
            if text.is_empty() {
                continue;
            }
            tx.execute(
                "INSERT INTO offline_text(text, work_id, page) VALUES(?1, ?2, ?3)",
                rusqlite::params![text, work_id, page as i64],
            )
            .map_err(records_error)?;
        }
        tx.execute(
            "INSERT INTO offline_catalog(work_id, generation, source, source_id) VALUES(?1, ?2, ?3, ?4)
             ON CONFLICT(work_id) DO UPDATE SET generation=excluded.generation,
                 source=excluded.source, source_id=excluded.source_id",
            rusqlite::params![
                work_id,
                generation,
                manifest.summary.work.source,
                manifest.summary.work.source_id
            ],
        )
        .map_err(records_error)?;
        Ok(())
    })
}

pub(super) fn remove_work(state: &OfflineState, work_id: i64) -> Result<(), ClientError> {
    state.db.transaction(|tx| {
        tx.execute("DELETE FROM offline_text WHERE work_id = ?1", [work_id])
            .map_err(records_error)?;
        tx.execute("DELETE FROM offline_catalog WHERE work_id = ?1", [work_id])
            .map_err(records_error)?;
        Ok(())
    })
}

/// 索引を保存版にそろえる。読めない保存版は索引から外し、一覧の「問題」に任せる。
pub(super) fn reconcile(state: &OfflineState) -> Result<(), ClientError> {
    let (works, _) = scan_works(state)?;
    let indexed: BTreeMap<i64, String> = state.db.transaction(|tx| {
        let mut query = tx
            .prepare("SELECT work_id, generation FROM offline_catalog")
            .map_err(records_error)?;
        let rows = query
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(records_error)?;
        rows.collect::<Result<_, _>>().map_err(records_error)
    })?;
    let mut present = BTreeSet::new();
    for work in &works {
        let work_id = work.work.id;
        present.insert(work_id);
        let current = read_manifest(state, work_id)
            .and_then(|(_, snapshot)| snapshot_generation(&snapshot))
            .ok();
        if current.is_some() && current.as_ref() != indexed.get(&work_id) {
            // 1 作品が読めなくても、ほかの作品の索引は作る。
            if index_work(state, work_id).is_err() {
                remove_work(state, work_id)?;
            }
        }
    }
    for work_id in indexed.keys().filter(|id| !present.contains(id)) {
        remove_work(state, *work_id)?;
    }
    Ok(())
}

fn terms(query: &str) -> Vec<String> {
    let mut terms: Vec<String> = query
        .split_whitespace()
        .map(str::to_string)
        .filter(|term| !term.is_empty())
        .collect();
    terms.dedup();
    terms.truncate(MAX_TERMS);
    terms
}

fn like_pattern(term: &str) -> String {
    let mut pattern = String::from("%");
    for char in term.chars() {
        if matches!(char, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(char);
    }
    pattern.push('%');
    pattern
}

/// 1 語を含むページ。語が 3 文字以上なら索引で、短ければ順に探す。
fn pages_with(
    tx: &rusqlite::Transaction<'_>,
    term: &str,
) -> Result<Vec<(i64, i64, String)>, ClientError> {
    let (sql, value) = if term.chars().count() >= 3 {
        (
            "SELECT work_id, page, text FROM offline_text WHERE offline_text MATCH ?1 ORDER BY work_id, page",
            format!("\"{}\"", term.replace('"', "\"\"")),
        )
    } else {
        (
            "SELECT work_id, page, text FROM offline_text WHERE text LIKE ?1 ESCAPE '\\' ORDER BY work_id, page",
            like_pattern(term),
        )
    };
    let mut query = tx.prepare(sql).map_err(records_error)?;
    let rows = query
        .query_map([value], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(records_error)?;
    rows.collect::<Result<_, _>>().map_err(records_error)
}

/// 語の前後を少しだけ切り出す。英字の大文字・小文字は区別しない。
fn snippet(text: &str, term: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let needle: Vec<char> = term.chars().flat_map(char::to_lowercase).collect();
    let lower: Vec<char> = chars
        .iter()
        .map(|char| char.to_lowercase().next().unwrap_or(*char))
        .collect();
    let at = (0..lower.len().saturating_sub(needle.len().saturating_sub(1)))
        .find(|&start| lower[start..].starts_with(&needle))
        .unwrap_or(0);
    let from = at.saturating_sub(SNIPPET_BEFORE);
    let to = (at + needle.len() + SNIPPET_AFTER).min(chars.len());
    let mut out = String::new();
    if from > 0 {
        out.push('…');
    }
    out.extend(&chars[from..to]);
    if to < chars.len() {
        out.push('…');
    }
    out
}

/// すべての語を含む作品を、作品の番号の順に返す。語が別のページにあってもよい。
pub(super) fn search(state: &OfflineState, query: &str) -> Result<Vec<TextHit>, ClientError> {
    if query.chars().count() > MAX_QUERY_CHARS {
        return Err(ClientError::remote());
    }
    let terms = terms(query);
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    state.db.transaction(|tx| {
        let mut found: Option<BTreeMap<i64, (i64, String, i64)>> = None;
        for term in &terms {
            let mut works: BTreeMap<i64, (i64, String, i64)> = BTreeMap::new();
            for (work_id, page, text) in pages_with(tx, term)? {
                works
                    .entry(work_id)
                    .and_modify(|entry| entry.2 += 1)
                    .or_insert_with(|| (page, snippet(&text, term), 1));
            }
            found = Some(match found {
                None => works,
                Some(previous) => previous
                    .into_iter()
                    .filter(|(work_id, _)| works.contains_key(work_id))
                    .collect(),
            });
        }
        Ok(found
            .unwrap_or_default()
            .into_iter()
            .take(MAX_HITS)
            .map(|(work_id, (page, snippet, pages))| TextHit {
                work_id,
                page,
                snippet,
                pages,
            })
            .collect())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_text_keeps_the_words_and_drops_tags_readings_and_entities() {
        let html = "<p>吾輩は<ruby>猫<rp>(</rp><rt>ねこ</rt><rp>)</rp></ruby>である。</p><p>名前は&lt;まだ&gt;無い&amp;&#x2603;</p><img src=\"x\">";
        assert_eq!(page_text(html), "吾輩は猫である。 名前は<まだ>無い&☃");
    }

    #[test]
    fn a_broken_entity_is_left_as_written() {
        assert_eq!(page_text("A &unknown; B & C"), "A &unknown; B & C");
    }

    #[test]
    fn the_snippet_is_cut_around_the_word() {
        let text = format!("{}目印{}", "あ".repeat(40), "い".repeat(80));
        let cut = snippet(&text, "目印");
        assert!(cut.starts_with('…') && cut.ends_with('…'));
        assert!(cut.contains("目印"));
        assert_eq!(snippet("Short Text", "text"), "Short Text");
    }

    #[test]
    fn like_patterns_escape_wildcards() {
        assert_eq!(like_pattern("50%_\\"), "%50\\%\\_\\\\%");
    }
}
