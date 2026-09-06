//! 続き物の題名を、本題・配布注記・階層を持つ話数に分ける。
//! 同じ解析結果を照合と読む順に使い、丸数字を正規化で失うことを防ぐ。

use std::sync::OnceLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, Default)]
pub struct SequenceTitle {
    pub display_stem: String,
    pub key: String,
    /// 題名が示す階層順。「第1章 後編②」は [1, 3, 2]。
    /// 無番号の初回は空のままにし、作品を比較する段階で判断する。
    pub order_path: Vec<i64>,
    pub ordinal_label: Option<String>,
    pub is_composite: bool,
}

fn number(value: &str) -> Option<i64> {
    if value.is_empty() {
        return None;
    }
    if let Ok(value) = value.parse() {
        return Some(value);
    }
    let mut total = 0;
    let mut current = 0;
    for ch in value.chars() {
        if let Some(index) = "〇一二三四五六七八九".chars().position(|v| v == ch) {
            current = current * 10 + index as i64;
        } else if ch == '零' {
            current *= 10;
        } else if matches!(ch, '十' | '百' | '千') {
            total += current.max(1)
                * match ch {
                    '十' => 10,
                    '百' => 100,
                    _ => 1_000,
                };
            current = 0;
        } else {
            return None;
        }
    }
    Some(total + current)
}

fn key(text: &str) -> String {
    super::search::normalize_search_text(text)
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect()
}

fn is_separator(ch: char) -> bool {
    ch.is_whitespace() || "・、。!?！？~〜～-–—…:：/／＆&_＿+＋".contains(ch)
}

fn clean_stem(text: &str) -> String {
    static EMPTY: OnceLock<Regex> = OnceLock::new();
    let empty = EMPTY.get_or_init(|| {
        Regex::new(r"(?:【[\s・+:/-]*】|〔[\s・+:/-]*〕|\([\s・+:/-]*\)|\[[\s・+:/-]*\])")
            .expect("empty sequence bracket")
    });
    let without_empty = empty.replace_all(text, "");
    let mut value = without_empty.trim_matches(is_separator).to_string();
    // 話数の直前で切った「本題【」だけを除く。本題の閉じた括弧は残す。
    while value.ends_with(['【', '〔', '(', '[']) {
        value.pop();
        value = value.trim_matches(is_separator).to_string();
    }
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

type Token = (usize, usize, i64, String);

fn without_tokens(text: &str, tokens: &[Token], end: usize) -> String {
    let mut out = String::with_capacity(end);
    let mut cursor = 0;
    for token in tokens.iter().filter(|token| token.1 <= end) {
        out.push_str(&text[cursor..token.0]);
        cursor = token.1;
    }
    out.push_str(&text[cursor..end]);
    out
}

fn embedded_in_word(text: &str, start: usize, end: usize, english: bool) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    if english {
        return before.is_some_and(|ch| ch.is_ascii_alphabetic())
            || after.is_some_and(|ch| ch.is_ascii_alphanumeric());
    }
    // 「一夜の物語」「第一章の秘密」は、話数でなく題名そのもの。
    after.is_some_and(|ch| "のをがでとにへもはっ目".contains(ch))
}

