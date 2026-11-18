//! 編集したブロックを、読む画面の HTML と取得元の本文の形へ戻す。

use crate::parser::{escape_html as escape_editor_html, expand_pixiv_inline_notation};
use crate::{AssetEntry, WorkBlock};
use std::path::Path;

/// ブロックの前にあった空行の数を憶えておく鍵。
///
/// 段落の切れ目は取得元によって形が違う（pixiv は空行 1 つ、FANBOX は
/// ブロックの境界そのもので空行なし）。**数として持っておけば**、書き戻す
/// ときに元の見え方をそのまま再現できる。持たずに書き戻していたころは、
/// 編集画面を開いて何も変えずに反映するだけで、本文全体が 1 行に潰れた。
pub const BLOCK_GAP_KEY: &str = "gap";

/// 画像ブロックが元の本文で名乗っていた参照。EPUB へ書き出すときに、
/// 同じ絵をもう一度指すために使う。
pub const BLOCK_SOURCE_REF_KEY: &str = "sourceRef";

/// 取り込んだときに絵が見つからなかった画像ブロックの印。編集画面がそれと
/// 名乗り、手元の画像で埋めるよう促すのに使う。
pub const BLOCK_MISSING_KEY: &str = "missing";

/// 空行の上限。壊れた値や悪意のある値で本文が縦に伸び続けないようにする。
pub const MAX_BLOCK_GAP: usize = 64;

pub fn block_attrs(block: &WorkBlock) -> Option<serde_json::Value> {
    block
        .attrs_json
        .as_deref()
        .and_then(|attrs| serde_json::from_str::<serde_json::Value>(attrs).ok())
        .filter(serde_json::Value::is_object)
}

pub fn block_gap(block: &WorkBlock) -> Option<usize> {
    block_attrs(block)
        .and_then(|attrs| attrs.get(BLOCK_GAP_KEY).and_then(serde_json::Value::as_u64))
        .map(|gap| (gap as usize).min(MAX_BLOCK_GAP))
}

