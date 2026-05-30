use std::collections::{BTreeSet, HashSet};

use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchTerm {
    pub raw: String,
    pub normalized: String,
    pub is_phrase: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedSearchQuery {
    pub include: Vec<SearchTerm>,
    pub exclude: Vec<SearchTerm>,
}

#[derive(Debug, Clone, Default)]
pub struct SearchDocument {
    pub title: String,
    pub author_name: String,
    pub tags: String,
    pub series_title: String,
    pub excerpt: String,
    pub body: String,
}

impl SearchDocument {
    pub fn fields(&self) -> [(&'static str, &str, f64); 6] {
        [
            ("title", self.title.as_str(), 8.0),
            ("author_name", self.author_name.as_str(), 7.0),
            ("tags", self.tags.as_str(), 6.0),
            ("series_title", self.series_title.as_str(), 5.0),
            ("excerpt", self.excerpt.as_str(), 3.0),
            ("body", self.body.as_str(), 1.0),
        ]
    }
}

pub fn parse_search_query(query: &str) -> ParsedSearchQuery {
    let mut parsed = ParsedSearchQuery::default();
    let mut chars = query.trim().chars().peekable();

    while chars.peek().is_some() {
        while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }

        let mut excluded = false;
        if chars.peek() == Some(&'-') {
            excluded = true;
            chars.next();
        }

        let mut is_phrase = false;
        let mut raw = String::new();
        if chars.peek() == Some(&'"') {
            is_phrase = true;
            chars.next();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                raw.push(c);
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                raw.push(c);
                chars.next();
            }
        }

        let normalized = normalize_search_text(&raw);
        if normalized.is_empty() {
            continue;
        }

        let term = SearchTerm {
            raw: raw.trim().to_string(),
            normalized,
            is_phrase,
        };
        if excluded {
            parsed.exclude.push(term);
        } else {
            parsed.include.push(term);
        }
    }

    parsed
}

pub fn normalize_search_text(input: &str) -> String {
    let mut out = String::new();
    let mut prev_space = true;

    for c in input.chars() {
        let mapped = match c {
            '\u{3000}' => ' ',
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(c as u32 - 0xfee0).unwrap_or(c),
            _ => c,
        };

        for lower in mapped.to_lowercase() {
            if lower.is_alphanumeric() {
                out.push(lower);
                prev_space = false;
            } else if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        }
    }

    out.trim().to_string()
}

pub fn parse_tags_for_search(tags: Option<&str>) -> Vec<String> {
    let Some(tags) = tags else { return Vec::new() };
    let trimmed = tags.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        if let Ok(values) = serde_json::from_str::<Vec<Value>>(trimmed) {
            return values
                .into_iter()
                .filter_map(|v| match v {
                    Value::String(s) => Some(s),
                    other => Some(other.to_string()),
                })
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }

    trimmed
        .replace(['[', ']', '"', '\''], "")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn extract_search_body(data: &Value, source: &str) -> String {
    if source == "pixiv" {
        return data
            .get("text")
            .or_else(|| data.get("detail").and_then(|d| d.get("text")))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
    }

    if source == "fanbox" {
        if let Some(body) = data.get("body") {
            if let Some(blocks) = body.get("blocks").and_then(|b| b.as_array()) {
                return blocks
                    .iter()
                    .filter_map(|block| {
                        let block_type = block.get("type").and_then(|t| t.as_str());
                        if matches!(block_type, Some("p" | "paragraph" | "header")) {
                            block.get("text").and_then(|t| t.as_str())
                        } else {
                            None
                        }
                    })
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<&str>>()
                    .join("\n\n");
            }
            if let Some(text) = body.get("text").and_then(|t| t.as_str()) {
                return text.to_string();
            }
        }
    }

    String::new()
}

pub fn generate_ngrams_limited(text: &str, max_terms: usize) -> Vec<String> {
    let normalized = normalize_search_text(text);
    let mut grams = BTreeSet::new();

    for word in normalized.split_whitespace() {
        if !word.is_empty() {
            grams.insert(word.to_string());
        }
        if grams.len() >= max_terms {
            return grams.into_iter().collect();
        }
    }

    let compact: Vec<char> = normalized.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.is_empty() {
        return grams.into_iter().collect();
    }
    if compact.len() == 1 {
        grams.insert(compact[0].to_string());
        return grams.into_iter().take(max_terms).collect();
    }

    for size in [2usize, 3usize] {
        if compact.len() < size {
            continue;
        }
        for window in compact.windows(size) {
            grams.insert(window.iter().collect::<String>());
            if grams.len() >= max_terms {
                return grams.into_iter().collect();
            }
        }
    }

    grams.into_iter().collect()
}

pub fn query_ngrams(term: &str) -> Vec<String> {
    generate_ngrams_limited(term, 64)
}

pub fn ngram_threshold(count: usize) -> i64 {
    if count <= 2 {
        count as i64
    } else {
        ((count as f64) * 0.45).ceil().max(2.0) as i64
    }
}

pub fn fts_query(parsed: &ParsedSearchQuery) -> Option<String> {
    let mut parts = Vec::new();
    for term in &parsed.include {
        parts.push(format!("\"{}\"", term.normalized.replace('"', "\"\"")));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" AND "))
    }
}

