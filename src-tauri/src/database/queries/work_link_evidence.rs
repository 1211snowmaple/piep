//! 一つの URL に付いた言葉から関係を読む。隣のリンクの説明は混ぜない。

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use regex::Regex;

use super::{normalize_linked_work_url, normalize_search_text, truncate_chars, ExtractedWorkLink};

const MAX_LINKS: usize = 2_000;
const CONTEXT_CHARS: usize = 100;

fn url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)(?:https?://[^\s\"'<>\\\[\]{}]+|pixiv://novels/\d+)"#)
            .expect("work link URL regex")
    })
}

fn plain_text(input: &str) -> String {
    static BLOCK_RE: OnceLock<Regex> = OnceLock::new();
    static TAG_RE: OnceLock<Regex> = OnceLock::new();
    let with_breaks = BLOCK_RE
        .get_or_init(|| {
            Regex::new(r"(?is)</?(?:p|div|br|li|h[1-6])\b[^>]*>").expect("link block regex")
        })
        .replace_all(input, "\n");
    TAG_RE
        .get_or_init(|| Regex::new(r"(?is)<[^>]*>").expect("link HTML tag regex"))
        .replace_all(&with_breaks, " ")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
}

fn compact(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn has_any(text: &str, markers: &[&str]) -> bool {
    markers
        .iter()
        .any(|marker| text.contains(&normalize_search_text(marker)))
}

/// 明示された関係だけを返す。`None` は判断材料がないことを表す。
///
/// 保存済みの旧 context を再評価する側も使う。旧 context には隣のリンクの
/// 説明が入り得るため、方向が競合する場合は棄権し、呼び出し側でも話数と照合する。
pub(super) fn classify_link_context(text: &str) -> Option<(&'static str, f64)> {
    let normalized = normalize_search_text(&plain_text(text))
        .replace("前後編", "")
        .replace("前後篇", "");
    // 「続きは完全版へ」は次話ではなく、同じ作品の版への誘導である。
    if has_any(
        &normalized,
        &[
            "完全版",
            "フル版",
            "全文",
            "サンプル",
            "再掲",
            "改訂版",
            "加筆版",
            "別版",
            "続きはご支援者様向け",
            "続きは支援者限定",
        ],
    ) {
        return Some(("edition_of", 0.9));
    }
    if has_any(
        &normalized,
        &[
            "参考",
            "引用",
            "紹介",
            "元ネタ",
            "資料",
            "出典",
            "おすすめ",
            "推薦",
            "続きではない",
            "続きではありません",
            "続編ではない",
            "続編ではありません",
        ],
    ) {
        return Some(("mentions", 0.4));
    }
    // 「これの続きです」は、リンク先が前の作品であることを示す。
    if has_any(
        &normalized,
        &[
            "の続きです",
            "の続きですが",
            "の続編です",
            "前半はこちら",
            "前回投稿した作品",
            "前回のお話",
        ],
    ) {
        return Some(("continues_from", 0.94));
    }
    static NEXT_RE: OnceLock<Regex> = OnceLock::new();
    static PREV_RE: OnceLock<Regex> = OnceLock::new();
    let next = has_any(
        &normalized,
        &[
            "続き",
            "次話",
            "次編",
            "次の話",
            "後編",
            "後篇",
            "後半はこちら",
            "中編へ",
            "中篇へ",
        ],
    ) || NEXT_RE
        .get_or_init(|| Regex::new(r"\bnext\b").expect("next marker regex"))
        .is_match(&normalized);
    let previous = has_any(&normalized, &["前話", "前編", "前篇", "前作", "前の話"])
        || PREV_RE
            .get_or_init(|| Regex::new(r"\bprev(?:ious)?\b").expect("previous marker regex"))
            .is_match(&normalized);
    if next && previous {
        return Some(("mentions", 0.4));
    }
    if has_any(&normalized, &["補足", "番外", "おまけ", "関連"]) {
        return Some(("supplement", 0.86));
    }
    if next {
        Some(("continues_to", 0.94))
    } else if previous {
        Some(("continues_from", 0.94))
    } else {
        None
    }
}

#[derive(Debug)]
struct Anchor {
    start: usize,
    end: usize,
    url_start: usize,
    url_end: usize,
    label: String,
}

fn anchors(text: &str) -> Vec<Anchor> {
    static HTML_RE: OnceLock<Regex> = OnceLock::new();
    static MARKDOWN_RE: OnceLock<Regex> = OnceLock::new();
    let mut anchors = Vec::new();
    let html = HTML_RE.get_or_init(|| {
        Regex::new(
            r#"(?is)<a\b[^>]*\bhref\s*=\s*(?:"(?P<double>[^"]*)"|'(?P<single>[^']*)')[^>]*>(?P<label>.*?)</a\s*>"#,
        )
        .expect("HTML link anchor regex")
    });
    for capture in html.captures_iter(text).take(MAX_LINKS) {
        let whole = capture.get(0).expect("HTML anchor");
        let url = capture
            .name("double")
            .or_else(|| capture.name("single"))
            .expect("href");
        let label = capture.name("label").expect("anchor label");
        anchors.push(Anchor {
            start: whole.start(),
            end: whole.end(),
            url_start: url.start(),
            url_end: url.end(),
            label: compact(&plain_text(label.as_str())),
        });
    }
    let markdown = MARKDOWN_RE.get_or_init(|| {
        Regex::new(r"\[(?P<label>[^\]\n]*)\]\((?P<url>https?://[^\s)]+)\)")
            .expect("Markdown link anchor regex")
    });
    for capture in markdown.captures_iter(text).take(MAX_LINKS) {
        let whole = capture.get(0).expect("Markdown anchor");
        let url = capture.name("url").expect("Markdown URL");
        let label = capture.name("label").expect("Markdown label");
        anchors.push(Anchor {
            start: whole.start(),
            end: whole.end(),
            url_start: url.start(),
            url_end: url.end(),
            label: compact(&plain_text(label.as_str())),
        });
    }
    anchors.sort_by_key(|anchor| anchor.start);
    anchors
}

fn separator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '。' | ';' | '；' | '|' | '｜')
}

