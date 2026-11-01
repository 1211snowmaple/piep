//! 編集中の本文から、誤字・脱字の候補を挙げてもらう。
//!
//! **直すのは利用者で、ここは候補を出すだけ。** モデルの答えをそのまま
//! 当てると、口調や意図した崩しまで「正しい日本語」へ均されてしまう。
//! 候補は一件ずつ、本文のどこをどう変えるかが分かる形で返し、編集画面で
//! 採るかどうかを選ばせる。
//!
//! モデルが写し間違えた箇所、言い換えに近い大きな書き換え、同じ文字列が
//! 段落に何度も出てどこを指すのか決まらないもの ―― は、ここで落とす。
//! 当てる場所が一つに決まらない案は、出すだけ利用者の手間になる。

use serde::{Deserialize, Serialize};

use super::{
    ask_summary_json, configured_system_prompt, extract_json, object_schema, require_body_consent,
    AssistEngine, FEATURE_EDITOR_PROOFREAD,
};

/// 一回に読ませる本文の上限（字）。手元のモデルで一往復が数十秒に収まる量。
pub const MAX_PROOFREAD_CHARS: usize = 6_000;
/// 一回に渡す段落の上限。
pub const MAX_PROOFREAD_SEGMENTS: usize = 60;
/// 一回で返す候補の上限。多すぎる答えは、細かな好みまで拾い始めている。
const MAX_SUGGESTIONS: usize = 40;
/// 書き換える部分の長さの上限（字）。これより長い差は校正ではなく書き直し。
const MAX_CHANGED_CHARS: usize = 12;
/// `before` の長さの上限（字）。場所を一つに決めるための前後を含めた長さ。
const MAX_SNIPPET_CHARS: usize = 80;

/// 校正してほしい本文のひとまとまり。`id` は編集画面のブロックの身元。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofreadSegment {
    pub id: String,
    pub text: String,
}

/// 直す候補。`before` は段落の本文にそのまま一度だけ現れる。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProofreadSuggestion {
    pub segment_id: String,
    pub before: String,
    pub after: String,
    pub kind: String,
    pub reason: String,
}

#[derive(Debug, Deserialize)]
struct RawSuggestions {
    suggestions: Vec<RawSuggestion>,
}

#[derive(Debug, Deserialize)]
struct RawSuggestion {
    segment: usize,
    before: String,
    after: String,
    kind: String,
    reason: String,
}

const KINDS: &[&str] = &["誤字", "脱字", "衍字", "変換ミス", "表記ゆれ", "記号"];

const PROOFREAD_SYSTEM: &str = "あなたは日本語の小説の校正者です。番号付きの段落を読み、明らかな誤りだけを指摘します。\n\
指摘してよいもの: 誤字、脱字、同じ字の重複（衍字）、漢字の変換ミス、同じ作品内での表記ゆれ、括弧の閉じ忘れなど記号の誤り。\n\
指摘しないもの: 文体、言い回し、口調、方言、くだけた書き方や意図した崩し（「〜っす」「ぁ」「ー」の連続など）、句読点の好み、擬音、伏せ字、ルビや [[rb:…]] などの記法、固有名詞の表記。迷うものは出さないでください。\n\
before には、その段落の本文から直したい箇所を前後数文字ごと一字一句そのまま写します（10〜30字ほど。段落の中で一か所に決まる長さ）。after は before を直したもので、直す字以外は before と同じにします。\n\
reason は何をどう直したかを20字以内で書きます。誤りが無ければ suggestions は空の配列にします。";

/// 候補を挙げてもらう。**本文を送るので、許可が要る。**
pub async fn proofread(
    engine: &AssistEngine,
    segments: &[ProofreadSegment],
) -> Result<Vec<ProofreadSuggestion>, String> {
    require_body_consent(engine)?;
    check_request(segments)?;
    let segments: Vec<&ProofreadSegment> = segments
        .iter()
        .filter(|segment| !segment.text.trim().is_empty())
        .collect();
    if segments.is_empty() {
        return Ok(Vec::new());
    }
    let system = configured_system_prompt(engine, FEATURE_EDITOR_PROOFREAD, PROOFREAD_SYSTEM)?;
    let user = segments
        .iter()
        .enumerate()
        .map(|(index, segment)| format!("<段落 番号=\"{}\">\n{}\n</段落>", index + 1, segment.text))
        .collect::<Vec<_>>()
        .join("\n");
    let schema = object_schema(
        "proofread_suggestions",
        serde_json::json!({
            "suggestions": {
                "type": "array",
                "maxItems": MAX_SUGGESTIONS,
                "items": {
                    "type": "object",
                    "properties": {
                        "segment": { "type": "integer", "minimum": 1, "maximum": segments.len() },
                        "before": { "type": "string" },
                        "after": { "type": "string" },
                        "kind": { "type": "string", "enum": KINDS },
                        "reason": { "type": "string" }
                    },
                    "required": ["segment", "before", "after", "kind", "reason"],
                    "additionalProperties": false
                }
            }
        }),
        vec!["suggestions"],
    );
    let content = ask_summary_json(engine, &system, &user, schema, 2_048).await?;
    let raw: RawSuggestions = serde_json::from_str(extract_json(&content)?)
        .map_err(|e| format!("校正の答えを読めません: {e}"))?;
    Ok(checked_suggestions(&segments, raw.suggestions))
}

fn check_request(segments: &[ProofreadSegment]) -> Result<(), String> {
    if segments.len() > MAX_PROOFREAD_SEGMENTS {
        return Err(format!(
            "一度に校正できるのは{MAX_PROOFREAD_SEGMENTS}段落までです"
        ));
    }
    let chars: usize = segments
        .iter()
        .map(|segment| segment.text.chars().count())
        .sum();
    if chars > MAX_PROOFREAD_CHARS {
        return Err(format!(
            "一度に校正できるのは{MAX_PROOFREAD_CHARS}字までです（{chars}字）"
        ));
    }
    Ok(())
}

