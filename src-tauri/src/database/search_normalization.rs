use std::sync::OnceLock;

use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use lindera::tokenizer::Tokenizer;
use unicode_normalization::UnicodeNormalization;
use wana_kana::{ConvertJapanese, IsJapaneseStr};

static TOKENIZER: OnceLock<Option<Tokenizer>> = OnceLock::new();

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
    analyze_text(input)
        .into_iter()
        .filter_map(|token| {
            if !token.reading_kana.is_empty() {
                Some(token.reading_kana)
            } else if !token.normalized.is_empty() {
                Some(token.normalized)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn reading_romaji_text(input: &str) -> String {
    analyze_text(input)
        .into_iter()
        .filter_map(|token| {
            if !token.reading_romaji.is_empty() {
                Some(token.reading_romaji)
            } else if token.normalized.is_ascii() && !token.normalized.is_empty() {
                Some(token.normalized)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn search_index_text(input: &str) -> String {
    let mut values = query_variants(input);
    let analyzed = analyze_text(input);
    for token in analyzed {
        push_unique(&mut values, token.normalized);
        push_unique(&mut values, token.base);
        push_unique(&mut values, token.reading_kana);
        push_unique(&mut values, token.reading_romaji);
    }
    values.join(" ")
}

pub fn query_variants(input: &str) -> Vec<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let mut values = Vec::new();
    push_unique(&mut values, normalize_for_search(trimmed));
    push_unique(&mut values, normalize_kana_reading(trimmed));
    push_unique(&mut values, reading_kana_text(trimmed));
    push_unique(&mut values, reading_romaji_text(trimmed));

    if trimmed.is_romaji() {
        let kana = trimmed.to_kana();
        push_unique(&mut values, normalize_for_search(&kana));
        push_unique(&mut values, normalize_kana_reading(&kana));
    }

    if trimmed.is_kana() {
        push_unique(&mut values, trimmed.to_romaji());
    }

    for synonym in synonym_variants(trimmed) {
        push_unique(&mut values, normalize_for_search(&synonym));
        push_unique(&mut values, reading_kana_text(&synonym));
        push_unique(&mut values, reading_romaji_text(&synonym));
    }

    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

pub fn synonym_variants(input: &str) -> Vec<String> {
    let normalized = normalize_for_search(input);
    let kana = reading_kana_text(input);
    let romaji = reading_romaji_text(input);
    let groups: &[&[&str]] = &[
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

    let mut out = Vec::new();
    for group in groups {
        let matched = group.iter().any(|candidate| {
            let candidate_norm = normalize_for_search(candidate);
            normalized == candidate_norm
                || kana == reading_kana_text(candidate)
                || romaji == reading_romaji_text(candidate)
        });
        if matched {
            for candidate in *group {
                if normalize_for_search(candidate) != normalized {
                    out.push((*candidate).to_string());
                }
            }
        }
    }
    out
}

pub fn expand_query_for_tantivy(query: &str) -> String {
    let mut values = Vec::new();
    for part in query.split_whitespace() {
        let cleaned = part.trim_matches('"').trim_start_matches('-');
        for variant in query_variants(cleaned) {
            push_unique(&mut values, variant);
        }
    }
    values.join(" ")
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

fn push_unique(values: &mut Vec<String>, value: String) {
    let value = value.trim().to_string();
    if value.is_empty() || values.iter().any(|item| item == &value) {
        return;
    }
    values.push(value);
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
}