pub fn block_source_ref(block: &WorkBlock) -> Option<String> {
    block_attrs(block)
        .and_then(|attrs| {
            attrs
                .get(BLOCK_SOURCE_REF_KEY)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .filter(|value| !value.trim().is_empty())
}

pub fn blocks_to_plain_text(blocks: &[WorkBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block.block_type.as_str() {
            "image" | "separator" | "page_break" => None,
            _ => block.text.as_deref(),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 編集画面の「画像を追加」で取り込んだ絵の種別。
pub const EDITOR_IMAGE_ASSET_TYPE: &str = "editor_image";

/// 編集したブロックを、読書画面が受け取る HTML へ戻す。
///
/// 取り込んだ本文と**同じ組み方**で書く。行の終わりごとに `<br />` を置き、
/// 空行のぶんだけ足す。ただ連結していたころは行の区切りがどこにも残らず、
/// 「編集画面を開いて何も変えずに反映する」だけで作品全体がひと続きの塊に
/// なっていた。
///
/// ここが吐く HTML は取り込み直されない（版があるときブロックは DB から
/// 読む）ので、`html_to_editor_blocks` が知らない `<figure>` を使ってよい。
pub fn blocks_to_html(blocks: &[WorkBlock], assets: &[AssetEntry]) -> String {
    let mut html = String::new();
    for (index, block) in blocks.iter().enumerate() {
        let gap = block_gap(block).unwrap_or(if index == 0 { 0 } else { 1 });
        if index == 0 {
            for _ in 0..gap {
                html.push_str("<br />\n");
            }
        } else {
            // 直前の行を閉じる 1 つと、空行の数だけの `<br />`。
            for _ in 0..=gap {
                html.push_str("<br />\n");
            }
            // FANBOX の読書画面はこの印でページを切り出す。
            html.push_str("<!-- content-block -->\n");
        }
        html.push_str(&block_to_html(block, assets));
    }
    html
}

/// このブロックが指す絵を、取得元の本文が使っていた名前で呼ぶ。
///
/// 取り込んだときに控えた参照を第一に使い、編集画面から足した絵は
/// ファイル名から引く。EPUB のビルダーはどちらの形でも実ファイルへ辿れる。
pub fn block_image_reference(block: &WorkBlock, assets: &[AssetEntry]) -> Option<String> {
    let asset = block
        .asset_id
        .and_then(|id| assets.iter().find(|asset| asset.id == id));
    // 編集で足した画像を選んだブロックは、その画像を指す。元の参照は
    // 選び直す前の絵（取得できなかった絵を含む）を名乗っているので、
    // 優先すると EPUB に古い絵が出るか、何も出なかった。
    let added = asset.is_some_and(|asset| asset.asset_type == EDITOR_IMAGE_ASSET_TYPE);
    if !added {
        if let Some(reference) = block_source_ref(block) {
            return Some(reference);
        }
    }
    let asset = asset?;
    Path::new(&asset.filename)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string)
}

pub fn block_link_label(block: &WorkBlock) -> Option<String> {
    block_attrs(block)
        .and_then(|attrs| {
            attrs
                .get("label")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .filter(|label| !label.trim().is_empty())
}

/// 反映済みの編集を、pixiv の本文記法へ組み直す。
///
/// 平文へ均していたころは、`blocks_to_plain_text` が画像・区切り・改ページを
/// 落とし、見出しをただの行にしていた。編集した作品の EPUB には挿絵が 1 枚も
/// 入らず、章も改ページも消えていた。
pub fn blocks_to_pixiv_text(blocks: &[WorkBlock], assets: &[AssetEntry]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let gap = block_gap(block).unwrap_or(if index == 0 { 0 } else { 1 });
        for _ in 0..gap {
            lines.push(String::new());
        }
        let text = block.text.as_deref().unwrap_or("");
        match block.block_type.as_str() {
            "heading" => lines.push(format!("[chapter:{}]", text.trim())),
            "page_break" => lines.push("[newpage]".to_string()),
            "separator" => lines.push(crate::parser::EDITOR_SEPARATOR_NOTATION.to_string()),
            "image" => {
                if let Some(reference) = block_image_reference(block, assets) {
                    // 取り込んだ挿絵は、元の記法（`[pixivimage:…]`）ごと控えてある。
                    // それをさらに包んでいたので、`[uploadedimage:[pixivimage:…]]`
                    // という誰も読めない参照になり、**編集した作品の EPUB からは
                    // 挿絵が 1 枚残らず消えていた**。
                    lines.push(if reference.starts_with('[') {
                        reference
                    } else {
                        format!("[uploadedimage:{reference}]")
                    });
                }
            }
            "link" => {
                let url = text.trim();
                if !url.is_empty() {
                    let label = block_link_label(block).unwrap_or_else(|| url.to_string());
                    lines.push(format!("[[jumpuri:{label}>{url}]]"));
                }
            }
            _ => lines.extend(text.split('\n').map(str::to_string)),
        }
    }
    lines.join("\n")
}

/// 反映済みの編集を、FANBOX の本文ブロックへ組み直す。
pub fn blocks_to_fanbox_blocks(
    blocks: &[WorkBlock],
    assets: &[AssetEntry],
) -> Vec<serde_json::Value> {
    let blank = || serde_json::json!({ "type": "p", "text": "" });
    let mut out = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let gap = block_gap(block).unwrap_or(if index == 0 { 0 } else { 1 });
        for _ in 0..gap {
            out.push(blank());
        }
        let text = block.text.as_deref().unwrap_or("");
        match block.block_type.as_str() {
            "heading" => out.push(serde_json::json!({ "type": "header", "text": text.trim() })),
            "image" => {
                if let Some(reference) = block_image_reference(block, assets) {
                    out.push(serde_json::json!({ "type": "image", "imageId": reference }));
                }
            }
            "separator" | "page_break" => out.push(blank()),
            "link" => {
                let url = text.trim();
                if !url.is_empty() {
                    let label = block_link_label(block).unwrap_or_else(|| url.to_string());
                    // FANBOX のリンクは本文の範囲で表す。長さは UTF-16 の単位。
                    out.push(serde_json::json!({
                        "type": "p",
                        "text": label,
                        "links": [{ "url": url, "offset": 0, "length": label.encode_utf16().count() }],
                    }));
                }
            }
            _ => out.push(serde_json::json!({ "type": "p", "text": text })),
        }
    }
    out
}

pub fn block_to_html(block: &WorkBlock, assets: &[AssetEntry]) -> String {
    match block.block_type.as_str() {
        "heading" => format!(
            "<h2>{}</h2>",
            expand_pixiv_inline_notation(&escape_editor_html(block.text.as_deref().unwrap_or("")))
        ),
        "image" => {
            let asset = block
                .asset_id
                .and_then(|id| assets.iter().find(|asset| asset.id == id));
            let Some(asset) = asset else {
                // 取り込んだときから絵が無かったものは、どの絵だったかを残す。
                return match block_source_ref(block) {
                    Some(reference) => format!(
                        r#"<div class="missing-image-placeholder" role="note" data-image-reference="{}" data-image-missing="true">取得できなかった画像 {}</div>"#,
                        escape_editor_html(&reference),
                        escape_editor_html(&reference)
                    ),
                    None => "<div class=\"missing-image-placeholder\">画像が見つかりません</div>"
                        .to_string(),
                };
            };
            let caption = block
                .text
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let image = format!(
                r#"<img class="novel-image" data-local-path="{}" alt="{}" />"#,
                escape_editor_html(&asset.local_path),
                escape_editor_html(caption.unwrap_or(&asset.filename))
            );
            // 書いた説明が読書画面にも EPUB にも出ないまま捨てられていた。
            match caption {
                Some(caption) => format!(
                    r#"<figure class="novel-figure">{}<figcaption>{}</figcaption></figure>"#,
                    image,
                    escape_editor_html(caption)
                ),
                None => image,
            }
        }
        "separator" => "<hr />".to_string(),
        "page_break" => "<!-- newpage -->".to_string(),
        "quote" => format!(
            "<blockquote>{}</blockquote>",
            expand_pixiv_inline_notation(&escape_editor_html(block.text.as_deref().unwrap_or("")))
                .replace('\n', "<br />\n")
        ),
        "link" => {
            let url = block.text.as_deref().unwrap_or("");
            let label = block_attrs(block)
                .and_then(|attrs| {
                    attrs
                        .get("label")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                })
                .filter(|label| !label.trim().is_empty())
                .unwrap_or_else(|| url.to_string());
            format!(
                r#"<a href="{}" target="_blank" rel="noopener noreferrer" class="novel-link-card"><span class="link-card-icon">🔗</span><span class="link-card-info"><span class="link-card-title">{}</span><span class="link-card-host">{}</span></span></a>"#,
                escape_editor_html(url),
                escape_editor_html(&label),
                escape_editor_html(url)
            )
        }
        // 地の文。書いた記法をそのまま開く ―― 取り込みと同じ関数を通すので、
        // 編集画面で足したルビも読書画面でルビになる。
        _ => expand_pixiv_inline_notation(&escape_editor_html(block.text.as_deref().unwrap_or("")))
            .replace('\n', "<br />\n"),
    }
}
