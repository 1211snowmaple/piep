//! Pixivの小説タグ置換およびFANBOXのブロックリッチテキスト構文解析を行う高速オンザフライパーサーエンジン。

use crate::database::models::AssetEntry;
use regex::Regex;
use std::sync::LazyLock;

// These patterns are fixed, but they used to be rebuilt on every call, so
// opening a document paid for eight regex compilations before any matching
// started. Compiling once keeps that cost off the per-document path.
static RE_RUBY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[rb:(?P<kanji>.+?)\s*&gt;\s*(?P<kana>.+?)\]\]").unwrap());
static RE_CHAPTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[chapter:(?P<title>.+?)\]").unwrap());
static RE_NEWPAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\[+newpage\s*\]+").unwrap());
static RE_JUMP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\[+jump\s*:\s*(?P<page>\d+)\s*\]+").unwrap());
static RE_JUMPURI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[\[jumpuri:(?P<title>.+?)\s*&gt;\s*(?P<url>https?://.+?)\]\]").unwrap()
});
static RE_NOVEL_SCHEME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"pixiv://novels/(?P<id>\d+)").unwrap());
static RE_ILLUST_SCHEME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"pixiv://illusts/(?P<id>\d+)").unwrap());
static RE_IMAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(?:uploadedimage|pixivimage):(?P<id>\d+)\]").unwrap());

/// HTML文字を安全にエスケープするインライン軽量エスケープ
fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Pixiv小説のプレーンテキストを XHTML/HTML へと動的パースする
pub fn parse_pixiv_to_html(raw_json: &str, assets: &[AssetEntry]) -> String {
    let v: serde_json::Value = match serde_json::from_str(raw_json) {
        Ok(json) => json,
        Err(_) => return String::new(),
    };

    // 本文テキストの抽出 (あらゆるネスト位置から発掘)
    let text = v
        .get("text")
        .or_else(|| v.get("novel").and_then(|n| n.get("text")))
        .or_else(|| v.get("detail").and_then(|d| d.get("text")))
        .and_then(|t| t.as_str())
        .unwrap_or("");

    if text.is_empty() {
        return String::new();
    }

    // 1. 安全のためにHTMLエスケープ
    let mut html = escape_html(text);

    // 2. ルビ [[rb:漢字 > ふりがな]] -> <ruby>漢字<rt>ふりがな</rt></ruby>
    // エスケープ後の ">" は "&gt;" になっているので "&gt;" でマッチさせる
    html = RE_RUBY
        .replace_all(&html, "<ruby>$kanji<rt>$kana</rt></ruby>")
        .to_string();

    // 3. 見出し [chapter:タイトル] -> <h2>タイトル</h2>
    html = RE_CHAPTER.replace_all(&html, "<h2>$title</h2>").to_string();

    // 3.5 改ページ [newpage] -> HTMLコメント <!-- newpage --> (大カッコ一重・二重、大文字小文字、スペース完全許容)
    html = RE_NEWPAGE
        .replace_all(&html, "<!-- newpage -->")
        .to_string();

    // 4. 改ページ [jump:ページ番号] -> ページ間リンク (大カッコ一重・二重、スペースの有無、大文字小文字を完全許容)
    html = RE_JUMP
        .replace_all(
            &html,
            r##"<a href="#" class="jump-link" data-page="$page">$pageページへ</a>"##,
        )
        .to_string();

    // 5. 外部リンク [[jumpuri:タイトル > URL]]
    // エスケープ後の ">" は "&gt;" になっているので "&gt;" でマッチさせる
    html = RE_JUMPURI
        .replace_all(
            &html,
            r##"<a href="$url" target="_blank" rel="noopener noreferrer">$title</a>"##,
        )
        .to_string();

    // 6. Pixiv内部スキーム
    // pixiv://novels/ID
    html = RE_NOVEL_SCHEME
        .replace_all(&html, "https://www.pixiv.net/novel/show.php?id=$id")
        .to_string();

    // pixiv://illusts/ID
    html = RE_ILLUST_SCHEME
        .replace_all(&html, "https://www.pixiv.net/artworks/$id")
        .to_string();

    // 7. 画像置換 [uploadedimage:ID] または [pixivimage:ID]
    html = RE_IMAGE
        .replace_all(&html, |caps: &regex::Captures| {
            let image_id = &caps["id"];
            // アセット配列から対応する画像を検索 (ファイル名に ID が含まれるものを探す)
            let found_asset = assets.iter().find(|a| a.filename.contains(image_id));
            if let Some(asset) = found_asset {
                format!(
                    r##"<img class="novel-image" data-local-path="{}" alt="image_{}" />"##,
                    escape_html(&asset.local_path),
                    image_id
                )
            } else {
                format!(
                    r##"<div class="missing-image-placeholder">画像 {}(見つかりません)</div>"##,
                    image_id
                )
            }
        })
        .to_string();

    // 8. 改行を <br /> に置換
    html = html.replace("\n", "<br />\n");

    html
}

