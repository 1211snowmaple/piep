use std::collections::HashSet;
use std::sync::OnceLock;

use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use lindera::tokenizer::Tokenizer;
use unicode_normalization::UnicodeNormalization;
use wana_kana::{ConvertJapanese, IsJapaneseStr};

static TOKENIZER: OnceLock<Option<Tokenizer>> = OnceLock::new();
static SYNONYM_READINGS: OnceLock<Vec<Vec<SynonymEntry>>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalyzedToken {
    pub surface: String,
    pub normalized: String,
    pub base: String,
    pub reading_kana: String,
    pub reading_romaji: String,
    pub byte_start: usize,
    pub byte_end: usize,
}

/// Every indexable form of one field value.
///
/// Producing these together matters: each form needs the same morphological
/// analysis, and analysing a novel-length body is by far the most expensive
/// step in building the index. Deriving them one call at a time re-ran that
/// analysis about a dozen times per document.
#[derive(Debug, Clone, Default)]
pub struct IndexedText {
    /// NFKC-folded, lowercased, kana-unified surface text, still contiguous so
    /// that n-grams spanning word boundaries survive.
    pub normalized: String,
    /// What the n-gram field indexes: the contiguous normalized text plus the
    /// dictionary base forms that differ from the surface, so conjugated verbs
    /// stay reachable. Readings are deliberately absent - they have their own
    /// fields and the query is expanded across all of them anyway.
    pub surface: String,
    pub reading_kana: String,
    pub reading_romaji: String,
}

/// Collects strings in insertion order while rejecting duplicates in constant
/// time. The linear scan this replaces turned long documents quadratic.
#[derive(Default)]
struct UniqueValues {
    values: Vec<String>,
    seen: HashSet<String>,
}

impl UniqueValues {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            values: Vec::with_capacity(capacity),
            seen: HashSet::with_capacity(capacity),
        }
    }

    fn push(&mut self, value: impl Into<String>) {
        let value = value.into();
        let value = value.trim();
        if value.is_empty() || self.seen.contains(value) {
            return;
        }
        self.seen.insert(value.to_string());
        self.values.push(value.to_string());
    }

    fn into_vec(self) -> Vec<String> {
        self.values
    }

    fn join(self) -> String {
        self.values.join(" ")
    }
}

struct SynonymEntry {
    term: &'static str,
    normalized: String,
    reading_kana: String,
    reading_romaji: String,
}

const SYNONYM_GROUPS: &[&[&str]] = &[
    &[
        "小説",
        "しょうせつ",
        "shousetsu",
        "物語",
        "ストーリー",
        "story",
        "novel",
    ],
    &["イラスト", "絵", "画像", "art", "illustration"],
    &["漫画", "マンガ", "manga", "コミック", "comic"],
    &["成人向け", "r18", "r-18", "18禁"],
    &["恋愛", "ラブ", "love", "romance"],
];

/// The synonym table is fixed, so its readings are analysed once per process
/// instead of on every field and every query.
fn synonym_readings() -> &'static [Vec<SynonymEntry>] {
    SYNONYM_READINGS.get_or_init(|| {
        SYNONYM_GROUPS
            .iter()
            .map(|group| {
                group
                    .iter()
                    .map(|term| {
                        let indexed = index_text(term);
                        SynonymEntry {
                            term,
                            normalized: indexed.normalized,
                            reading_kana: indexed.reading_kana,
                            reading_romaji: indexed.reading_romaji,
                        }
                    })
                    .collect()
            })
            .collect()
    })
}

/// Derives every indexable form of `input` from a single morphological pass.
pub fn index_text(input: &str) -> IndexedText {
    let normalized = normalize_for_search(input);
    if input.trim().is_empty() {
        return IndexedText::default();
    }

    let tokens = analyze_text(input);
    let mut surface = UniqueValues::with_capacity(tokens.len() + 1);
    // The contiguous text goes in first: n-grams that straddle two tokens can
    // only be produced from text that has not been split apart.
    surface.push(normalized.as_str());
    let mut reading_kana = Vec::with_capacity(tokens.len());
    let mut reading_romaji = Vec::with_capacity(tokens.len());

    for token in &tokens {
        if !token.base.is_empty() && token.base != token.normalized {
            surface.push(token.base.as_str());
        }
        if !token.reading_kana.is_empty() {
            reading_kana.push(token.reading_kana.as_str());
        } else if !token.normalized.is_empty() {
            reading_kana.push(token.normalized.as_str());
        }
        if !token.reading_romaji.is_empty() {
            reading_romaji.push(token.reading_romaji.as_str());
        } else if token.normalized.is_ascii() && !token.normalized.is_empty() {
            reading_romaji.push(token.normalized.as_str());
        }
    }

    IndexedText {
        surface: surface.join(),
        normalized,
        reading_kana: reading_kana.join(" "),
        reading_romaji: reading_romaji.join(" "),
    }
}

