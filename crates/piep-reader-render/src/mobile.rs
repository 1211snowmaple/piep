//! 携帯へ渡すページの形にする。画像の置き場（`data-local-path`）を画像の番号へ置き換える。

use regex::Regex;
use std::sync::LazyLock;

static HTML_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<!--.*?-->|<[a-z][a-z0-9:-]*\b(?:[^"'<>]|"[^"]*"|'[^']*')*>"#)
        .expect("valid tag regex")
});

static HTML_ATTRIBUTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)(\s+)([^\s/="'<>`]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?"#)
        .expect("valid attribute regex")
});

static ATTACHMENT_ORIGINAL_BUTTON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)\s*<button\b[^>]*\battachment-original\b[^>]*>.*?</button>"#)
        .expect("valid button regex")
});

/// 携帯の画面が読む画像の指定。
pub struct MobileAsset {
    pub id: i64,
    /// 縦横が分かれば、画像が届く前から枠の高さが決まる。本文を先に出しても位置がずれない。
    pub dimensions: Option<(u32, u32)>,
}

/// 本文の HTML の置き場を、`resolve` が返す画像の番号に置き換える。
///
/// 属性を値ごとに読む。本文の単語や、alt/title の値に書かれた `data-local-path` は画像の参照
/// として扱わない。`resolve` は属性の値（HTML の属性としてエスケープされたまま）を受け取る。
/// 解決できない参照があれば誤り（知らない置き場を携帯へ渡さない）。添付の「原本を見る」は
/// 携帯では開けないので外す。
pub fn mobile_html(
    html: &str,
    mut resolve: impl FnMut(&str) -> Option<MobileAsset>,
) -> Result<String, String> {
    let mut missing = false;
    let readable = ATTACHMENT_ORIGINAL_BUTTON.replace_all(html, "");
    let html = HTML_TAG
        .replace_all(&readable, |captures: &regex::Captures<'_>| {
            let tag = &captures[0];
            if tag.starts_with("<!--") {
                return tag.to_string();
            }
            HTML_ATTRIBUTE
                .replace_all(tag, |attribute: &regex::Captures<'_>| {
                    if !attribute[2].eq_ignore_ascii_case("data-local-path") {
                        return attribute[0].to_string();
                    }
                    let path = attribute
                        .get(3)
                        .or_else(|| attribute.get(4))
                        .or_else(|| attribute.get(5));
                    let Some(asset) = path.and_then(|path| resolve(path.as_str())) else {
                        missing = true;
                        return String::new();
                    };
                    let reference = match asset.dimensions {
                        Some((width, height)) => format!(
                            r#"data-asset-id="{}" width="{width}" height="{height}""#,
                            asset.id
                        ),
                        None => format!(r#"data-asset-id="{}""#, asset.id),
                    };
                    format!("{}{reference}", &attribute[1])
                })
                .into_owned()
        })
        .into_owned();
    if missing {
        return Err("本文の画像を解決できません".into());
    }
    Ok(html)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_become_asset_ids_and_unknown_paths_are_refused() {
        let html = r#"<p title="data-local-path=x">a</p><img class="novel-image" data-local-path="ab12" alt="x" /><button class="attachment-original">原本</button>"#;
        let out = mobile_html(html, |path| {
            (path == "ab12").then_some(MobileAsset {
                id: 3,
                dimensions: Some((10, 20)),
            })
        })
        .unwrap();
        assert_eq!(
            out,
            r#"<p title="data-local-path=x">a</p><img class="novel-image" data-asset-id="3" width="10" height="20" alt="x" />"#
        );
        assert!(mobile_html(html, |_| None).is_err());
    }
}