/// FANBOXの構造化JSONブロックデータを XHTML/HTML へと動的パースする
pub fn parse_fanbox_to_html(raw_json: &str, assets: &[AssetEntry]) -> String {
    let v: serde_json::Value = match serde_json::from_str(raw_json) {
        Ok(json) => json,
        Err(_) => return String::new(),
    };

    // 1. もし FanboxResponse ラッパーが被っている場合 (生レスポンス `{ "body": { "id": ..., "body": ... } }`)
    // ラッパーの中身を `post` に特定
    let post = match v.get("body") {
        Some(body) if body.get("id").is_some() => body,
        _ => &v,
    };

    // 2. 古いプレーンテキスト形式 (PostBodyText) の場合
    if let Some(text) = post.get("text").and_then(|t| t.as_str()) {
        let escaped = escape_html(text);
        return escaped.replace("\n", "<br />\n");
    }

    let body = match post.get("body") {
        Some(b) => b,
        None => return String::new(),
    };

    // body.text がプレーンテキスト形式の場合のフォールバック
    if let Some(text) = body.get("text").and_then(|t| t.as_str()) {
        let escaped = escape_html(text);
        return escaped.replace("\n", "<br />\n");
    }

    // body.blocks を取得
    let blocks = match body.get("blocks").and_then(|b| b.as_array()) {
        Some(arr) => arr,
        None => return String::new(),
    };

    let mut html_parts = Vec::new();
    let num_blocks = blocks.len();

    for (i, block) in blocks.iter().enumerate() {
        let block_type = block
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("unknown");
        let part = match block_type {
            "header" | "h" => {
                let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
                let escaped = escape_html(text);
                format!("<h2>{}</h2>", escaped)
            }
            "p" | "paragraph" => parse_fanbox_paragraph(block),
            "image" => {
                let image_id = block
                    .get("imageId")
                    .and_then(|id| id.as_str())
                    .unwrap_or("");
                // アセット配列から対応する画像を検索 (ファイル名やローカルパスに画像IDが含まれているか)
                let found_asset = assets
                    .iter()
                    .find(|a| a.filename.contains(image_id) || a.local_path.contains(image_id));
                if let Some(asset) = found_asset {
                    format!(
                        r#"<img class="novel-image" data-local-path="{}" alt="image_{}" />"#,
                        escape_html(&asset.local_path),
                        image_id
                    )
                } else {
                    format!(
                        r#"<div class="missing-image-placeholder">画像 {}(見つかりません)</div>"#,
                        image_id
                    )
                }
            }
            "file" => {
                let file_id = block.get("fileId").and_then(|id| id.as_str()).unwrap_or("");
                let file_info = post
                    .get("body")
                    .and_then(|b| b.get("fileMap"))
                    .and_then(|m| m.get(file_id));

                let (original_name, file_size) = if let Some(info) = file_info {
                    let name = info.get("name").and_then(|n| n.as_str()).unwrap_or("file");
                    let ext = info.get("extension").and_then(|e| e.as_str()).unwrap_or("");
                    let size = info.get("size").and_then(|s| s.as_i64()).unwrap_or(0);
                    let full_name = if ext.is_empty() {
                        name.to_string()
                    } else {
                        format!("{}.{}", name, ext)
                    };
                    (full_name, size)
                } else {
                    ("添付ファイル".to_string(), 0)
                };

                // assets から対応するファイルを検索 (サニタイズされた名前が一致するか、またはパスに ID が含まれるか)
                let found_asset = assets.iter().find(|a| {
                    a.filename
                        == crate::downloader::asset_downloader::sanitize_filename(&original_name)
                        || a.local_path.contains(file_id)
                });

                if let Some(asset) = found_asset {
                    let size_kb = (file_size as f64) / 1024.0;
                    format!(
                        r##"<div class="novel-file-attachment clickable-file" data-local-path="{}">
                            <span class="file-icon">📎</span>
                            <div class="file-info">
                                <span class="file-name">{}</span>
                                <span class="file-size">{:.1} KB</span>
                            </div>
                        </div>"##,
                        escape_html(&asset.local_path),
                        escape_html(&original_name),
                        size_kb
                    )
                } else {
                    format!(
                        r#"<div class="missing-file-placeholder">ファイル {}(見つかりません)</div>"#,
                        escape_html(&original_name)
                    )
                }
            }
            "url_embed" => {
                let embed_id = block
                    .get("urlEmbedId")
                    .and_then(|id| id.as_str())
                    .unwrap_or("");
                let embed_info = post
                    .get("body")
                    .and_then(|b| b.get("urlEmbedMap").or_else(|| b.get("url_embed_map")))
                    .or_else(|| post.get("urlEmbedMap"))
                    .or_else(|| post.get("url_embed_map"))
                    .and_then(|m| m.get(embed_id));

                if let Some(info) = embed_info {
                    let mut url = info
                        .get("url")
                        .and_then(|u| u.as_str())
                        .unwrap_or("")
                        .to_string();
                    if url.is_empty() {
                        if let Some(t_str) = info.get("type").and_then(|t| t.as_str()) {
                            if t_str == "fanbox.post" {
                                let post_id = info
                                    .get("postInfo")
                                    .and_then(|pi| pi.get("id"))
                                    .and_then(|id| id.as_str())
                                    .unwrap_or("");
                                let creator_id = info
                                    .get("postInfo")
                                    .and_then(|pi| pi.get("creatorId"))
                                    .and_then(|cid| cid.as_str())
                                    .unwrap_or("");
                                if !post_id.is_empty() && !creator_id.is_empty() {
                                    url = format!(
                                        "https://{}.fanbox.cc/posts/{}",
                                        creator_id, post_id
                                    );
                                }
                            }
                        }
                    }

                    // FANBOX sometimes returns the creator-only management URL in an
                    // embed. Saved documents must always point at the public post URL.
                    if url.contains(".fanbox.cc/manage/posts/") {
                        url = url.replace(".fanbox.cc/manage/posts/", ".fanbox.cc/posts/");
                    }

                    if url.is_empty() {
                        String::new()
                    } else {
                        let title = info
                            .get("postInfo")
                            .and_then(|pi| pi.get("title"))
                            .and_then(|t| t.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| {
                                info.get("title")
                                    .and_then(|t| t.as_str())
                                    .map(|s| s.to_string())
                            })
                            .unwrap_or_else(|| url.clone());

                        let host = info
                            .get("host")
                            .and_then(|h| h.as_str())
                            .map(|s| s.to_string())
                            .or_else(|| {
                                url::Url::parse(&url)
                                    .ok()
                                    .and_then(|u| u.host_str().map(|h| h.to_string()))
                            })
                            .unwrap_or_else(|| "外部リンク".to_string());

                        let is_fanbox = host == "fanbox.cc"
                            || host.ends_with(".fanbox.cc")
                            || info.get("type").and_then(|value| value.as_str())
                                == Some("fanbox.post");
                        let creator_name = info
                            .get("postInfo")
                            .and_then(|post| post.get("user"))
                            .and_then(|user| user.get("name"))
                            .and_then(|value| value.as_str())
                            .unwrap_or("");
                        let fee = info
                            .get("postInfo")
                            .and_then(|post| post.get("feeRequired"))
                            .and_then(|value| value.as_i64())
                            .unwrap_or(0);
                        let meta = match (fee > 0, creator_name.is_empty()) {
                            (true, false) => format!("¥{} · {}", fee, creator_name),
                            (true, true) => format!("¥{}", fee),
                            (false, false) => creator_name.to_string(),
                            (false, true) => host.clone(),
                        };
                        let card_class = if is_fanbox {
                            "novel-link-card novel-link-card--fanbox"
                        } else {
                            "novel-link-card"
                        };
                        let brand = if is_fanbox { "F" } else { "↗" };

                        format!(
                            r##"<a href="{}" target="_blank" rel="noopener noreferrer" class="{}" data-provider="{}">
                                <span class="link-card-brand">{}</span>
                                <span class="link-card-info">
                                    <span class="link-card-kicker">{}</span>
                                    <span class="link-card-title">{}</span>
                                    <span class="link-card-host">{}</span>
                                </span>
                                <span class="link-card-arrow">↗</span>
                            </a>"##,
                            escape_html(&url),
                            card_class,
                            if is_fanbox { "fanbox" } else { "web" },
                            brand,
                            if is_fanbox {
                                "pixivFANBOX"
                            } else {
                                "外部リンク"
                            },
                            escape_html(&title),
                            escape_html(&meta)
                        )
                    }
                } else {
                    String::new()
                }
            }
            _ => {
                // 未対応ブロックのフォールバック
                format!(
                    r#"<div class="fallback-block" style="padding: 1em; margin: 1em 0; border: 1px dashed var(--color-border); color: var(--color-text-secondary); font-style: italic; border-radius: 6px;">サポートされていないコンテンツブロック (タイプ: {}) は表示できません。</div>"#,
                    escape_html(block_type)
                )
            }
        };

        if !part.is_empty() {
            html_parts.push(part.clone());
        }

        // 最後のブロック以外は、ブロック同士を <br /> で繋ぐ
        let is_last_block = i == num_blocks - 1;
        if !part.is_empty() && part != "<br />" && !is_last_block {
            html_parts.push("<br />".to_string());
        }
    }

    // The marker is an internal transport boundary and disappears when the
    // HTML is paged.  It lets the reader request one bounded set of complete
    // FANBOX blocks instead of receiving the entire post through one IPC call.
    html_parts.join("\n<!-- content-block -->\n")
}

