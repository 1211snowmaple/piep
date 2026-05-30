//! JSON → 中間形式 (EpubManifest) 変換モジュール。
//!
//! Pixiv / Fanbox の `data.json` をパースして共通の `EpubManifest` に変換する。

use crate::epub::intermediate::*;
use serde_json::Value;
use std::path::Path;

// ============================================================
// パブリック API
// ============================================================

/// data.json をパースして EpubManifest に変換する
pub fn convert_to_manifest(
    data: &Value,
    source: &str,
    assets_dir: &Path,
) -> Result<EpubManifest, String> {
    match source {
        "pixiv" => convert_pixiv(data, assets_dir),
        "fanbox" => convert_fanbox(data, assets_dir),
        _ => Err(format!("未対応のソース: {}", source)),
    }
}

// ============================================================
// Pixiv 変換
// ============================================================

fn convert_pixiv(data: &Value, assets_dir: &Path) -> Result<EpubManifest, String> {
    // メタデータ抽出 (detail ラッパーを考慮)
    let detail = data.get("detail").unwrap_or(data);

    let novel_id = detail
        .get("id")
        .and_then(|v| {
            v.as_u64()
                .map(|n| n.to_string())
                .or_else(|| v.as_str().map(|s| s.to_string()))
        })
        .unwrap_or_default();

    let title = detail
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("無題")
        .to_string();

    let author_name = detail
        .get("user")
        .and_then(|u| u.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("不明")
        .to_string();

    let author_id = detail
        .get("user")
        .and_then(|u| u.get("id"))
        .map(|v| {
            v.as_u64()
                .map(|n| n.to_string())
                .or_else(|| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default()
        })
        .unwrap_or_default();

    let caption = detail
        .get("caption")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let create_date = detail
        .get("create_date")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // タグ抽出
    let keywords = extract_pixiv_tags(detail);

    // シリーズ情報
    let is_part_of = extract_pixiv_series(data);

    // シリーズIDの抽出
    let series_id = data
        .get("seriesId")
        .or_else(|| detail.get("seriesId"))
        .and_then(|v| {
            v.as_u64()
                .map(|n| n.to_string())
                .or_else(|| v.as_str().map(|s| s.to_string()))
        });

    // 本文テキスト取得
    let raw_text = data.get("text").and_then(|v| v.as_str()).unwrap_or("");

    let text_length = raw_text.chars().count() as u64;

    // 本文をHTML変換しページ分割
    let pages = convert_pixiv_text_to_pages(raw_text, data);

    // カバー画像
    let cover_image = find_cover_image(assets_dir);

    // 挿絵画像
    let illustrations = find_illustrations(assets_dir);

    let source_url = format!("https://www.pixiv.net/novel/show.php?id={}", novel_id);

    Ok(EpubManifest {
        core: EpubCore {
            id_: format!("urn:pixiv:novel:{}", novel_id),
            name: title,
            author: EpubAuthor {
                name: author_name,
                id: author_id,
            },
            description: caption,
            keywords,
            date_published: create_date.clone(),
            date_modified: Some(create_date),
            main_entity_of_page: source_url,
            is_part_of,
        },
        provider: ProviderData {
            source: "pixiv".to_string(),
            novel_id: Some(novel_id),
            post_id: None,
            series_id,
        },
        content: EpubContent {
            pages,
            cover_image,
            illustrations,
            text_length,
        },
    })
}

// ============================================================
// Fanbox 変換
// ============================================================

fn convert_fanbox(data: &Value, assets_dir: &Path) -> Result<EpubManifest, String> {
    let post_id = data
        .get("id")
        .and_then(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .or_else(|| v.as_u64().map(|n| n.to_string()))
        })
        .unwrap_or_default();

    let title = data
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("無題")
        .to_string();

    let author_name = data
        .get("user")
        .and_then(|u| u.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("不明")
        .to_string();

    let author_id = data
        .get("creatorId")
        .and_then(|v| v.as_str())
        .or_else(|| {
            data.get("user")
                .and_then(|u| u.get("userId"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("0")
        .to_string();

    let published = data
        .get("publishedDatetime")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let updated = data
        .get("updatedDatetime")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // タグ
    let keywords = data
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    // 本文HTML変換
    let (pages, text_length) = convert_fanbox_body_to_pages(data);

    // カバー画像
    let cover_image = find_cover_image(assets_dir);

    // 挿絵
    let illustrations = find_illustrations(assets_dir);

    let source_url = format!("https://{}.fanbox.cc/posts/{}", author_id, post_id);

    Ok(EpubManifest {
        core: EpubCore {
            id_: format!("urn:fanbox:post:{}", post_id),
            name: title,
            author: EpubAuthor {
                name: author_name,
                id: author_id,
            },
            description: None,
            keywords,
            date_published: published,
            date_modified: updated,
            main_entity_of_page: source_url,
            is_part_of: None,
        },
        provider: ProviderData {
            source: "fanbox".to_string(),
            novel_id: None,
            post_id: Some(post_id),
            series_id: None,
        },
        content: EpubContent {
            pages,
            cover_image,
            illustrations,
            text_length,
        },
    })
}

// ============================================================
// Pixiv ヘルパー関数
// ============================================================

fn extract_pixiv_tags(detail: &Value) -> Vec<String> {
    if let Some(tags) = detail.get("tags") {
        if let Some(arr) = tags.as_array() {
            return arr
                .iter()
                .filter_map(|t| {
                    // {name: "..."} 形式
                    t.get("name")
                        .and_then(|n| n.as_str())
                        .or_else(|| t.as_str())
                        .map(|s| s.to_string())
                })
                .collect();
        }
    }
    Vec::new()
}

fn extract_pixiv_series(data: &Value) -> Option<EpubSeries> {
    let detail = data.get("detail").unwrap_or(data);

    // seriesNavigation (App API形式)
    if let Some(nav) = detail
        .get("series")
        .or_else(|| data.get("seriesNavigation"))
    {
        let name = nav
            .get("title")
            .or_else(|| nav.get("seriesTitle"))
            .or_else(|| nav.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("シリーズ")
            .to_string();

        let order = nav.get("order").and_then(|v| v.as_u64()).map(|n| n as u32);

        return Some(EpubSeries { name, order });
    }

    // isPartOf (中間形式互換)
    if let Some(part) = data.get("isPartOf") {
        let name = part
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("シリーズ")
            .to_string();
        let order = part.get("order").and_then(|v| v.as_u64()).map(|n| n as u32);
        return Some(EpubSeries { name, order });
    }

    None
}

/// Pixiv小説記法をHTMLに変換し、[newpage] でページ分割する
fn convert_pixiv_text_to_pages(text: &str, data: &Value) -> Vec<EpubPage> {
    // [newpage] でページ分割
    let raw_pages: Vec<&str> = text.split("[newpage]").collect();

    raw_pages
        .iter()
        .enumerate()
        .map(|(i, page_text)| {
            let html = pixiv_text_to_html(page_text, data);
            let title = if raw_pages.len() > 1 {
                Some(format!("ページ {}", i + 1))
            } else {
                Some("本文".to_string())
            };
            EpubPage {
                title,
                html_content: html,
                order: (i + 1) as u32,
            }
        })
        .collect()
}

/// Pixiv小説記法を HTML に変換
fn pixiv_text_to_html(text: &str, _data: &Value) -> String {
    let mut html = String::new();

    for line in text.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            html.push_str("<br />\n");
            continue;
        }

        // [chapter: タイトル] → 見出し
        if let Some(chapter_title) = trimmed
            .strip_prefix("[chapter:")
            .and_then(|s| s.strip_suffix(']'))
        {
            html.push_str(&format!("<h2>{}</h2>\n", escape_html(chapter_title.trim())));
            continue;
        }

        // [pixivimage:xxxxx] → 挿絵プレースホルダー
        if trimmed.starts_with("[pixivimage:") && trimmed.ends_with(']') {
            let img_id = &trimmed[12..trimmed.len() - 1];
            // ページ番号を含む場合 (例: 12345-1)
            let clean_id = img_id.split('-').next().unwrap_or(img_id);
            html.push_str(&format!(
                "<div class=\"illustration\"><img src=\"../images/illust_{}.jpg\" alt=\"挿絵 {}\" /></div>\n",
                escape_html(clean_id),
                escape_html(img_id)
            ));
            continue;
        }

        // [uploadedimage:xxxxx] → アップロード画像
        if trimmed.starts_with("[uploadedimage:") && trimmed.ends_with(']') {
            let img_id = &trimmed[15..trimmed.len() - 1];
            html.push_str(&format!(
                "<div class=\"illustration\"><img src=\"../images/inline_{}.jpg\" alt=\"画像 {}\" /></div>\n",
                escape_html(img_id),
                escape_html(img_id)
            ));
            continue;
        }

        // [[rb: テキスト > ルビ]] → ルビ
        let processed = process_ruby(trimmed);

        html.push_str(&format!("<p>{}</p>\n", processed));
    }

    html
}

/// [[rb: テキスト > ルビ]] 記法をHTMLルビに変換
fn process_ruby(text: &str) -> String {
    let mut result = String::new();
    let mut remaining = text;

    while let Some(start) = remaining.find("[[rb:") {
        // rb タグの前の部分を追加
        result.push_str(&escape_html(&remaining[..start]));
        remaining = &remaining[start + 5..];

        if let Some(end) = remaining.find("]]") {
            let inner = &remaining[..end];
            if let Some(sep) = inner.find('>') {
                let base = inner[..sep].trim();
                let ruby = inner[sep + 1..].trim();
                result.push_str(&format!(
                    "<ruby>{}<rp>(</rp><rt>{}</rt><rp>)</rp></ruby>",
                    escape_html(base),
                    escape_html(ruby)
                ));
            } else {
                result.push_str(&escape_html(inner));
            }
            remaining = &remaining[end + 2..];
        } else {
            result.push_str("[[rb:");
        }
    }

    result.push_str(&escape_html(remaining));
    result
}

// ============================================================
// Fanbox ヘルパー関数
// ============================================================

fn convert_fanbox_body_to_pages(data: &Value) -> (Vec<EpubPage>, u64) {
    let mut html = String::new();
    let mut text_length: u64 = 0;

    if let Some(body) = data.get("body") {
        // article タイプ (blocks 配列)
        if let Some(blocks) = body.get("blocks").and_then(|b| b.as_array()) {
            for block in blocks {
                let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");

                match block_type {
                    "p" => {
                        if text.is_empty() {
                            html.push_str("<br />\n");
                        } else {
                            text_length += text.chars().count() as u64;
                            html.push_str(&format!("<p>{}</p>\n", escape_html(text)));
                        }
                    }
                    "header" => {
                        text_length += text.chars().count() as u64;
                        html.push_str(&format!("<h2>{}</h2>\n", escape_html(text)));
                    }
                    "image" => {
                        let image_id = block
                            .get("imageId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        html.push_str(&format!(
                            "<div class=\"illustration\"><img src=\"../images/{}.jpg\" alt=\"画像\" /></div>\n",
                            escape_html(image_id)
                        ));
                    }
                    "file" => {
                        let file_id = block
                            .get("fileId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("file");
                        html.push_str(&format!(
                            "<p class=\"file-link\">[添付ファイル: {}]</p>\n",
                            escape_html(file_id)
                        ));
                    }
                    _ => {
                        if !text.is_empty() {
                            text_length += text.chars().count() as u64;
                            html.push_str(&format!("<p>{}</p>\n", escape_html(text)));
                        }
                    }
                }
            }
        }
        // text タイプ (プレーンテキスト)
        else if let Some(text_val) = body.get("text").and_then(|t| t.as_str()) {
            text_length = text_val.chars().count() as u64;
            for line in text_val.lines() {
                if line.trim().is_empty() {
                    html.push_str("<br />\n");
                } else {
                    html.push_str(&format!("<p>{}</p>\n", escape_html(line)));
                }
            }
        }
    }

    let pages = vec![EpubPage {
        title: Some("本文".to_string()),
        html_content: html,
        order: 1,
    }];

    (pages, text_length)
}

// ============================================================
// 共通ヘルパー
// ============================================================

/// data_assets ディレクトリからカバー画像を検索
fn find_cover_image(assets_dir: &Path) -> Option<EpubImage> {
    let cover_dir = assets_dir.join("cover");
    if !cover_dir.exists() {
        return None;
    }

    if let Ok(entries) = std::fs::read_dir(&cover_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                let mime = match ext.as_str() {
                    "jpg" | "jpeg" => "image/jpeg",
                    "png" => "image/png",
                    "webp" => "image/webp",
                    "gif" => "image/gif",
                    _ => continue,
                };
                return Some(EpubImage {
                    id: "cover-image".to_string(),
                    local_path: path.to_string_lossy().to_string(),
                    mime_type: mime.to_string(),
                    alt_text: Some("カバー画像".to_string()),
                });
            }
        }
    }
    None
}

/// data_assets/illustrations ディレクトリから挿絵を検索
fn find_illustrations(assets_dir: &Path) -> Vec<EpubImage> {
    let illust_dir = assets_dir.join("illustrations");
    if !illust_dir.exists() {
        return Vec::new();
    }

    let mut images = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&illust_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            let mime = match ext.as_str() {
                "jpg" | "jpeg" => "image/jpeg",
                "png" => "image/png",
                "webp" => "image/webp",
                "gif" => "image/gif",
                _ => continue,
            };
            let filename = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("image")
                .to_string();
            images.push(EpubImage {
                id: format!("img-{}", filename),
                local_path: path.to_string_lossy().to_string(),
                mime_type: mime.to_string(),
                alt_text: Some(format!("挿絵 {}", filename)),
            });
        }
    }

    // ファイル名でソート
    images.sort_by(|a, b| a.id.cmp(&b.id));
    images
}

/// HTML特殊文字エスケープ
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}