pub fn normalize_for_search(input: &str) -> String {
    let nfkc = input.nfkc().collect::<String>();
    let mut out = String::new();
    let mut prev_space = true;

    for c in nfkc.chars() {
        let mapped = katakana_to_hiragana_char(c);
        for lower in mapped.to_lowercase() {
            if lower.is_alphanumeric() || is_japanese_search_char(lower) {
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

pub fn analyze_text(input: &str) -> Vec<AnalyzedToken> {
    if input.trim().is_empty() {
        return Vec::new();
    }

    if let Some(tokenizer) = tokenizer() {
        if let Ok(tokens) = tokenizer.tokenize(input) {
            let mut out = Vec::with_capacity(tokens.len());
            for mut token in tokens {
                let surface = token.surface.to_string();
                let details = token
                    .details
                    .as_ref()
                    .map(|values| values.iter().map(|v| v.to_string()).collect::<Vec<_>>())
                    .unwrap_or_default();
                let base = token
                    .get("base_form")
                    .map(str::to_string)
                    .or_else(|| details.get(6).cloned())
                    .unwrap_or_else(|| surface.clone());
                let raw_reading = token
                    .get("reading")
                    .map(str::to_string)
                    .or_else(|| token.get("pronunciation").map(str::to_string))
                    .or_else(|| details.get(7).cloned())
                    .unwrap_or_default();
                let reading_kana = normalize_kana_reading(&raw_reading);
                let normalized = normalize_for_search(&surface);
                if normalized.is_empty() && reading_kana.is_empty() {
                    continue;
                }
                out.push(AnalyzedToken {
                    surface,
                    normalized,
                    base: normalize_for_search(&base),
                    reading_romaji: reading_kana.as_str().to_romaji(),
                    reading_kana,
                    byte_start: token.byte_start,
                    byte_end: token.byte_end,
                });
            }
            if !out.is_empty() {
                return out;
            }
        }
    }

    fallback_tokens(input)
}

pub fn reading_kana_text(input: &str) -> String {
    index_text(input).reading_kana
}

pub fn reading_romaji_text(input: &str) -> String {
    index_text(input).reading_romaji
}

/// The query-side expansion: every written form a term might have been indexed
/// under. Field values use [`index_text`] instead, which keeps the readings in
/// their own fields rather than repeating them here.
pub fn search_index_text(input: &str) -> String {
    let indexed = index_text(input);
    let mut values = UniqueValues::with_capacity(8);
    for variant in variants_from(input, &indexed) {
        values.push(variant);
    }
    values.push(indexed.surface);
    values.push(indexed.reading_kana);
    values.push(indexed.reading_romaji);
    values.join()
}

pub fn query_variants(input: &str) -> Vec<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let indexed = index_text(trimmed);
    variants_from(trimmed, &indexed)
}

fn variants_from(input: &str, indexed: &IndexedText) -> Vec<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let mut values = UniqueValues::with_capacity(12);
    values.push(indexed.normalized.as_str());
    values.push(normalize_kana_reading(trimmed));
    values.push(indexed.reading_kana.as_str());
    values.push(indexed.reading_romaji.as_str());

    if trimmed.is_romaji() {
        let kana = trimmed.to_kana();
        values.push(normalize_for_search(&kana));
        values.push(normalize_kana_reading(&kana));
    }

    if trimmed.is_kana() {
        values.push(trimmed.to_romaji());
    }

    for entry in matching_synonyms(indexed) {
        values.push(entry.normalized.as_str());
        values.push(entry.reading_kana.as_str());
        values.push(entry.reading_romaji.as_str());
    }

    values.into_vec()
}

pub fn synonym_variants(input: &str) -> Vec<String> {
    let indexed = index_text(input);
    matching_synonyms(&indexed)
        .into_iter()
        .map(|entry| entry.term.to_string())
        .collect()
}

fn matching_synonyms(indexed: &IndexedText) -> Vec<&'static SynonymEntry> {
    let mut out = Vec::new();
    for group in synonym_readings() {
        let matched = group.iter().any(|entry| {
            indexed.normalized == entry.normalized
                || (!entry.reading_kana.is_empty() && indexed.reading_kana == entry.reading_kana)
                || (!entry.reading_romaji.is_empty()
                    && indexed.reading_romaji == entry.reading_romaji)
        });
        if !matched {
            continue;
        }
        for entry in group {
            if entry.normalized != indexed.normalized {
                out.push(entry);
            }
        }
    }
    out
}

pub fn expand_query_for_tantivy(query: &str) -> String {
    let mut values = UniqueValues::with_capacity(16);
    for part in query.split_whitespace() {
        let cleaned = part.trim_matches('"').trim_start_matches('-');
        for variant in query_variants(cleaned) {
            values.push(variant);
        }
    }
    values.join()
}

pub fn find_token_match_span(text: &str, variants: &[String]) -> Option<(usize, usize, String)> {
    if variants.is_empty() {
        return None;
    }
    let normalized_variants = variants
        .iter()
        .flat_map(|value| query_variants(value))
        .collect::<Vec<_>>();

    for token in analyze_text(text) {
        let token_values = [
            token.normalized.as_str(),
            token.base.as_str(),
            token.reading_kana.as_str(),
            token.reading_romaji.as_str(),
        ];
        if normalized_variants.iter().any(|needle| {
            !needle.is_empty()
                && token_values
                    .iter()
                    .any(|value| !value.is_empty() && value.contains(needle))
        }) {
            return Some((
                token.byte_start,
                token.byte_end.saturating_sub(token.byte_start),
                token.surface,
            ));
        }
    }
    None
}

fn tokenizer() -> Option<&'static Tokenizer> {
    TOKENIZER
        .get_or_init(|| {
            load_dictionary("embedded://ipadic")
                .map(|dictionary| Tokenizer::new(Segmenter::new(Mode::Normal, dictionary, None)))
                .ok()
        })
        .as_ref()
}