/// FANBOXの段落ブロック内の装飾テキスト（逆順インデックス挿入アルゴリズム）を解析する
fn parse_fanbox_paragraph(block: &serde_json::Value) -> String {
    let raw_text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
    if raw_text.is_empty() {
        return "<br />".to_string();
    }

    let text_str = raw_text.to_string();
    let mut result_html = String::new();
    let chars: Vec<char> = text_str.chars().collect();

    let mut inserts_map: Vec<Vec<String>> = vec![Vec::new(); chars.len() + 1];
    let mut all_tags = Vec::new();

    if let Some(styles) = block.get("styles").and_then(|s| s.as_array()) {
        for style in styles {
            let style_type = style.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let offset = style.get("offset").and_then(|o| o.as_u64()).unwrap_or(0) as usize;
            let length = style.get("length").and_then(|l| l.as_u64()).unwrap_or(0) as usize;

            if style_type == "bold" {
                all_tags.push((offset, "<b>".to_string(), true));
                all_tags.push((offset + length, "</b>".to_string(), false));
            }
        }
    }

    if let Some(links) = block.get("links").and_then(|l| l.as_array()) {
        for link in links {
            let url = link.get("url").and_then(|u| u.as_str()).unwrap_or("");
            let offset = link.get("offset").and_then(|o| o.as_u64()).unwrap_or(0) as usize;
            let length = link.get("length").and_then(|l| l.as_u64()).unwrap_or(0) as usize;

            let escaped_url = escape_html(url);
            all_tags.push((
                offset,
                format!(
                    r#"<a href="{}" target="_blank" rel="noopener noreferrer">"#,
                    escaped_url
                ),
                true,
            ));
            all_tags.push((offset + length, "</a>".to_string(), false));
        }
    }

    all_tags.sort_by(|a, b| {
        if a.0 != b.0 {
            a.0.cmp(&b.0)
        } else {
            a.2.cmp(&b.2)
        }
    });

    for (offset, tag, _) in all_tags {
        if offset <= chars.len() {
            inserts_map[offset].push(tag);
        }
    }

    for i in 0..=chars.len() {
        for tag in &inserts_map[i] {
            result_html.push_str(tag);
        }

        if i < chars.len() {
            let c = chars[i];
            match c {
                '&' => result_html.push_str("&amp;"),
                '<' => result_html.push_str("&lt;"),
                '>' => result_html.push_str("&gt;"),
                '"' => result_html.push_str("&quot;"),
                '\'' => result_html.push_str("&#x27;"),
                _ => result_html.push(c),
            }
        }
    }

    result_html = result_html.replace("\n", "<br />\n");
    result_html
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fanbox_embed_uses_public_branded_card() {
        let json = serde_json::json!({
            "body": {
                "blocks": [{ "type": "url_embed", "urlEmbedId": "post" }],
                "urlEmbedMap": {
                    "post": {
                        "type": "fanbox.post",
                        "postInfo": {
                            "id": "123",
                            "creatorId": "creator",
                            "feeRequired": 500,
                            "title": "投稿タイトル",
                            "user": { "name": "作者" }
                        }
                    }
                }
            }
        });
        let html = parse_fanbox_to_html(&json.to_string(), &[]);
        assert!(html.contains("novel-link-card--fanbox"));
        assert!(html.contains("https://creator.fanbox.cc/posts/123"));
        assert!(html.contains("¥500 · 作者"));
        assert!(!html.contains('🔗'));
    }

    #[test]
    fn fanbox_management_embed_is_rewritten_for_reading() {
        let json = serde_json::json!({
            "body": {
                "blocks": [{ "type": "url_embed", "urlEmbedId": "link" }],
                "urlEmbedMap": {
                    "link": {
                        "type": "default",
                        "url": "https://creator.fanbox.cc/manage/posts/321",
                        "host": "creator.fanbox.cc"
                    }
                }
            }
        });
        let html = parse_fanbox_to_html(&json.to_string(), &[]);
        assert!(html.contains("https://creator.fanbox.cc/posts/321"));
        assert!(!html.contains("/manage/posts/"));
    }
}