pub fn match_fields_and_score(
    doc: &SearchDocument,
    parsed: &ParsedSearchQuery,
) -> (Vec<String>, f64) {
    let mut fields = Vec::new();
    let mut score = 0.0;

    for (field_name, field_text, weight) in doc.fields() {
        if field_text.is_empty() {
            continue;
        }

        let field_grams: HashSet<String> = generate_ngrams_limited(field_text, 4096)
            .into_iter()
            .collect();
        let mut field_score = 0.0;

        for term in &parsed.include {
            if field_text.contains(&term.normalized) {
                field_score += weight * if term.is_phrase { 12.0 } else { 10.0 };
                continue;
            }

            let grams = query_ngrams(&term.normalized);
            if grams.is_empty() {
                continue;
            }
            let overlap = grams.iter().filter(|g| field_grams.contains(*g)).count();
            let ratio = overlap as f64 / grams.len() as f64;
            if ratio >= 0.45 {
                field_score += weight * ratio * 4.0;
            } else if weight >= 5.0 && fuzzy_close(field_text, &term.normalized) {
                field_score += weight * 2.0;
            }
        }

        if field_score > 0.0 {
            fields.push(field_name.to_string());
            score += field_score;
        }
    }

    (fields, score)
}

pub fn make_match_snippet(doc: &SearchDocument, parsed: &ParsedSearchQuery) -> Option<String> {
    let needles: Vec<&str> = parsed
        .include
        .iter()
        .map(|term| term.normalized.as_str())
        .filter(|s| !s.is_empty())
        .collect();

    for text in [&doc.excerpt, &doc.body] {
        if text.is_empty() {
            continue;
        }
        for needle in &needles {
            if let Some(byte_idx) = text.find(needle) {
                return Some(snippet_around(text, byte_idx, needle.len()));
            }
        }
    }

    None
}

fn snippet_around(text: &str, byte_idx: usize, needle_len: usize) -> String {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let center = chars
        .iter()
        .position(|(idx, _)| *idx >= byte_idx)
        .unwrap_or(chars.len());
    let start = center.saturating_sub(40);
    let end = (center + 80).min(chars.len());
    let mut snippet: String = chars[start..end].iter().map(|(_, c)| *c).collect();
    if start > 0 {
        snippet.insert_str(0, "...");
    }
    if end < chars.len() {
        snippet.push_str("...");
    }
    if needle_len == 0 {
        snippet.truncate(180);
    }
    snippet
}

fn fuzzy_close(field_text: &str, term: &str) -> bool {
    if term.chars().count() < 4 {
        return false;
    }
    field_text
        .split_whitespace()
        .any(|word| normalized_levenshtein(word, term) >= 0.78)
}

pub fn normalized_levenshtein(a: &str, b: &str) -> f64 {
    let ac: Vec<char> = a.chars().collect();
    let bc: Vec<char> = b.chars().collect();
    let max_len = ac.len().max(bc.len());
    if max_len == 0 {
        return 1.0;
    }
    let dist = levenshtein_chars(&ac, &bc);
    1.0 - (dist as f64 / max_len as f64)
}

fn levenshtein_chars(a: &[char], b: &[char]) -> usize {
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }

    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0; b.len() + 1];

    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_width_case_and_symbols() {
        assert_eq!(normalize_search_text("ＡＢＣ　テスト!!"), "abc テスト");
    }

    #[test]
    fn parses_terms_phrases_and_excludes() {
        let parsed = parse_search_query(r#"alpha "beta gamma" -delta"#);
        assert_eq!(parsed.include.len(), 2);
        assert_eq!(parsed.exclude.len(), 1);
        assert!(parsed.include[1].is_phrase);
        assert_eq!(parsed.exclude[0].normalized, "delta");
    }

    #[test]
    fn extracts_pixiv_and_fanbox_body() {
        assert_eq!(
            extract_search_body(&json!({"text": "pixiv body"}), "pixiv"),
            "pixiv body"
        );
        assert_eq!(
            extract_search_body(
                &json!({"body": {"blocks": [
                    {"type": "header", "text": "見出し"},
                    {"type": "p", "text": "本文"},
                    {"type": "image", "text": "ignore"}
                ]}}),
                "fanbox"
            ),
            "見出し\n\n本文"
        );
    }

    #[test]
    fn generates_japanese_ngrams() {
        let grams = generate_ngrams_limited("検索能力", 64);
        assert!(grams.contains(&"検索".to_string()));
        assert!(grams.contains(&"検索能".to_string()));
    }
}