fn normalize_kana_reading(value: &str) -> String {
    let nfkc = value.nfkc().collect::<String>();
    let converted = nfkc
        .chars()
        .map(katakana_to_hiragana_char)
        .collect::<String>();
    normalize_for_search(&converted)
}

fn katakana_to_hiragana_char(c: char) -> char {
    match c {
        '\u{30a1}'..='\u{30f6}' => char::from_u32(c as u32 - 0x60).unwrap_or(c),
        _ => c,
    }
}

fn is_japanese_search_char(c: char) -> bool {
    matches!(
        c,
        '\u{3040}'..='\u{309f}'
            | '\u{30a0}'..='\u{30ff}'
            | '\u{3400}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}'
    )
}

fn fallback_tokens(input: &str) -> Vec<AnalyzedToken> {
    let mut tokens = Vec::new();
    let mut start: Option<usize> = None;
    for (idx, c) in input.char_indices() {
        if c.is_whitespace() {
            if let Some(start_idx) = start.take() {
                push_fallback_token(input, start_idx, idx, &mut tokens);
            }
        } else if start.is_none() {
            start = Some(idx);
        }
    }
    if let Some(start_idx) = start {
        push_fallback_token(input, start_idx, input.len(), &mut tokens);
    }
    tokens
}

fn push_fallback_token(input: &str, start: usize, end: usize, tokens: &mut Vec<AnalyzedToken>) {
    let surface = input[start..end].to_string();
    let normalized = normalize_for_search(&surface);
    if normalized.is_empty() {
        return;
    }
    let reading_kana = if surface.as_str().is_kana() {
        normalize_kana_reading(&surface)
    } else {
        String::new()
    };
    let reading_romaji = if !reading_kana.is_empty() {
        reading_kana.as_str().to_romaji()
    } else if normalized.is_ascii() {
        normalized.clone()
    } else {
        String::new()
    };
    tokens.push(AnalyzedToken {
        surface,
        normalized: normalized.clone(),
        base: normalized,
        reading_kana,
        reading_romaji,
        byte_start: start,
        byte_end: end,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_width_case_and_kana() {
        assert_eq!(normalize_for_search("ＡＢＣ　テスト!!"), "abc てすと");
    }

    #[test]
    fn expands_romaji_to_kana() {
        let variants = query_variants("tesuto");
        assert!(variants.contains(&"tesuto".to_string()));
        assert!(variants.contains(&"てすと".to_string()));
    }

    #[test]
    fn extracts_reading_for_kanji_words() {
        let variants = query_variants("小説");
        assert!(variants.iter().any(|value| value.contains("しょうせつ")));
        assert!(variants.iter().any(|value| value.contains("shousetsu")));
    }

    #[test]
    fn one_pass_matches_the_per_field_helpers() {
        let text = "静かな教室で手紙を書き留めた。Shizuka na kyoushitsu.";
        let indexed = index_text(text);
        assert_eq!(indexed.normalized, normalize_for_search(text));
        assert_eq!(indexed.reading_kana, reading_kana_text(text));
        assert_eq!(indexed.reading_romaji, reading_romaji_text(text));
        // The n-gram field keeps the text contiguous so matches can span two
        // tokens, which per-token values alone would break.
        assert!(indexed.surface.starts_with(&indexed.normalized));
    }

    #[test]
    fn repeated_wording_does_not_grow_the_indexed_payload() {
        // The n-gram field is the contiguous text plus the base forms that
        // differ from the surface. The text has to grow with the document; the
        // base forms must not, however often the same wording comes back.
        let phrase = "走った猫と歩いた犬を見た。";
        let once = index_text(phrase);
        let repeated = index_text(&phrase.repeat(50));
        let extra = |value: &IndexedText| value.surface.len() - value.normalized.len();
        assert!(
            extra(&once) > 0,
            "conjugated verbs should contribute base forms"
        );
        assert_eq!(
            extra(&repeated),
            extra(&once),
            "the same base forms must be recorded once, not once per occurrence"
        );
    }

    #[test]
    #[ignore = "measurement harness, run with --ignored --nocapture"]
    fn measure_body_preparation() {
        use std::time::Instant;
        // Real prose keeps introducing new vocabulary, so the deduplication set
        // grows with the document. Repeating one sentence would keep it tiny and
        // hide exactly the cost this harness exists to measure.
        let nouns = [
            "教室",
            "図書館",
            "海岸",
            "旋律",
            "記憶",
            "季節",
            "手紙",
            "灯台",
            "回廊",
            "約束",
            "硝子",
            "残響",
            "標本",
            "封筒",
            "螺旋",
            "夜明",
            "輪郭",
            "潮騒",
            "書架",
            "遠雷",
        ];
        let verbs = [
            "見つめていた",
            "思い出していた",
            "書き留めた",
            "数えていた",
            "聞いていた",
            "受け止めた",
        ];
        let adjectives = ["静かな", "薄い", "淡い", "遠い", "冷たい", "眩しい"];
        for size in [1_000usize, 2_000, 4_000, 8_000, 16_000] {
            let mut text = String::new();
            let mut seed = 0x2545_f491_4f6c_dd1du64;
            let mut next = move || {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed as usize
            };
            while text.chars().count() < size {
                text.push_str(adjectives[next() % adjectives.len()]);
                text.push_str(nouns[next() % nouns.len()]);
                text.push('と');
                text.push_str(nouns[next() % nouns.len()]);
                text.push_str(&format!("{}番", next() % 100_000));
                text.push('を');
                text.push_str(verbs[next() % verbs.len()]);
                text.push_str("。\n");
            }
            let text = text.chars().take(size).collect::<String>();

            let started = Instant::now();
            let indexed = index_text(&text);
            let field_ms = started.elapsed().as_secs_f64() * 1000.0;
            let indexed_bytes =
                indexed.surface.len() + indexed.reading_kana.len() + indexed.reading_romaji.len();
            println!(
                "{size:>6} chars | index_text {field_ms:>7.1} ms, {indexed_bytes:>8} indexed bytes"
            );
        }
    }
}