pub fn parse_sequence_title(title: &str) -> SequenceTitle {
    static META: OnceLock<Regex> = OnceLock::new();
    static ORDINAL: OnceLock<Regex> = OnceLock::new();
    static STAGE: OnceLock<Regex> = OnceLock::new();
    static BARE: OnceLock<Regex> = OnceLock::new();
    static BRACKET: OnceLock<Regex> = OnceLock::new();
    static ROMAN: OnceLock<Regex> = OnceLock::new();
    static RANGE: OnceLock<Regex> = OnceLock::new();
    static COMPOSITE: OnceLock<Regex> = OnceLock::new();
    let circled = title
        .chars()
        .map(|ch| {
            let value = match ch {
                '①'..='⑳' => Some(ch as u32 - 0x245f),
                '㉑'..='㉟' => Some(ch as u32 - 0x3250 + 20),
                '㊱'..='㊿' => Some(ch as u32 - 0x32b0 + 35),
                _ => None,
            };
            value.map_or_else(|| ch.to_string(), |value| format!("第{value}話"))
        })
        .collect::<String>();
    let width: String = circled.nfkc().collect();
    let metadata = META.get_or_init(|| {
        let token = r"(?:FANBOX|ファンボックス|ファンボ|pixiv|ピクシブ|支援サイト|サンプル|完全版|全文|先行公開|再掲|おまけ付き|体験版|支援者限定|フル版|R-?18|(?:本編|本文)?(?:約|全)?[0-9,.万千]+(?:文字|字))";
        Regex::new(&format!(r"(?i)[【〔(\[]\s*{token}(?:[\s/・,+]*{token})*\s*[】〕)\]]")).expect("sequence metadata")
    });
    let text = metadata.replace_all(&width, " ").to_string();
    let numbered = ORDINAL.get_or_init(|| Regex::new(r"(?i)(?:第\s*)?([0-9〇零一二三四五六七八九十百千]{1,4})\s*(?:話|章|回|編|篇|部|夜|巻|周目|節|幕)|(?:#\s*|その\s*|part\s*|ep(?:isode)?\.?\s*)([0-9〇零一二三四五六七八九十百千]{1,4})").expect("sequence ordinal"));
    let mut tokens = numbered
        .captures_iter(&text)
        .filter_map(|caps| {
            let all = caps.get(0)?;
            let english = all
                .as_str()
                .starts_with(|ch: char| ch.is_ascii_alphabetic());
            if embedded_in_word(&text, all.start(), all.end(), english) {
                return None;
            }
            let value = number(caps.get(1).or_else(|| caps.get(2))?.as_str())?;
            Some((all.start(), all.end(), value, all.as_str().to_string()))
        })
        .collect::<Vec<_>>();
    let stages = STAGE.get_or_init(|| Regex::new(r"前編|中編|後編|前篇|中篇|後篇|完結編|プロローグ|エピローグ|最終話|最終回|番外編|外伝|[【〔(\[]([上中下])[】〕)\]]").expect("sequence stage"));
    for caps in stages.captures_iter(&text) {
        let all = caps.get(0).expect("stage match");
        if embedded_in_word(&text, all.start(), all.end(), false) {
            continue;
        }
        let word = caps.get(1).unwrap_or(all).as_str();
        let value = match word {
            "前編" | "前篇" | "上" => 1,
            "中編" | "中篇" | "中" => 2,
            "後編" | "後篇" | "下" => 3,
            "完結編" => 4,
            "プロローグ" => 0,
            "最終話" | "最終回" => 98,
            "エピローグ" => 99,
            _ => 100,
        };
        tokens.push((all.start(), all.end(), value, word.to_string()));
    }
    {
        // 前置番号と末尾番号は、前後編があっても読む。
        let bare = BARE.get_or_init(|| Regex::new(r"^\s*[【〔(\[]?([0-9]{1,3})\s*[:.、_\-)】〕\]]|(?:\s+|[【〔(\[])([0-9]{1,3})[】〕)\]]?\s*$").expect("sequence bare ordinal"));
        for caps in bare.captures_iter(&text) {
            let all = caps.get(0).expect("bare match");
            if tokens
                .iter()
                .any(|token| all.start() < token.1 && token.0 < all.end())
                || (caps.get(1).is_some()
                    && text[all.end()..].starts_with(|ch: char| ch.is_ascii_digit()))
            {
                continue;
            }
            let num = caps.get(1).or_else(|| caps.get(2)).expect("bare number");
            if let Some(value) = number(num.as_str()) {
                tokens.push((all.start(), all.end(), value, format!("{value}")));
            }
        }
    }
    if tokens.is_empty() {
        let roman = ROMAN.get_or_init(|| {
            Regex::new(r"\s+(VIII|VII|VI|IV|III|II|IX|I|V|X)\s*$").expect("roman sequence ordinal")
        });
        if let Some(caps) = roman.captures(&text) {
            let all = caps.get(0).expect("roman match");
            let value = ["I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X"]
                .iter()
                .position(|word| *word == &caps[1])
                .expect("roman value")
                + 1;
            tokens.push((all.start(), all.end(), value as i64, caps[1].to_string()));
        }
    }
    let composite = COMPOSITE.get_or_init(|| {
        Regex::new(r"前(?:中)?後[編篇](?:合体|まとめ|収録)?").expect("composite sequence stage")
    });
    let mut explicit_composite = text.contains("合本") || text.contains("総集編");
    for found in composite.find_iter(&text) {
        explicit_composite = true;
        tokens.retain(|token| token.1 <= found.start() || token.0 >= found.end());
        tokens.push((found.start(), found.end(), 1, found.as_str().to_string()));
    }
    let range = RANGE.get_or_init(|| Regex::new(r"(?:第\s*)?([0-9〇零一二三四五六七八九十百千]{1,4})\s*(?:話|章|回|編|巻)?\s*[~〜–-]\s*(?:第\s*)?([0-9〇零一二三四五六七八九十百千]{1,4})\s*(?:話|章|回|編|巻)").expect("sequence range"));
    for caps in range.captures_iter(&text) {
        let all = caps.get(0).expect("range match");
        if let (Some(first), Some(last)) = (number(&caps[1]), number(&caps[2])) {
            explicit_composite |= first != last;
            tokens.retain(|token| token.1 <= all.start() || token.0 >= all.end());
            tokens.push((
                all.start(),
                all.end(),
                first.min(last),
                all.as_str().to_string(),
            ));
        }
    }
    tokens.sort_by_key(|token| token.0);
    let bracket = BRACKET.get_or_init(|| {
        Regex::new(r"[【〔(\[][^】〕)\]]*[】〕)\]]").expect("sequence identity bracket")
    });
    // 前置話数と本題を取り出した後に、末尾話数と副題を分ける。両端の話数の
    // あいだに本題があっても捨てない。括弧だけの前置人物注記は本題と一緒に残す。
    let boundary = tokens.iter().position(|token| {
        let before = without_tokens(&text, &tokens, token.0);
        !key(&bracket.replace_all(&before, "")).is_empty()
    });
    let stem_end = boundary.map_or(text.len(), |index| tokens[index].0);
    let display_stem = clean_stem(&without_tokens(&text, &tokens, stem_end));
    if let Some(index) = boundary {
        let mut end = index + 1;
        while end < tokens.len()
            && text[tokens[end - 1].1..tokens[end].0]
                .chars()
                .all(|ch| is_separator(ch) || "【】〔〕()[]「」『』".contains(ch))
        {
            end += 1;
        }
        tokens.truncate(end);
    }
    let mut is_composite = explicit_composite;
    let mut order_path = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        // 章と前後編を区切る「・」は合本の証拠ではない。同じ種の話数だけを見る。
        let level = |label: &str| {
            if label.contains("編") || label.contains("篇") || matches!(label, "上" | "中" | "下")
            {
                "stage"
            } else if label.ends_with("章") {
                "chapter"
            } else if label.ends_with("部") {
                "part"
            } else {
                "episode"
            }
        };
        let part_of_range = index > 0
            && level(&token.3) == level(&tokens[index - 1].3)
            && text[tokens[index - 1].1..token.0]
                .chars()
                .any(|ch| "+・,&~〜–-".contains(ch));
        is_composite |= part_of_range;
        if !part_of_range {
            order_path.push(token.2);
        }
    }
    SequenceTitle {
        key: key(&display_stem),
        display_stem,
        order_path,
        ordinal_label: (!tokens.is_empty()).then(|| {
            tokens
                .iter()
                .map(|t| t.3.as_str())
                .collect::<Vec<_>>()
                .join(" · ")
        }),
        is_composite,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotations_do_not_join_different_stories() {
        let a = parse_sequence_title("【ファンボサンプル】（後編）星の舟");
        let b = parse_sequence_title("【ファンボサンプル】（前編）花の庭");
        assert_eq!(a.display_stem, "星の舟");
        assert_ne!(a.key, b.key);
        for note in [
            "【FANBOXサンプル】",
            "【pixiv・全文】",
            "(本編約1万字)",
            "【先行公開/完全版】",
        ] {
            assert_eq!(
                parse_sequence_title(&format!("{note}（前編）星の舟")).key,
                parse_sequence_title("星の舟 後編").key
            );
        }
        assert_ne!(
            parse_sequence_title("【東の街】風の図書館 第1話").key,
            parse_sequence_title("【西の街】風の図書館 第2話").key
        );
    }

    #[test]
    fn identity_brackets_and_punctuation_remain_balanced() {
        for (title, expected) in [
            ("【東の街】風の図書館 第1話", "【東の街】風の図書館"),
            ("【東の街】（前編）風の図書館", "【東の街】風の図書館"),
            ("『星の舟』【第1話】出発", "『星の舟』"),
            ("星の舟（アオ編） 第1話", "星の舟(アオ編)"),
            ("【前編①】星の舟", "星の舟"),
            ("【東の街】風の図書館", "【東の街】風の図書館"),
        ] {
            assert_eq!(
                parse_sequence_title(title).display_stem,
                expected,
                "{title}"
            );
        }
    }

    #[test]
    fn hierarchy_and_number_forms_share_one_parser() {
        assert_eq!(
            parse_sequence_title("星の舟 第十一章 後編②").order_path,
            [11, 3, 2]
        );
        for (title, expected) in [
            ("①星の舟", 1),
            ("02：星の舟", 2),
            ("星の舟 第三話", 3),
            ("星の舟 4節", 4),
            ("㉑星の舟", 21),
            ("㊿星の舟", 50),
            ("星の舟 III", 3),
        ] {
            let parsed = parse_sequence_title(title);
            assert_eq!(parsed.order_path, [expected], "{title}");
            assert!(parsed.ordinal_label.is_some());
            assert_eq!(parsed.display_stem, "星の舟");
        }
        assert_eq!(parse_sequence_title("星の舟 第1章 前編").order_path, [1, 1]);
        assert_eq!(parse_sequence_title("星の舟 第1章 後編").order_path, [1, 3]);
        assert_eq!(
            parse_sequence_title("03：星の舟（前編）").order_path,
            [3, 1]
        );
        assert_eq!(
            parse_sequence_title("03：星の舟（前編）").display_stem,
            "星の舟"
        );
        assert!(
            parse_sequence_title("星の舟 第1章 後編").order_path
                < parse_sequence_title("星の舟 第2章 前編").order_path
        );
    }

    #[test]
    fn title_numbers_and_composites_are_not_ordinary_episodes() {
        for title in [
            "第七小隊の長い一日",
            "第1宇宙の旅",
            "猫が2匹いる暮らし",
            "旅の記録 2025",
            "一夜の物語",
            "第一章の秘密",
            "前編のない物語",
            "1.5倍の世界",
            "depart2ure",
        ] {
            let parsed = parse_sequence_title(title);
            assert!(parsed.order_path.is_empty(), "{title}: {parsed:?}");
            assert_eq!(parsed.display_stem, title);
        }
        for title in [
            "星の舟【前編＋後編】",
            "星の舟 第1〜3話",
            "【前後編合体】星の舟",
        ] {
            let parsed = parse_sequence_title(title);
            assert!(parsed.is_composite, "{title}");
            assert_eq!(parsed.order_path, [1]);
            assert_eq!(parsed.display_stem, "星の舟");
        }
        assert!(!parse_sequence_title("星の舟 第1章 後編").is_composite);
        assert!(!parse_sequence_title("星の舟 第1章・後編").is_composite);
    }

    #[test]
    fn subtitles_do_not_become_identity_or_extra_hierarchy() {
        let first = parse_sequence_title("星の舟 第1話 第七小隊との出会い");
        let second = parse_sequence_title("星の舟 第2話 もう一つの第3章");
        assert_eq!(first.key, second.key);
        assert_eq!(second.order_path, [2]);
        assert_eq!(parse_sequence_title("【後編②】星の舟").order_path, [3, 2]);
        assert_eq!(parse_sequence_title("星の舟").key, first.key);
        assert!(parse_sequence_title("星の舟").order_path.is_empty());
        assert_eq!(
            parse_sequence_title("星の舟 第1話 青い窓").order_path,
            parse_sequence_title("星の舟 第1話 赤い窓").order_path
        );
    }
}
