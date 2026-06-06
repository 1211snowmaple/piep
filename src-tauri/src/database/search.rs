use std::collections::{BTreeSet, HashSet};

use serde_json::Value;

use super::models::{SearchHighlight, SearchHighlightSegment};
use super::search_normalization::{
    find_token_match_span, normalize_for_search, query_variants, reading_kana_text,
    reading_romaji_text, search_index_text, synonym_variants,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchTerm {
    pub raw: String,
    pub normalized: String,
    pub variants: Vec<String>,
    pub synonyms: Vec<String>,
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

#[derive(Debug, Clone, PartialEq)]
pub struct SearchScoreReason {
    pub field: String,
    pub match_type: String,
    pub term: String,
    pub contribution: f64,
    pub detail: Option<String>,
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
            variants: query_variants(raw.trim()),
            synonyms: synonym_variants(raw.trim()),
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
    normalize_for_search(input)
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
    let normalized = search_index_text(text);
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

#[allow(dead_code)]
pub fn ngram_threshold(count: usize) -> i64 {
    if count <= 2 {
        count as i64
    } else {
        ((count as f64) * 0.45).ceil().max(2.0) as i64
    }
}

#[allow(dead_code)]
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
) -> (Vec<String>, Vec<SearchScoreReason>, f64) {
    let mut fields = Vec::new();
    let mut reasons = Vec::new();
    let mut score = 0.0;

    for (field_name, field_text, weight) in doc.fields() {
        if field_text.is_empty() {
            continue;
        }

        let field_normalized = normalize_search_text(field_text);
        let allow_expensive_analysis = field_name != "body";
        let field_reading_kana = if allow_expensive_analysis {
            reading_kana_text(field_text)
        } else {
            String::new()
        };
        let field_reading_romaji = if allow_expensive_analysis {
            reading_romaji_text(field_text)
        } else {
            String::new()
        };
        let field_grams: HashSet<String> = if allow_expensive_analysis {
            generate_ngrams_limited(field_text, 4096)
                .into_iter()
                .collect()
        } else {
            HashSet::new()
        };
        let mut field_score = 0.0;

        for term in &parsed.include {
            let direct_match = term.variants.iter().any(|variant| {
                !variant.is_empty()
                    && (field_normalized.contains(variant) || field_text.contains(variant))
            });
            if direct_match {
                let boost = if term.is_phrase { 12.0 } else { 10.0 };
                let amount = weight * boost;
                field_score += amount;
                reasons.push(SearchScoreReason {
                    field: field_name.to_string(),
                    match_type: if term.is_phrase {
                        "phrase".to_string()
                    } else {
                        "exact".to_string()
                    },
                    term: term.raw.clone(),
                    contribution: amount,
                    detail: None,
                });
                continue;
            }

            let reading_match = term.variants.iter().any(|variant| {
                !variant.is_empty()
                    && !field_reading_kana.is_empty()
                    && field_reading_kana.contains(variant)
            });
            if reading_match {
                let amount = weight * 6.0;
                field_score += amount;
                reasons.push(SearchScoreReason {
                    field: field_name.to_string(),
                    match_type: "reading".to_string(),
                    term: term.raw.clone(),
                    contribution: amount,
                    detail: Some(field_reading_kana.clone()),
                });
                continue;
            }

            let romaji_match = term.variants.iter().any(|variant| {
                !variant.is_empty()
                    && variant.is_ascii()
                    && !field_reading_romaji.is_empty()
                    && field_reading_romaji.contains(variant)
            });
            if romaji_match {
                let amount = weight * 5.5;
                field_score += amount;
                reasons.push(SearchScoreReason {
                    field: field_name.to_string(),
                    match_type: "romaji".to_string(),
                    term: term.raw.clone(),
                    contribution: amount,
                    detail: Some(field_reading_romaji.clone()),
                });
                continue;
            }

            let synonym_match = term.synonyms.iter().any(|synonym| {
                let variants = query_variants(synonym);
                variants
                    .iter()
                    .any(|variant| !variant.is_empty() && field_normalized.contains(variant))
            });
            if synonym_match {
                let amount = weight * 4.8;
                field_score += amount;
                reasons.push(SearchScoreReason {
                    field: field_name.to_string(),
                    match_type: "synonym".to_string(),
                    term: term.raw.clone(),
                    contribution: amount,
                    detail: Some(term.synonyms.join(", ")),
                });
                continue;
            }

            if !allow_expensive_analysis {
                continue;
            }

            let grams = query_ngrams(&term.normalized);
            if grams.is_empty() {
                continue;
            }
            let overlap = grams.iter().filter(|g| field_grams.contains(*g)).count();
            let ratio = overlap as f64 / grams.len() as f64;
            if ratio >= 0.45 {
                let amount = weight * ratio * 4.0;
                field_score += amount;
                reasons.push(SearchScoreReason {
                    field: field_name.to_string(),
                    match_type: "ngram".to_string(),
                    term: term.raw.clone(),
                    contribution: amount,
                    detail: Some(format!("{:.0}% gram overlap", ratio * 100.0)),
                });
            } else if weight >= 5.0 && fuzzy_close(&field_normalized, &term.normalized) {
                let amount = weight * 2.0;
                field_score += amount;
                reasons.push(SearchScoreReason {
                    field: field_name.to_string(),
                    match_type: "fuzzy".to_string(),
                    term: term.raw.clone(),
                    contribution: amount,
                    detail: None,
                });
            }
        }

        if field_score > 0.0 {
            fields.push(field_name.to_string());
            score += field_score;
        }
    }

    (fields, reasons, score)
}

pub fn make_match_highlights(
    doc: &SearchDocument,
    parsed: &ParsedSearchQuery,
) -> Vec<SearchHighlight> {
    let mut highlights = Vec::new();
    for (field, text, _) in doc.fields() {
        if text.is_empty() {
            continue;
        }
        for term in &parsed.include {
            let needles = [term.raw.as_str(), term.normalized.as_str()];
            if let Some((byte_idx, byte_len)) = needles
                .iter()
                .filter(|needle| !needle.is_empty())
                .find_map(|needle| find_case_insensitive(text, needle))
            {
                highlights.push(SearchHighlight {
                    field: field.to_string(),
                    text: text.to_string(),
                    segments: highlight_segments(text, byte_idx, byte_len),
                    source_chunk_id: None,
                    match_type: Some(if term.is_phrase { "phrase" } else { "exact" }.to_string()),
                });
                break;
            }
            if field != "body" {
                if let Some((byte_idx, byte_len, _surface)) =
                    find_token_match_span(text, &term.variants)
                {
                    highlights.push(SearchHighlight {
                        field: field.to_string(),
                        text: text.to_string(),
                        segments: highlight_segments(text, byte_idx, byte_len),
                        source_chunk_id: None,
                        match_type: Some("reading".to_string()),
                    });
                    break;
                }
            }
            for synonym in &term.synonyms {
                if let Some((byte_idx, byte_len)) = find_case_insensitive(text, synonym) {
                    highlights.push(SearchHighlight {
                        field: field.to_string(),
                        text: text.to_string(),
                        segments: highlight_segments(text, byte_idx, byte_len),
                        source_chunk_id: None,
                        match_type: Some("synonym".to_string()),
                    });
                    break;
                }
            }
        }
        if highlights.len() >= 3 {
            break;
        }
    }
    highlights
}

fn find_case_insensitive(haystack: &str, needle: &str) -> Option<(usize, usize)> {
    if let Some(idx) = haystack.find(needle) {
        return Some((idx, needle.len()));
    }
    let lower_haystack = haystack.to_lowercase();
    let lower_needle = needle.to_lowercase();
    lower_haystack
        .find(&lower_needle)
        .map(|idx| (idx, lower_needle.len()))
        .filter(|(idx, len)| {
            haystack.is_char_boundary(*idx) && haystack.is_char_boundary(idx + len)
        })
}

fn highlight_segments(
    text: &str,
    byte_idx: usize,
    needle_len: usize,
) -> Vec<SearchHighlightSegment> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let center = chars
        .iter()
        .position(|(idx, _)| *idx >= byte_idx)
        .unwrap_or(chars.len());
    let start = center.saturating_sub(40);
    let match_end_byte = byte_idx.saturating_add(needle_len);
    let match_end = chars
        .iter()
        .position(|(idx, _)| *idx >= match_end_byte)
        .unwrap_or(chars.len());
    let end = (match_end + 80).min(chars.len());
    let mut segments = Vec::new();
    if start > 0 {
        segments.push(SearchHighlightSegment {
            text: "...".to_string(),
            matched: false,
        });
    }
    let prefix: String = chars[start..center].iter().map(|(_, c)| *c).collect();
    if !prefix.is_empty() {
        segments.push(SearchHighlightSegment {
            text: prefix,
            matched: false,
        });
    }
    let matched: String = chars[center..match_end.max(center + 1).min(chars.len())]
        .iter()
        .map(|(_, c)| *c)
        .collect();
    if !matched.is_empty() {
        segments.push(SearchHighlightSegment {
            text: matched,
            matched: true,
        });
    }
    let suffix: String = chars[match_end.max(center + 1).min(chars.len())..end]
        .iter()
        .map(|(_, c)| *c)
        .collect();
    if !suffix.is_empty() {
        segments.push(SearchHighlightSegment {
            text: suffix,
            matched: false,
        });
    }
    if end < chars.len() {
        segments.push(SearchHighlightSegment {
            text: "...".to_string(),
            matched: false,
        });
    }
    segments
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
        assert_eq!(normalize_search_text("ＡＢＣ　テスト!!"), "abc てすと");
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
