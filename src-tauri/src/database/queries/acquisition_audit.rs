use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::LazyLock;

static REFERENCES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(pixivimage|uploadedimage):([^\]]+)\]").unwrap());
static PIXIV_SPEC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+)(?:-(\d+))?$").unwrap());

/// Fields the App API blanked when it gated an R-18 novel. The age-gate cover
/// is the evidence: an empty caption on its own is an ordinary author choice.
/// Once the web detail has been consulted, a blank field is what the source
/// really holds and is no longer reported.
pub(crate) fn masked_pixiv_metadata(value: &Value) -> Vec<&'static str> {
    crate::library::masked_pixiv_metadata(value)
}

pub(crate) fn masked_metadata_label(fields: &[&str]) -> String {
    fields
        .iter()
        .map(|field| match *field {
            "title" => "題名",
            "caption" => "説明",
            _ => "タグ",
        })
        .collect::<Vec<_>>()
        .join("・")
}

fn missing_saved_references(text: &str, filenames: &HashSet<String>) -> Vec<String> {
    let mut missing = Vec::new();
    for capture in REFERENCES.captures_iter(text) {
        let spec = &capture[2];
        let prefixes = if &capture[1] == "uploadedimage" {
            vec![format!("inline_{spec}.")]
        } else if let Some(parts) = PIXIV_SPEC.captures(spec) {
            let page = parts
                .get(2)
                .map_or(Some(1), |value| value.as_str().parse::<u64>().ok());
            match page.and_then(|page| page.checked_sub(1)) {
                Some(index) => vec![
                    format!("{}_p{index}.", &parts[1]),
                    format!("{spec}_p{index}."),
                ],
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let available = filenames.iter().any(|filename| {
            let filename = filename.strip_prefix("illust_").unwrap_or(filename);
            prefixes.iter().any(|prefix| filename.starts_with(prefix))
        });
        let reference = capture.get(0).unwrap().as_str();
        if !available && !missing.iter().any(|existing| existing == reference) {
            missing.push(reference.to_string());
        }
    }
    missing
}

pub(crate) fn pixiv_missing_references_for_filenames(
    value: &Value,
    filenames: impl IntoIterator<Item = String>,
) -> Vec<String> {
    let text = value
        .get("text")
        .or_else(|| value.pointer("/detail/text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    missing_saved_references(text, &filenames.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_require_the_exact_page_and_real_filename() {
        let names = HashSet::from([
            "123_p1.jpg".into(),
            "illust_456-3_p2.png".into(),
            "inline_789.jpg".into(),
        ]);
        assert_eq!(missing_saved_references(
            "[pixivimage:123-2][pixivimage:456-3][uploadedimage:789][pixivimage:123][pixivimage:123-0][pixivimage:id=123][pixivimage:123]", &names),
            vec!["[pixivimage:123]", "[pixivimage:123-0]", "[pixivimage:id=123]"]);
    }

    #[test]
    fn masked_details_need_the_age_gate_and_an_unconsulted_web_detail() {
        let gate = "https://s.pximg.net/common/images/limit_r18g_100.png";
        let masked = serde_json::json!({"detail":{"title":"","caption":" ","cover_url":gate}});
        assert_eq!(
            masked_pixiv_metadata(&masked),
            vec!["title", "caption", "tags"]
        );
        assert_eq!(
            masked_metadata_label(&masked_pixiv_metadata(&masked)),
            "題名・説明・タグ"
        );
        let filled = serde_json::json!({"cover_url":gate,"detail":{"title":"題","caption":"説明","tags":[{"name":"R-18"}]}});
        assert!(masked_pixiv_metadata(&filled).is_empty());
        let ungated = serde_json::json!({"cover_url":"https://i.pximg.net/a.jpg","detail":{"title":"題","caption":"","tags":[]}});
        assert!(masked_pixiv_metadata(&ungated).is_empty());
        let consulted = serde_json::json!({"cover_url":gate,"web_detail":{},"detail":{"title":"題","caption":"","tags":[]}});
        assert!(masked_pixiv_metadata(&consulted).is_empty());
    }
}
