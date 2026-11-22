//! 検索欄の候補の組み立て（旧棚と棚で共通）。
//!
//! 候補は前方一致の速い道（それぞれの棚の SQL）で集める。それで足りないときだけ、よく
//! 使う名前の束（棚の世代ごとに覚える）から、正規化した部分一致と作者の読みで足す。前方
//! 一致の候補は必ず先に並べ、足した候補がそれを押し出さない。

use crate::database::models::SearchSuggestion;
use crate::database::search::normalize_search_text;
use crate::database::search_normalization::index_text;
use std::ops::Deref;

/// 正規化で足す候補の束に、種類ごとに入れる数（よく使う順）。
pub const POOL_PER_KIND: i64 = 400;

/// 束の一件。比べるための形を先に作っておく。
#[derive(Clone)]
pub struct Candidate {
    pub item: SearchSuggestion,
    normalized_label: String,
    compact_label: String,
    reading_kana: String,
    reading_romaji: String,
}

fn compact(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

impl Candidate {
    pub fn new(item: SearchSuggestion) -> Self {
        let normalized_label = normalize_search_text(&item.label);
        let compact_label = compact(&normalized_label);
        let indexed = if item.kind == "author" {
            index_text(&item.label)
        } else {
            Default::default()
        };
        Self {
            item,
            normalized_label,
            compact_label,
            reading_kana: compact(&indexed.reading_kana),
            reading_romaji: compact(&indexed.reading_romaji),
        }
    }

    /// 覚えておくときの大きさの目安。
    pub fn bytes(&self) -> usize {
        self.item.label.len()
            + self.normalized_label.len()
            + self.reading_kana.len()
            + self.reading_romaji.len()
            + 128
    }
}

/// 前方一致の LIKE の型。`%` と `_` は文字のまま（`ESCAPE '\'` で使う）。
pub fn like_prefix(text: &str) -> String {
    format!("{}%", text.replace('%', "\\%").replace('_', "\\_"))
}

fn kind_priority(kind: &str) -> u8 {
    match kind {
        "author" => 0,
        "series" => 1,
        "title" => 2,
        "tag" => 3,
        _ => 4,
    }
}

/// 一致したもの・作者・シリーズ・題・タグの順、同じ種類は多い順・名前の順。
pub fn sort(items: &mut [SearchSuggestion]) {
    items.sort_by(|left, right| {
        right
            .exact_match
            .cmp(&left.exact_match)
            .then_with(|| kind_priority(&left.kind).cmp(&kind_priority(&right.kind)))
            .then_with(|| right.count.unwrap_or(0).cmp(&left.count.unwrap_or(0)))
            .then_with(|| left.label.cmp(&right.label))
    });
}

fn same(left: &SearchSuggestion, right: &SearchSuggestion) -> bool {
    left.kind == right.kind
        && left.source == right.source
        && left.source_key == right.source_key
        && left.value == right.value
}

/// 束から、前方一致で埋まらなかった種類の候補を足す。
pub fn fallbacks(
    candidates: &[Candidate],
    text: &str,
    limit: i64,
    existing: &[SearchSuggestion],
    out: &mut Vec<SearchSuggestion>,
) {
    let query = normalize_search_text(text);
    let compact_query = compact(&query);
    if compact_query.chars().count() < 2 {
        return;
    }
    let allow_compact = !query.chars().any(char::is_whitespace);
    for candidate in candidates {
        if existing
            .iter()
            .filter(|item| item.kind == candidate.item.kind)
            .count() as i64
            >= limit
        {
            continue;
        }
        let surface_match = candidate.normalized_label.contains(&query)
            || (allow_compact && candidate.compact_label.contains(&compact_query));
        let reading_match = candidate.item.kind == "author"
            && !surface_match
            && (candidate.reading_kana.contains(&query)
                || candidate.reading_romaji.contains(&query)
                || (allow_compact
                    && (candidate.reading_kana.contains(&compact_query)
                        || candidate.reading_romaji.contains(&compact_query))));
        if !surface_match && !reading_match {
            continue;
        }
        if existing
            .iter()
            .chain(out.iter())
            .any(|item| same(item, &candidate.item))
        {
            continue;
        }
        out.push(candidate.item.clone());
    }
}

/// 前方一致で集めた候補（種類ごとに `limit` 件まで）に、足りなければ束から足して、
/// 並べて `limit` 件にする。束は要るときだけ `pool` で引く。
pub fn assemble<P, E>(
    text: &str,
    limit: i64,
    mut items: Vec<SearchSuggestion>,
    pool: impl FnOnce() -> Result<P, E>,
) -> Result<Vec<SearchSuggestion>, E>
where
    P: Deref<Target = [Candidate]>,
{
    let normalized_text = normalize_search_text(text);
    for item in &mut items {
        item.exact_match = normalize_search_text(&item.label) == normalized_text;
    }
    sort(&mut items);
    if items.len() < limit as usize && normalized_text.chars().count() >= 2 {
        let candidates = pool()?;
        let mut additions = Vec::new();
        fallbacks(&candidates, text, limit, &items, &mut additions);
        for item in &mut additions {
            item.exact_match = normalize_search_text(&item.label) == normalized_text;
        }
        sort(&mut additions);
        items.extend(additions);
    }
    items.dedup_by(|left, right| same(left, right));
    items.truncate(limit as usize);
    Ok(items)
}