fn prefix_context(input: &str) -> String {
    let plain = plain_text(input);
    let fragment = plain.rsplit(separator).next().unwrap_or("");
    let chars = fragment.chars().collect::<Vec<_>>();
    compact(
        &chars[chars.len().saturating_sub(CONTEXT_CHARS)..]
            .iter()
            .collect::<String>(),
    )
}

fn suffix_context(input: &str, has_next_link: bool) -> String {
    let plain = plain_text(input);
    // 同じ文の次の URL の前にある説明は、その次の URL のものかもしれない。
    // 区切りもアンカーもなければ、後ろ側の語から方向を借りない。
    if has_next_link && !plain.chars().any(separator) {
        return String::new();
    }
    compact(&truncate_chars(
        plain.split(separator).next().unwrap_or(""),
        CONTEXT_CHARS,
    ))
}

pub(super) fn extract(
    text: &str,
    from_source: &str,
    from_source_id: &str,
) -> Vec<ExtractedWorkLink> {
    let anchors = anchors(text);
    let candidates = url_re().find_iter(text).take(MAX_LINKS).collect::<Vec<_>>();
    let containing_anchors = candidates
        .iter()
        .map(|candidate| {
            anchors
                .iter()
                .find(|anchor| anchor.start <= candidate.start() && candidate.start() < anchor.end)
        })
        .collect::<Vec<_>>();
    let mut links: HashMap<(String, String), ExtractedWorkLink> = HashMap::new();
    let mut contradictory = HashSet::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let anchor = containing_anchors[index];
        if anchor.is_some_and(|anchor| {
            candidate.start() < anchor.url_start || candidate.start() >= anchor.url_end
        }) {
            continue; // 自動リンクの表示文字にある同じ URL を二度読まない。
        }
        let raw = candidate
            .as_str()
            .replace("&amp;", "&")
            .replace("\\u0026", "&");
        let raw = raw.trim_end_matches([
            '.', ',', ':', ';', '!', '?', ')', ']', '}', '。', '、', '！', '？', '）', '】', '」',
            '』',
        ]);
        let Some((to_source, to_source_id)) = normalize_linked_work_url(raw) else {
            continue;
        };
        if to_source == from_source && to_source_id == from_source_id {
            continue;
        }
        let start = anchor.map_or(candidate.start(), |anchor| anchor.start);
        let end = anchor.map_or(candidate.end(), |anchor| anchor.end);
        let previous_end = index
            .checked_sub(1)
            .map_or(0, |previous| {
                containing_anchors[previous].map_or(candidates[previous].end(), |anchor| anchor.end)
            })
            .min(start);
        let next_start = candidates
            .get(index + 1)
            .map_or(text.len(), |next| {
                containing_anchors[index + 1].map_or(next.start(), |anchor| anchor.start)
            })
            .max(end);
        let prefix = prefix_context(&text[previous_end..start]);
        let suffix = suffix_context(&text[end..next_start], index + 1 < candidates.len());
        let anchor_label = anchor
            .map(|anchor| anchor.label.as_str())
            .filter(|label| !label.is_empty());
        let anchor_relation = anchor_label.and_then(classify_link_context);
        let prefix_relation = classify_link_context(&prefix);
        let surrounding = compact(&format!("{prefix} {suffix}"));
        let surrounding_relation = classify_link_context(&surrounding);
        // アンカーの明示を優先し、参考・版の誘導は本文周囲からも確認する。
        let (relation, context) = if let Some(relation) =
            prefix_relation.filter(|(kind, _)| matches!(*kind, "edition_of" | "mentions"))
        {
            (relation, prefix)
        } else if let Some(relation) = anchor_relation {
            (relation, anchor_label.unwrap_or("").to_string())
        } else {
            (
                surrounding_relation.unwrap_or(("mentions", 0.72)),
                surrounding,
            )
        };
        let value = ExtractedWorkLink {
            to_source: to_source.clone(),
            to_source_id: to_source_id.clone(),
            relation_type: relation.0.to_string(),
            confidence: relation.1,
            anchor_text: anchor_label.map(|label| truncate_chars(label, 120)),
            context_text: (!context.is_empty()).then(|| truncate_chars(&context, 240)),
        };
        let key = (to_source, to_source_id);
        if let Some(current) = links.get_mut(&key) {
            let direction_conflict = matches!(
                (current.relation_type.as_str(), value.relation_type.as_str()),
                ("continues_from", "continues_to") | ("continues_to", "continues_from")
            );
            if direction_conflict || contradictory.contains(&key) {
                current.relation_type = "mentions".to_string();
                current.confidence = 0.4;
                contradictory.insert(key);
            } else if value.confidence > current.confidence {
                *current = value;
            }
        } else {
            links.insert(key, value);
        }
    }
    let mut links = links.into_values().collect::<Vec<_>>();
    links.sort_by(|left, right| {
        right
            .confidence
            .total_cmp(&left.confidence)
            .then_with(|| left.to_source.cmp(&right.to_source))
            .then_with(|| left.to_source_id.cmp(&right.to_source_id))
    });
    links
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relations(text: &str) -> HashMap<String, String> {
        extract(text, "pixiv", "999")
            .into_iter()
            .map(|link| (link.to_source_id, link.relation_type))
            .collect()
    }

    #[test]
    fn adjacent_plain_urls_keep_their_own_direction() {
        for separator in ["\n", " ", " | "] {
            let text = format!("前話 https://www.pixiv.net/novel/show.php?id=1{separator}次話 https://writer.fanbox.cc/posts/2");
            let links = relations(&text);
            assert_eq!(links["1"], "continues_from", "{text}");
            assert_eq!(links["2"], "continues_to", "{text}");
        }
    }

    #[test]
    fn html_anchors_keep_labels_and_ignore_adjacent_link_text() {
        let text = r#"<p><a href="https://www.pixiv.net/novel/show.php?id=1"><b>前編</b></a> <a href='https://writer.fanbox.cc/posts/2'>後編</a></p>"#;
        let links = extract(text, "pixiv", "999");
        let previous = links.iter().find(|link| link.to_source_id == "1").unwrap();
        let next = links.iter().find(|link| link.to_source_id == "2").unwrap();
        assert_eq!(previous.relation_type, "continues_from");
        assert_eq!(previous.anchor_text.as_deref(), Some("前編"));
        assert_eq!(next.relation_type, "continues_to");
        assert_eq!(next.anchor_text.as_deref(), Some("後編"));
    }

    #[test]
    fn markdown_links_are_classified_by_their_own_anchor() {
        let links = relations("[前編](https://www.pixiv.net/novel/show.php?id=1) [後編](https://writer.fanbox.cc/posts/2)");
        assert_eq!(links["1"], "continues_from");
        assert_eq!(links["2"], "continues_to");
    }

    #[test]
    fn conflicting_directions_abstain() {
        let links = extract(
            "前編と後編はこちら https://www.pixiv.net/novel/show.php?id=1",
            "pixiv",
            "999",
        );
        assert_eq!(links[0].relation_type, "mentions");
        assert!(links[0].confidence < 0.6);
    }

    #[test]
    fn reference_and_full_edition_are_not_continuations() {
        for (label, expected) in [
            ("前作の参考資料", "mentions"),
            ("続きは完全版", "edition_of"),
            ("後編のサンプル", "edition_of"),
        ] {
            let links = relations(&format!(
                "{label} https://www.pixiv.net/novel/show.php?id=1"
            ));
            assert_eq!(links["1"], expected);
        }
        let links =
            relations(r#"参考: <a href="https://www.pixiv.net/novel/show.php?id=1">前編</a>"#);
        assert_eq!(links["1"], "mentions");
    }

    #[test]
    fn unmarked_reference_does_not_borrow_next_links_label() {
        let links = relations(
            "https://www.pixiv.net/novel/show.php?id=1\n次話 https://writer.fanbox.cc/posts/2",
        );
        assert_eq!(links["1"], "mentions");
        assert_eq!(links["2"], "continues_to");
    }

    #[test]
    fn repeated_opposite_evidence_does_not_choose_first_or_last() {
        let links = extract("前話 https://www.pixiv.net/novel/show.php?id=1\n次話 https://www.pixiv.net/novel/show.php?id=1\n前話 https://www.pixiv.net/novel/show.php?id=1", "pixiv", "999");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].relation_type, "mentions");
        assert!(links[0].confidence < 0.6);
    }

    #[test]
    fn url_then_label_and_english_word_boundaries_work() {
        assert_eq!(
            relations("https://www.pixiv.net/novel/show.php?id=1 前話")["1"],
            "continues_from"
        );
        assert_eq!(classify_link_context("preview of nextdoor"), None);
        assert_eq!(
            classify_link_context("PREVIOUS"),
            Some(("continues_from", 0.94))
        );
    }

    #[test]
    fn a_current_story_described_as_the_continuation_points_back() {
        for label in [
            "これの続きですが単独でも読めます",
            "前後編の後半です。前半はこちら",
            "前回投稿した作品の続きです",
        ] {
            assert_eq!(
                classify_link_context(label),
                Some(("continues_from", 0.94)),
                "{label}"
            );
        }
        assert_eq!(classify_link_context("前後編をまとめました"), None);
        assert_eq!(
            classify_link_context("続きはご支援者様向けに公開しております"),
            Some(("edition_of", 0.9))
        );
    }

    #[test]
    fn self_links_are_omitted_and_html_entities_are_decoded() {
        let links = extract(
            r#"<a href="https://www.pixiv.net/novel/show.php?id=999">前編</a><a href="https://www.pixiv.net/novel/show.php?id=1&amp;mode=all">続き</a>"#,
            "pixiv",
            "999",
        );
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].to_source_id, "1");
        assert_eq!(links[0].relation_type, "continues_to");
    }
}