/// 当てられる候補だけを残す。
fn checked_suggestions(
    segments: &[&ProofreadSegment],
    raw: Vec<RawSuggestion>,
) -> Vec<ProofreadSuggestion> {
    let mut out: Vec<ProofreadSuggestion> = Vec::new();
    for item in raw.into_iter().take(MAX_SUGGESTIONS) {
        let Some(segment) = item
            .segment
            .checked_sub(1)
            .and_then(|index| segments.get(index))
        else {
            continue;
        };
        let before = item.before;
        let after = item.after;
        if before.is_empty() || before == after || before.chars().count() > MAX_SNIPPET_CHARS {
            continue;
        }
        // 写し間違えた箇所と、どこを指すのか決まらない箇所は当てられない。
        if segment.text.matches(before.as_str()).count() != 1 {
            continue;
        }
        let (removed, added) = changed_span(&before, &after);
        if removed > MAX_CHANGED_CHARS || added > MAX_CHANGED_CHARS {
            continue;
        }
        let kind = if KINDS.contains(&item.kind.as_str()) {
            item.kind
        } else {
            "誤字".to_string()
        };
        let suggestion = ProofreadSuggestion {
            segment_id: segment.id.clone(),
            before,
            after,
            kind,
            reason: item.reason.trim().chars().take(60).collect(),
        };
        // 同じ箇所への二つ目の案は捨てる。どちらを当てても、もう一方は
        // 当てられなくなる。
        if out.iter().any(|seen| {
            seen.segment_id == suggestion.segment_id && overlaps(seen, &suggestion, segment)
        }) {
            continue;
        }
        out.push(suggestion);
    }
    out
}

/// 前後の一致を除いた、書き換わる部分の長さ（消える字数, 入る字数）。
fn changed_span(before: &str, after: &str) -> (usize, usize) {
    let left: Vec<char> = before.chars().collect();
    let right: Vec<char> = after.chars().collect();
    let prefix = left.iter().zip(&right).take_while(|(a, b)| a == b).count();
    let suffix = left[prefix..]
        .iter()
        .rev()
        .zip(right[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    (left.len() - prefix - suffix, right.len() - prefix - suffix)
}

fn overlaps(a: &ProofreadSuggestion, b: &ProofreadSuggestion, segment: &ProofreadSegment) -> bool {
    let (Some(a_start), Some(b_start)) =
        (segment.text.find(&a.before), segment.text.find(&b.before))
    else {
        return false;
    };
    let a_end = a_start + a.before.len();
    let b_end = b_start + b.before.len();
    a_start < b_end && b_start < a_end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(id: &str, text: &str) -> ProofreadSegment {
        ProofreadSegment {
            id: id.to_string(),
            text: text.to_string(),
        }
    }

    fn raw(segment: usize, before: &str, after: &str) -> RawSuggestion {
        RawSuggestion {
            segment,
            before: before.to_string(),
            after: after.to_string(),
            kind: "誤字".to_string(),
            reason: "直す".to_string(),
        }
    }

    #[test]
    fn keeps_a_small_fix_that_points_at_one_place() {
        let a = segment("a", "彼は静かに扉を開けた。部屋は暗らかった。");
        let found = checked_suggestions(&[&a], vec![raw(1, "部屋は暗らかった", "部屋は暗かった")]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].segment_id, "a");
    }

    #[test]
    fn drops_text_the_model_did_not_copy_exactly() {
        let a = segment("a", "部屋は暗らかった。");
        let found = checked_suggestions(&[&a], vec![raw(1, "部屋が暗らかった", "部屋が暗かった")]);
        assert!(found.is_empty(), "本文に無い文字列は当てられない");
    }

    #[test]
    fn drops_a_place_that_appears_more_than_once() {
        let a = segment("a", "あいう。あいう。");
        let found = checked_suggestions(&[&a], vec![raw(1, "あいう", "あいえ")]);
        assert!(found.is_empty(), "どちらを直すのか決まらない");
    }

    #[test]
    fn drops_a_rewrite_that_is_not_a_correction() {
        let a = segment("a", "彼女はとても嬉しそうに笑っていた。");
        let found = checked_suggestions(
            &[&a],
            vec![raw(
                1,
                "彼女はとても嬉しそうに笑っていた",
                "彼女は満面の笑みを浮かべて喜んでいた",
            )],
        );
        assert!(found.is_empty(), "言い換えは校正ではない");
    }

    #[test]
    fn drops_numbers_outside_the_request_and_overlapping_fixes() {
        let a = segment("a", "部屋は暗らかった。");
        let found = checked_suggestions(
            &[&a],
            vec![
                raw(2, "部屋は暗らかった", "部屋は暗かった"),
                raw(0, "部屋は暗らかった", "部屋は暗かった"),
                raw(1, "部屋は暗らかった", "部屋は暗かった"),
                raw(1, "暗らかった。", "暗かった。"),
            ],
        );
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[test]
    fn refuses_more_than_one_request_can_carry() {
        let long = segment("a", &"あ".repeat(MAX_PROOFREAD_CHARS + 1));
        assert!(check_request(&[long]).is_err());
    }

    #[test]
    fn measures_only_the_changed_middle() {
        assert_eq!(changed_span("部屋は暗らかった", "部屋は暗かった"), (1, 0));
        assert_eq!(changed_span("abc", "abc"), (0, 0));
        assert_eq!(changed_span("あいう", "かきく"), (3, 3));
    }
}
