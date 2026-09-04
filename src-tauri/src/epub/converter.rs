//! JSON → 中間形式 (EpubManifest) 変換モジュール。
//!
//! Pixiv / Fanbox の `data.json` をパースして共通の `EpubManifest` に変換する。
//!
//! ここで作る本文は、この時点ですでに整形式 XHTML でなければならない。
//! 後段のビルダーは画像参照の解決しか行わないため、記法の展開もエスケープも
//! すべてこのモジュールの責任になる。

use crate::epub::intermediate::*;
use crate::epub::meta;
use crate::epub::xhtml;
use regex::Regex;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

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
        "pixiv" => Ok(convert_pixiv(data, assets_dir)),
        "fanbox" => Ok(convert_fanbox(data, assets_dir)),
        _ => Err(format!("未対応のソース: {}", source)),
    }
}

// ============================================================
// Pixiv 変換
// ============================================================

fn convert_pixiv(data: &Value, assets_dir: &Path) -> EpubManifest {
    // メタデータ抽出 (detail ラッパーを考慮)
    let detail = data.get("detail").unwrap_or(data);

    let novel_id = read_id(detail.get("id")).unwrap_or_default();
    let title = read_str(detail.get("title")).unwrap_or_else(|| "無題".to_string());
    let author_name = read_str(detail.get("user").and_then(|u| u.get("name")))
        .unwrap_or_else(|| "不明".to_string());
    let author_id = read_id(detail.get("user").and_then(|u| u.get("id"))).unwrap_or_default();
    let account = read_str(detail.get("user").and_then(|u| u.get("account")));
    let icon_url = read_str(detail.get("user").and_then(|u| u.get("profile_image_url")));

    let caption = read_str(detail.get("caption"));
    let create_date = read_str(detail.get("create_date")).unwrap_or_default();
    let update_date = read_str(detail.get("update_date")).or_else(|| create_date_fallback(detail));

    let tags = extract_pixiv_tags(detail);
    let is_part_of = extract_pixiv_series(data, detail);
    let series_id = is_part_of.as_ref().and_then(|series| series.id.clone());

    let raw_text = read_str(data.get("text"))
        .or_else(|| read_str(detail.get("text")))
        .unwrap_or_default();

    // pixiv 自身が数えた文字数があればそちらを信じる。
    let text_length = detail
        .get("text_length")
        .and_then(|v| v.as_u64())
        .unwrap_or_else(|| raw_text.chars().count() as u64);

    let pages = convert_pixiv_text_to_pages(&raw_text);
    let cover_image = find_cover_image(assets_dir);
    let illustrations = find_illustrations(assets_dir);
    let source_url = format!("https://www.pixiv.net/novel/show.php?id={}", novel_id);

    let author_url =
        (!author_id.is_empty()).then(|| format!("https://www.pixiv.net/users/{}", author_id));

    build_manifest(
        EpubCore {
            id_: meta::source_urn("pixiv", "novel", &novel_id),
            name: title,
            author: EpubAuthor {
                name: author_name,
                id: author_id,
                account,
                url: author_url,
                icon_url,
            },
            description: caption.as_deref().map(xhtml::sanitize_fragment),
            description_text: caption.as_deref().map(xhtml::strip_tags),
            keywords: tags.iter().map(|tag| tag.name.clone()).collect(),
            tags,
            date_published: create_date,
            date_modified: update_date,
            main_entity_of_page: source_url,
            is_part_of,
            language: "ja".to_string(),
            publisher: "pixiv".to_string(),
        },
        ProviderData {
            source: "pixiv".to_string(),
            novel_id: Some(novel_id),
            post_id: None,
            series_id,
            post_type: None,
        },
        pages,
        cover_image,
        illustrations,
        Vec::new(),
        text_length,
        PartialStats {
            like_count: detail.get("total_bookmarks").and_then(|v| v.as_u64()),
            comment_count: detail.get("total_comments").and_then(|v| v.as_u64()),
            fee_required: None,
            adult: has_adult_tag(detail),
        },
    )
}

fn create_date_fallback(detail: &Value) -> Option<String> {
    read_str(detail.get("create_date"))
}

fn has_adult_tag(detail: &Value) -> bool {
    detail
        .get("x_restrict")
        .and_then(|v| v.as_u64())
        .is_some_and(|value| value > 0)
        || extract_pixiv_tags(detail)
            .iter()
            .any(|tag| matches!(tag.name.as_str(), "R-18" | "R-18G"))
}

// ============================================================
// Fanbox 変換
// ============================================================

fn convert_fanbox(data: &Value, assets_dir: &Path) -> EpubManifest {
    let data = crate::fanbox_api::payload::post_or_self(data);
    let post_id = read_id(data.get("id")).unwrap_or_default();
    let title = read_str(data.get("title")).unwrap_or_else(|| "無題".to_string());
    let author_name = read_str(data.get("user").and_then(|u| u.get("name")))
        .unwrap_or_else(|| "不明".to_string());
    let creator_id = read_str(data.get("creatorId"))
        .or_else(|| read_str(data.get("user").and_then(|u| u.get("userId"))))
        .unwrap_or_else(|| "0".to_string());
    let user_id = read_id(data.get("user").and_then(|u| u.get("userId")))
        .unwrap_or_else(|| creator_id.clone());
    let icon_url = read_str(data.get("user").and_then(|u| u.get("iconUrl")));

    let published = read_str(data.get("publishedDatetime")).unwrap_or_default();
    let updated = read_str(data.get("updatedDatetime"));
    let excerpt = read_str(data.get("excerpt")).filter(|text| !text.trim().is_empty());

    let tags: Vec<EpubTag> = data
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|name| EpubTag {
                    name: name.to_string(),
                    translated: None,
                })
                .collect()
        })
        .unwrap_or_default();

    let (pages, text_length, attachments) = convert_fanbox_body_to_pages(data);
    let cover_image = find_cover_image(assets_dir);
    let illustrations = find_illustrations(assets_dir);
    let source_url = format!("https://{}.fanbox.cc/posts/{}", creator_id, post_id);
    let post_type = read_str(data.get("type"));

    build_manifest(
        EpubCore {
            id_: meta::source_urn("fanbox", "post", &post_id),
            name: title,
            author: EpubAuthor {
                name: author_name,
                id: user_id,
                account: Some(creator_id.clone()),
                url: Some(format!("https://{}.fanbox.cc", creator_id)),
                icon_url,
            },
            description: excerpt.as_deref().map(xhtml::sanitize_fragment),
            description_text: excerpt.as_deref().map(xhtml::strip_tags),
            keywords: tags.iter().map(|tag| tag.name.clone()).collect(),
            tags,
            date_published: published,
            date_modified: updated,
            main_entity_of_page: source_url,
            is_part_of: None,
            language: "ja".to_string(),
            publisher: "pixivFANBOX".to_string(),
        },
        ProviderData {
            source: "fanbox".to_string(),
            novel_id: None,
            post_id: Some(post_id),
            series_id: None,
            post_type,
        },
        pages,
        cover_image,
        illustrations,
        attachments,
        text_length,
        PartialStats {
            like_count: data.get("likeCount").and_then(|v| v.as_u64()),
            comment_count: data.get("commentCount").and_then(|v| v.as_u64()),
            fee_required: data.get("feeRequired").and_then(|v| v.as_u64()),
            adult: data
                .get("hasAdultContent")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        },
    )
}

// ============================================================
// 組み立て
// ============================================================

struct PartialStats {
    like_count: Option<u64>,
    comment_count: Option<u64>,
    fee_required: Option<u64>,
    adult: bool,
}

#[allow(clippy::too_many_arguments)]
fn build_manifest(
    core: EpubCore,
    provider: ProviderData,
    pages: Vec<EpubPage>,
    cover_image: Option<EpubImage>,
    illustrations: Vec<EpubImage>,
    attachments: Vec<EpubAttachment>,
    text_length: u64,
    partial: PartialStats,
) -> EpubManifest {
    let stats = EpubStats {
        text_length,
        page_count: pages.len() as u32,
        chapter_count: pages.iter().map(|page| page.chapters.len() as u32).sum(),
        image_count: illustrations.len() as u32 + u32::from(cover_image.is_some()),
        attachment_count: attachments.len() as u32,
        like_count: partial.like_count,
        comment_count: partial.comment_count,
        fee_required: partial.fee_required,
        adult: partial.adult,
    };
    EpubManifest {
        core,
        provider,
        content: EpubContent {
            pages,
            cover_image,
            illustrations,
            attachments,
            text_length,
        },
        stats,
    }
}

// ============================================================
// Pixiv ヘルパー関数
// ============================================================

fn extract_pixiv_tags(detail: &Value) -> Vec<EpubTag> {
    let Some(tags) = detail.get("tags").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    tags.iter()
        .filter_map(|tag| {
            let name = read_str(tag.get("name"))
                .or_else(|| read_str(tag.get("tag")))
                .or_else(|| tag.as_str().map(str::to_string))?;
            let translated = read_str(tag.get("translated_name"))
                .or_else(|| read_str(tag.get("translatedName")))
                .filter(|value| !value.trim().is_empty());
            (!name.trim().is_empty()).then_some(EpubTag { name, translated })
        })
        .collect()
}

/// シリーズ情報を拾う。
///
/// 保存済みの pixiv データは `detail.series_id` / `detail.series_title` を持つ。
/// App API 由来の `series` オブジェクトや中間形式の `isPartOf` も同じ形に寄せる。
fn extract_pixiv_series(data: &Value, detail: &Value) -> Option<EpubSeries> {
    let from_flat = || {
        let id = read_id(detail.get("series_id"))?;
        let name = read_str(detail.get("series_title"))
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "シリーズ".to_string());
        Some(EpubSeries {
            name,
            order: read_u32(detail.get("series_order")),
            id: Some(id),
            url: None,
        })
    };
    let from_object = || {
        let node = detail
            .get("series")
            .or_else(|| data.get("seriesNavigation"))
            .or_else(|| data.get("isPartOf"))?;
        if node.is_null() {
            return None;
        }
        let name = read_str(node.get("title"))
            .or_else(|| read_str(node.get("seriesTitle")))
            .or_else(|| read_str(node.get("name")))
            .filter(|value| !value.trim().is_empty())?;
        Some(EpubSeries {
            name,
            order: read_u32(node.get("order")).or_else(|| read_u32(node.get("contentOrder"))),
            id: read_id(node.get("id")).or_else(|| read_id(node.get("seriesId"))),
            url: None,
        })
    };

    let mut series = from_flat().or_else(from_object)?;
    if let Some(id) = &series.id {
        series.url = Some(format!("https://www.pixiv.net/novel/series/{}", id));
    }
    Some(series)
}

/// Pixiv小説記法をHTMLに変換し、`[newpage]` でページ分割する
fn convert_pixiv_text_to_pages(text: &str) -> Vec<EpubPage> {
    // 改ページの見分け方は読書画面と同じものを使う。こちらだけが
    // `"[newpage]"` の完全一致だったので、`[[newpage]]` や `[NewPage]` と
    // 書かれた作品は、読むときはページが分かれるのに本にすると 1 ページに
    // 融合していた。同じ作品が、読むときと本にするときで別の構造になる。
    let raw_pages: Vec<&str> = crate::database::parser::split_pixiv_pages(text);
    let multi_page = raw_pages.len() > 1;
    let page_count = raw_pages.len();
    let mut chapter_seq = 0usize;

    raw_pages
        .iter()
        .enumerate()
        .map(|(index, page_text)| {
            let rendered = pixiv_text_to_html(page_text, &mut chapter_seq, page_count);
            // 章見出しで始まるページは、その見出しを目次の見出しにする。
            let title = rendered
                .chapters
                .first()
                .map(|chapter| chapter.title.clone())
                .or_else(|| multi_page.then(|| format!("ページ {}", index + 1)))
                .unwrap_or_else(|| "本文".to_string());
            EpubPage {
                title: Some(title),
                html_content: rendered.html,
                order: (index + 1) as u32,
                chapters: rendered.chapters,
            }
        })
        .collect()
}

struct RenderedText {
    html: String,
    chapters: Vec<EpubChapter>,
}

/// Pixiv小説記法を XHTML に変換
fn pixiv_text_to_html(text: &str, chapter_seq: &mut usize, page_count: usize) -> RenderedText {
    let mut html = String::new();
    let mut chapters = Vec::new();
    // 連続する空行は段落の区切りとして一度だけ空ける。行ごとに <br /> を積むと
    // 読み手側で不揃いな空白になり、字下げのある小説では特に目立つ。
    let mut pending_blank = false;

    for line in text.lines() {
        let trimmed = line.trim_end_matches(['\r', ' ', '\u{3000}']);

        if trimmed.trim().is_empty() {
            pending_blank = true;
            continue;
        }
        if pending_blank && !html.is_empty() {
            html.push_str("<p class=\"blank-line\"><br /></p>\n");
        }
        pending_blank = false;

        // [chapter: タイトル] → 見出し
        if let Some(chapter_title) = trimmed
            .trim()
            .strip_prefix("[chapter:")
            .and_then(|rest| rest.strip_suffix(']'))
        {
            *chapter_seq += 1;
            let id = format!("chapter-{:03}", chapter_seq);
            let title = chapter_title.trim().to_string();
            html.push_str(&format!(
                "<h2 id=\"{}\">{}</h2>\n",
                xhtml::escape_attr(&id),
                xhtml::escape_text(&title)
            ));
            chapters.push(EpubChapter { id, title });
            continue;
        }

        // 編集画面で足した区切り線。pixiv 本来の記法には無い印なので、
        // 取り込んだ本文がこれに当たることはない。
        if trimmed.trim() == crate::database::parser::EDITOR_SEPARATOR_NOTATION {
            html.push_str("<hr />\n");
            continue;
        }

        // [pixivimage:xxxxx] → 挿絵
        if let Some(reference) = bracketed(trimmed.trim(), "[pixivimage:") {
            // "12345-2" は作品 12345 の 2 枚目。ファイル名は 0 起点で付く。
            let (illust_id, page) = match reference.split_once('-') {
                Some((id, page)) => (id, page.parse::<usize>().unwrap_or(1).saturating_sub(1)),
                None => (reference, 0),
            };
            html.push_str(&illustration_html(
                &format!("{}_p{}", illust_id, page),
                &format!("挿絵 {}", reference),
            ));
            continue;
        }

        // [uploadedimage:xxxxx] → 本文に挿入された画像
        if let Some(reference) = bracketed(trimmed.trim(), "[uploadedimage:") {
            html.push_str(&illustration_html(
                reference,
                &format!("画像 {}", reference),
            ));
            continue;
        }

        html.push_str(&format!(
            "<p>{}</p>\n",
            inline_pixiv_notation(trimmed, page_count)
        ));
    }

    RenderedText { html, chapters }
}

fn bracketed<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    line.strip_prefix(prefix)?.strip_suffix(']')
}

/// 挿絵一枚分のマークアップ。参照先はビルダーが実ファイルへ解決する。
fn illustration_html(reference: &str, alt: &str) -> String {
    format!(
        "<div class=\"illustration\"><img src=\"../images/{}.jpg\" alt=\"{}\" /></div>\n",
        xhtml::escape_attr(reference),
        xhtml::escape_attr(alt)
    )
}

/// pixiv のアプリ内アドレスを、誰でも辿れる住所へ置き換える。
///
/// 読書画面は最初からこうしている。EPUB だけが `pixiv://…` を素通しにして
/// いたので、本の中には**どの端末でも開けないリンク**が残っていた。
fn rewrite_pixiv_scheme(url: &str) -> String {
    static NOVEL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^pixiv://novels/(\d+)$").expect("valid novel scheme regex"));
    static ILLUST: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^pixiv://illusts/(\d+)$").expect("valid illust scheme regex")
    });
    if let Some(captures) = NOVEL.captures(url) {
        return format!("https://www.pixiv.net/novel/show.php?id={}", &captures[1]);
    }
    if let Some(captures) = ILLUST.captures(url) {
        return format!("https://www.pixiv.net/artworks/{}", &captures[1]);
    }
    url.to_string()
}

/// 行内記法（ルビ・リンク）を展開しつつ、それ以外をすべてエスケープする。
fn inline_pixiv_notation(text: &str, page_count: usize) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(start) = rest.find("[[") {
        let Some(end_offset) = rest[start..].find("]]") else {
            break;
        };
        let end = start + end_offset;
        out.push_str(&xhtml::escape_text(&rest[..start]));
        let inner = &rest[start + 2..end];
        rest = &rest[end + 2..];

        if let Some(body) = inner.strip_prefix("rb:") {
            match body.split_once('>') {
                Some((base, ruby)) => out.push_str(&format!(
                    "<ruby>{}<rp>(</rp><rt>{}</rt><rp>)</rp></ruby>",
                    xhtml::escape_text(base.trim()),
                    xhtml::escape_text(ruby.trim())
                )),
                None => out.push_str(&xhtml::escape_text(body)),
            }
            continue;
        }
        if let Some(body) = inner.strip_prefix("jumpuri:") {
            match body.split_once('>') {
                Some((label, url)) => {
                    // pixiv のアプリ内アドレスは、読書画面と同じく普通の住所へ。
                    let url = rewrite_pixiv_scheme(url.trim());
                    // 外部リンクは残すが、読み手が辿れない書式なら文字として置く。
                    if url.starts_with("http://") || url.starts_with("https://") {
                        out.push_str(&format!(
                            "<a href=\"{}\">{}</a>",
                            xhtml::escape_attr(&url),
                            xhtml::escape_text(label.trim())
                        ));
                    } else {
                        out.push_str(&xhtml::escape_text(label.trim()));
                    }
                }
                None => out.push_str(&xhtml::escape_text(body)),
            }
            continue;
        }
        // 知らない [[…]] は記法として壊れている。中身を字面のまま残す。
        out.push_str(&xhtml::escape_text(&format!("[[{}]]", inner)));
    }

    out.push_str(&xhtml::escape_text(rest));
    // [jump:N] はページ内移動。飛び先が本の中にあるなら、本の中のリンクにする。
    // 字面を残していたころは、読み手には `[jump:5]` という壊れた文字列だけが
    // 見えていた。組み立て終わった文字列の上で置き換えるので、ここで足す
    // タグが二重にエスケープされることはない。
    static JUMP: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)\[+jump\s*:\s*(\d+)\s*\]+").expect("valid jump regex"));
    JUMP.replace_all(&out, |captures: &regex::Captures| {
        let page: usize = captures[1].parse().unwrap_or(0);
        if page >= 1 && page <= page_count {
            format!(
                r#"<a href="page_{:03}.xhtml">{}ページへ</a>"#,
                page as u32, page
            )
        } else {
            // 飛び先が無いページ番号は、リンクにしても行き止まりになる。
            captures[0].to_string()
        }
    })
    .into_owned()
}

// ============================================================
// Fanbox ヘルパー関数
// ============================================================

fn convert_fanbox_body_to_pages(data: &Value) -> (Vec<EpubPage>, u64, Vec<EpubAttachment>) {
    let mut html = String::new();
    let mut text_length: u64 = 0;
    let mut chapters = Vec::new();
    let mut attachments = Vec::new();
    let mut chapter_seq = 0usize;
    let mut pending_blank = false;

    let Some(body) = data.get("body") else {
        return (
            vec![EpubPage {
                title: Some("本文".to_string()),
                html_content: html,
                order: 1,
                chapters,
            }],
            0,
            attachments,
        );
    };

    let image_extensions = fanbox_image_extensions(body);

    if let Some(blocks) = body.get("blocks").and_then(|value| value.as_array()) {
        for block in blocks {
            let block_type = read_str(block.get("type")).unwrap_or_default();
            let text = read_str(block.get("text")).unwrap_or_default();

            match block_type.as_str() {
                "p" => {
                    if text.trim().is_empty() {
                        pending_blank = true;
                        continue;
                    }
                    if pending_blank && !html.is_empty() {
                        html.push_str("<p class=\"blank-line\"><br /></p>\n");
                    }
                    pending_blank = false;
                    text_length += text.chars().count() as u64;
                    // 段落の途中の改行は投稿者が入れた行送り。そのまま流すと
                    // XHTML では空白 1 つに畳まれ、詩や会話が 1 行に潰れる。
                    html.push_str(&format!(
                        "<p>{}</p>\n",
                        decorate_fanbox_text(&text, block).replace('\n', "<br />")
                    ));
                }
                "header" => {
                    pending_blank = false;
                    chapter_seq += 1;
                    text_length += text.chars().count() as u64;
                    let id = format!("chapter-{:03}", chapter_seq);
                    html.push_str(&format!(
                        "<h2 id=\"{}\">{}</h2>\n",
                        xhtml::escape_attr(&id),
                        xhtml::escape_text(&text)
                    ));
                    chapters.push(EpubChapter {
                        id,
                        title: text.clone(),
                    });
                }
                "image" => {
                    pending_blank = false;
                    if let Some(image_id) = read_str(block.get("imageId")) {
                        // 拡張子を当て推量にすると参照先が存在せず EPUB が壊れる。
                        let extension = image_extensions
                            .get(&image_id)
                            .cloned()
                            .unwrap_or_else(|| "jpg".to_string());
                        html.push_str(&format!(
                            "<div class=\"illustration\"><img src=\"../images/{}.{}\" alt=\"画像\" /></div>\n",
                            xhtml::escape_attr(&image_id),
                            xhtml::escape_attr(&extension)
                        ));
                    }
                }
                "file" => {
                    pending_blank = false;
                    if let Some(file) =
                        read_str(block.get("fileId")).and_then(|id| fanbox_file(body, &id))
                    {
                        html.push_str(&format!(
                            "<p class=\"file-link\">添付ファイル: {}</p>\n",
                            xhtml::escape_text(&file.name)
                        ));
                        attachments.push(file);
                    }
                }
                "url_embed" => {
                    pending_blank = false;
                    if let Some((label, url)) =
                        read_str(block.get("urlEmbedId")).and_then(|id| fanbox_embed(body, &id))
                    {
                        html.push_str(&format!(
                            "<p class=\"embed-link\"><a href=\"{}\">{}</a></p>\n",
                            xhtml::escape_attr(&url),
                            xhtml::escape_text(&label)
                        ));
                    }
                }
                _ => {
                    if !text.trim().is_empty() {
                        pending_blank = false;
                        text_length += text.chars().count() as u64;
                        html.push_str(&format!("<p>{}</p>\n", xhtml::escape_text(&text)));
                    }
                }
            }
        }
    }

    // text 投稿は本文が丸ごと一つの文字列で来る。
    if let Some(plain) = read_str(body.get("text")).filter(|value| !value.trim().is_empty()) {
        if html.is_empty() {
            text_length = plain.chars().count() as u64;
            for line in plain.lines() {
                if line.trim().is_empty() {
                    pending_blank = true;
                    continue;
                }
                if pending_blank && !html.is_empty() {
                    html.push_str("<p class=\"blank-line\"><br /></p>\n");
                }
                pending_blank = false;
                html.push_str(&format!("<p>{}</p>\n", xhtml::escape_text(line)));
            }
        }
    }

    // image 投稿と file 投稿は blocks を持たず、配列だけで本文を構成する。
    if let Some(images) = body.get("images").and_then(|value| value.as_array()) {
        for image in images {
            let Some(id) = read_str(image.get("id")) else {
                continue;
            };
            let extension = read_str(image.get("extension")).unwrap_or_else(|| "jpg".to_string());
            html.push_str(&format!(
                "<div class=\"illustration\"><img src=\"../images/{}.{}\" alt=\"画像\" /></div>\n",
                xhtml::escape_attr(&id),
                xhtml::escape_attr(&extension)
            ));
        }
    }
    if let Some(files) = body.get("files").and_then(|value| value.as_array()) {
        for file in files {
            let Some(attachment) = read_fanbox_file(file) else {
                continue;
            };
            html.push_str(&format!(
                "<p class=\"file-link\">添付ファイル: {}</p>\n",
                xhtml::escape_text(&attachment.name)
            ));
            attachments.push(attachment);
        }
    }

    let pages = vec![EpubPage {
        title: Some("本文".to_string()),
        html_content: html,
        order: 1,
        chapters,
    }];

    (pages, text_length, attachments)
}

/// `imageMap` から画像 ID ごとの拡張子を引ける表を作る。
fn fanbox_image_extensions(body: &Value) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let mut record = |value: &Value| {
        if let (Some(id), Some(extension)) =
            (read_str(value.get("id")), read_str(value.get("extension")))
        {
            map.insert(id, extension.to_lowercase());
        }
    };
    if let Some(image_map) = body.get("imageMap").and_then(|value| value.as_object()) {
        for image in image_map.values() {
            record(image);
        }
    }
    if let Some(images) = body.get("images").and_then(|value| value.as_array()) {
        for image in images {
            record(image);
        }
    }
    map
}

fn fanbox_file(body: &Value, file_id: &str) -> Option<EpubAttachment> {
    body.get("fileMap")
        .and_then(|value| value.get(file_id))
        .and_then(read_fanbox_file)
}

fn read_fanbox_file(file: &Value) -> Option<EpubAttachment> {
    let name = read_str(file.get("name"))?;
    let extension = read_str(file.get("extension"));
    Some(EpubAttachment {
        name: match &extension {
            Some(extension) => format!("{}.{}", name, extension),
            None => name,
        },
        extension,
        size_bytes: file.get("size").and_then(|value| value.as_u64()),
    })
}

/// 埋め込みの表示名とリンク先。生の埋め込み HTML は本文に持ち込まない。
fn fanbox_embed(body: &Value, embed_id: &str) -> Option<(String, String)> {
    let embed = body
        .get("urlEmbedMap")
        .and_then(|value| value.get(embed_id))?;
    if let Some(url) = read_str(embed.get("url")) {
        let label = read_str(embed.get("host")).unwrap_or_else(|| url.clone());
        return Some((label, url));
    }
    // FANBOX の投稿を指す埋め込みは URL を持たず、投稿情報だけを持つ。
    let post = embed.get("postInfo")?;
    let post_id = read_id(post.get("id"))?;
    let creator = read_str(post.get("creatorId"))?;
    let title = read_str(post.get("title")).unwrap_or_else(|| post_id.clone());
    Some((
        title,
        format!("https://{}.fanbox.cc/posts/{}", creator, post_id),
    ))
}

/// FANBOX の装飾指定（太字とリンク）を本文へ適用する。
///
/// 位置と長さは JavaScript の文字列、つまり UTF-16 の符号単位で数えられている。
/// Rust のバイト位置や文字数で切ると、絵文字やサロゲートを含む行がずれる。
fn decorate_fanbox_text(text: &str, block: &Value) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let mut opens: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut closes: BTreeMap<usize, Vec<String>> = BTreeMap::new();

    let mut add = |offset: usize, length: usize, open: String, close: String| {
        let end = (offset + length).min(units.len());
        if offset >= units.len() || end <= offset {
            return;
        }
        opens.entry(offset).or_default().push(open);
        // 同じ位置で閉じるものは後から開いたものから閉じる。
        closes.entry(end).or_default().insert(0, close);
    };

    if let Some(styles) = block.get("styles").and_then(|value| value.as_array()) {
        for style in styles {
            let (Some(offset), Some(length)) = (
                read_usize(style.get("offset")),
                read_usize(style.get("length")),
            ) else {
                continue;
            };
            let element = match read_str(style.get("type")).as_deref() {
                Some("bold") => "strong",
                _ => "em",
            };
            add(
                offset,
                length,
                format!("<{}>", element),
                format!("</{}>", element),
            );
        }
    }
    if let Some(links) = block.get("links").and_then(|value| value.as_array()) {
        for link in links {
            let (Some(offset), Some(length), Some(url)) = (
                read_usize(link.get("offset")),
                read_usize(link.get("length")),
                read_str(link.get("url")),
            ) else {
                continue;
            };
            add(
                offset,
                length,
                format!("<a href=\"{}\">", xhtml::escape_attr(&url)),
                "</a>".to_string(),
            );
        }
    }

    if opens.is_empty() && closes.is_empty() {
        return xhtml::escape_text(text);
    }

    let mut out = String::with_capacity(text.len() + 32);
    let mut buffer: Vec<u16> = Vec::new();
    let flush = |buffer: &mut Vec<u16>, out: &mut String| {
        if !buffer.is_empty() {
            out.push_str(&xhtml::escape_text(&String::from_utf16_lossy(buffer)));
            buffer.clear();
        }
    };
    for index in 0..=units.len() {
        if let Some(tags) = closes.get(&index) {
            flush(&mut buffer, &mut out);
            for tag in tags {
                out.push_str(tag);
            }
        }
        if let Some(tags) = opens.get(&index) {
            flush(&mut buffer, &mut out);
            for tag in tags {
                out.push_str(tag);
            }
        }
        if index < units.len() {
            buffer.push(units[index]);
        }
    }
    flush(&mut buffer, &mut out);
    // 重なりあう装飾は入れ子が崩れうるので、最後に整形式へ均す。
    xhtml::sanitize_fragment(&out)
}

// ============================================================
// 共通ヘルパー
// ============================================================

fn read_str(value: Option<&Value>) -> Option<String> {
    value.and_then(|value| value.as_str()).map(str::to_string)
}

/// 数値でも文字列でも来る ID を文字列に揃える。
fn read_id(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_u64().map(|number| number.to_string()))
        .or_else(|| value.as_i64().map(|number| number.to_string()))
        .filter(|text| !text.trim().is_empty())
}

fn read_u32(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(|value| value.as_u64())
        .and_then(|number| u32::try_from(number).ok())
}

fn read_usize(value: Option<&Value>) -> Option<usize> {
    value
        .and_then(|value| value.as_u64())
        .and_then(|number| usize::try_from(number).ok())
}

/// data_assets ディレクトリからカバー画像を検索
fn find_cover_image(assets_dir: &Path) -> Option<EpubImage> {
    let cover_dir = assets_dir.join("cover");
    let entries = std::fs::read_dir(&cover_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(mime) = image_mime(&path) else {
            continue;
        };
        return Some(EpubImage {
            id: "cover-image".to_string(),
            local_path: path.to_string_lossy().to_string(),
            mime_type: mime.to_string(),
            alt_text: Some("表紙".to_string()),
            caption: None,
            width: None,
            height: None,
        });
    }
    None
}

/// data_assets/illustrations ディレクトリから挿絵を検索
fn find_illustrations(assets_dir: &Path) -> Vec<EpubImage> {
    let illust_dir = assets_dir.join("illustrations");
    let Ok(entries) = std::fs::read_dir(&illust_dir) else {
        return Vec::new();
    };

    let mut images: Vec<EpubImage> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            let mime = image_mime(&path)?;
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
            Some(EpubImage {
                id: format!("img-{}", stem),
                local_path: path.to_string_lossy().to_string(),
                mime_type: mime.to_string(),
                alt_text: Some(format!("挿絵 {}", stem)),
                caption: None,
                width: None,
                height: None,
            })
        })
        .collect();

    // 数字を含む名前が並ぶので、桁数を揃えた比較で人が期待する順にする。
    images.sort_by_key(|image| natural_key(&image.id));
    images
}

fn image_mime(path: &Path) -> Option<&'static str> {
    let extension = path.extension().and_then(|e| e.to_str())?.to_lowercase();
    match extension.as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

/// "p10" が "p2" より後ろに来るよう、数字部分を桁数で揃えた並べ替え鍵。
fn natural_key(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    let mut digits = String::new();
    for ch in value.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            continue;
        }
        if !digits.is_empty() {
            out.push_str(&format!("{:0>12}", digits));
            digits.clear();
        }
        out.push(ch);
    }
    if !digits.is_empty() {
        out.push_str(&format!("{:0>12}", digits));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pixiv(text: &str) -> EpubManifest {
        convert_pixiv(
            &json!({
                "text": text,
                "detail": {
                    "id": 42,
                    "title": "夏 & 秋",
                    "caption": "紹介<br>文 & もっと",
                    "create_date": "2018-08-21T21:52:01+09:00",
                    "text_length": 1234,
                    "series_id": 900,
                    "series_title": "連作",
                    "tags": [{"name": "R-18", "translated_name": "R-18"}, {"name": "百合"}],
                    "user": {"id": 7, "name": "作者", "account": "acct"},
                },
            }),
            Path::new("/nonexistent"),
        )
    }

    /// 読書画面と EPUB が、同じ本文を同じ構造として読むこと。
    ///
    /// 片方が正規表現でもう片方が完全一致だったので、同じ作品が「読むとき」と
    /// 「本にするとき」で別の形になっていた。
    #[test]
    fn the_page_break_is_recognised_the_same_way_the_reader_recognises_it() {
        for text in [
            "前<br>[newpage]<br>後",
            "前
[newpage]
後",
            "前
[[newpage]]
後",
            "前
[NewPage]
後",
            "前
[newpage ]
後",
        ] {
            let pages = pixiv(text).content.pages;
            assert_eq!(pages.len(), 2, "改ページを見落としている: {text:?}");
        }
    }

    #[test]
    fn a_page_jump_becomes_a_link_the_reader_can_follow() {
        let manifest = pixiv(
            "最初
[jump:2]
[newpage]
二ページ目",
        );
        let first = &manifest.content.pages[0].html_content;
        assert!(
            first.contains(r#"<a href="page_002.xhtml">2ページへ</a>"#),
            "ページ内移動が字面のまま残っている: {first}"
        );
    }

    #[test]
    fn a_jump_with_no_destination_stays_as_plain_text() {
        // 行き止まりのリンクを作るくらいなら、字面のほうがまだ正直。
        let manifest = pixiv(
            "本文
[jump:9]",
        );
        assert!(!manifest.content.pages[0]
            .html_content
            .contains("<a href=\"page_"));
    }

    #[test]
    fn a_pixiv_app_address_becomes_one_anybody_can_open() {
        let manifest = pixiv("[[jumpuri:前編 > pixiv://novels/12345]]");
        let html = &manifest.content.pages[0].html_content;
        assert!(
            html.contains("https://www.pixiv.net/novel/show.php?id=12345"),
            "アプリ内アドレスが本の中に残っている: {html}"
        );
    }

    #[test]
    fn pixiv_series_is_read_from_the_shape_the_library_actually_stores() {
        // 保存済みデータは detail.series_id / series_title を持つ。以前はこの形を
        // 見ておらず、すべての pixiv 作品でシリーズが落ちていた。
        let series = pixiv("本文").core.is_part_of.expect("series");
        assert_eq!(series.name, "連作");
        assert_eq!(series.id.as_deref(), Some("900"));
        assert_eq!(
            series.url.as_deref(),
            Some("https://www.pixiv.net/novel/series/900")
        );
    }

    #[test]
    fn pixiv_metadata_is_escaped_and_normalized() {
        let manifest = pixiv("本文");
        assert_eq!(manifest.core.id_, "urn:pixiv:novel:42");
        assert_eq!(
            manifest.core.description.as_deref(),
            Some("紹介<br />文 &amp; もっと")
        );
        assert_eq!(
            manifest.core.description_text.as_deref(),
            Some("紹介 文 & もっと")
        );
        assert_eq!(manifest.core.keywords, vec!["R-18", "百合"]);
        assert_eq!(manifest.stats.text_length, 1234);
        assert!(manifest.stats.adult);
        assert_eq!(
            manifest.core.author.url.as_deref(),
            Some("https://www.pixiv.net/users/7")
        );
    }

    #[test]
    fn pixiv_notation_becomes_well_formed_xhtml() {
        let manifest = pixiv("[chapter:第一章]\n本文 & 続き\n[[rb:漢字 > かんじ]]\n[[jumpuri:リンク > https://example.com]]");
        let page = &manifest.content.pages[0];
        assert!(page
            .html_content
            .contains("<h2 id=\"chapter-001\">第一章</h2>"));
        assert!(page.html_content.contains("本文 &amp; 続き"));
        assert!(page
            .html_content
            .contains("<ruby>漢字<rp>(</rp><rt>かんじ</rt><rp>)</rp></ruby>"));
        assert!(page
            .html_content
            .contains("<a href=\"https://example.com\">リンク</a>"));
        assert_eq!(page.title.as_deref(), Some("第一章"));
        assert_eq!(page.chapters.len(), 1);
    }

    #[test]
    fn pixiv_pages_split_and_illustrations_point_at_page_indexed_files() {
        let manifest = pixiv("一枚目\n[pixivimage:555-2]\n[newpage]\n二枚目");
        assert_eq!(manifest.content.pages.len(), 2);
        assert!(manifest.content.pages[0]
            .html_content
            .contains("src=\"../images/555_p1.jpg\""));
        assert_eq!(manifest.stats.page_count, 2);
    }

    #[test]
    fn fanbox_image_blocks_use_the_extension_the_post_declares() {
        // 拡張子を jpg と決め打つと、png の投稿で参照先の無い EPUB ができる。
        let manifest = convert_fanbox(
            &json!({
                "id": "77", "title": "投稿", "creatorId": "someone", "type": "article",
                "publishedDatetime": "2024-03-04T05:06:07+09:00",
                "excerpt": "ようやく",
                "likeCount": 12, "commentCount": 3, "feeRequired": 500, "hasAdultContent": true,
                "user": {"userId": "9", "name": "作者"},
                "tags": ["日記"],
                "body": {
                    "blocks": [
                        {"type": "header", "text": "見出し"},
                        {"type": "p", "text": "本文 & 続き"},
                        {"type": "image", "imageId": "abc"},
                    ],
                    "imageMap": {"abc": {"id": "abc", "extension": "png"}},
                },
            }),
            Path::new("/nonexistent"),
        );
        let page = &manifest.content.pages[0];
        assert!(page.html_content.contains("src=\"../images/abc.png\""));
        assert!(page.html_content.contains("<p>本文 &amp; 続き</p>"));
        assert_eq!(page.chapters.len(), 1);
        assert_eq!(manifest.stats.fee_required, Some(500));
        assert_eq!(manifest.stats.like_count, Some(12));
        assert!(manifest.stats.adult);
        assert_eq!(manifest.core.description_text.as_deref(), Some("ようやく"));
    }

    #[test]
    fn fanbox_image_posts_carry_their_images_even_without_blocks() {
        let manifest = convert_fanbox(
            &json!({
                "body": { "post": {
                    "id": "1", "title": "画像投稿", "creatorId": "c", "type": "image",
                    "user": {"userId": "9", "name": "作者"},
                    "body": {"images": [{"id": "one", "extension": "png"}], "text": "ひとこと"}
                }}
            }),
            Path::new("/nonexistent"),
        );
        let html = &manifest.content.pages[0].html_content;
        assert!(html.contains("src=\"../images/one.png\""));
        assert!(html.contains("ひとこと"));
    }

    #[test]
    fn fanbox_decorations_are_placed_by_utf16_offsets() {
        // 絵文字を挟むと、文字数で切った装飾は位置がずれる。
        let block = json!({
            "type": "p", "text": "🙂太字です",
            "styles": [{"offset": 2, "length": 2, "type": "bold"}],
            "links": [{"offset": 4, "length": 2, "url": "https://example.com"}],
        });
        let rendered = decorate_fanbox_text("🙂太字です", &block);
        assert_eq!(
            rendered,
            "🙂<strong>太字</strong><a href=\"https://example.com\">です</a>"
        );
    }

    #[test]
    fn fanbox_attachments_and_embeds_are_named_not_inlined() {
        let manifest = convert_fanbox(
            &json!({
                "id": "1", "title": "投稿", "creatorId": "c", "type": "article",
                "user": {"userId": "9", "name": "作者"},
                "body": {
                    "blocks": [
                        {"type": "file", "fileId": "f1"},
                        {"type": "url_embed", "urlEmbedId": "e1"},
                    ],
                    "fileMap": {"f1": {"id": "f1", "name": "資料", "extension": "zip", "size": 10}},
                    "urlEmbedMap": {"e1": {"id": "e1", "type": "html", "html": "<iframe src=\"https://x\"></iframe>", "url": "https://example.com", "host": "example.com"}},
                },
            }),
            Path::new("/nonexistent"),
        );
        let html = &manifest.content.pages[0].html_content;
        assert!(html.contains("添付ファイル: 資料.zip"));
        assert!(html.contains("<a href=\"https://example.com\">example.com</a>"));
        // 埋め込みの生 HTML は iframe を含む。EPUB には持ち込まない。
        assert!(!html.contains("iframe"));
        assert_eq!(manifest.content.attachments.len(), 1);
    }

    #[test]
    fn illustrations_sort_the_way_a_reader_expects() {
        assert!(natural_key("img-123_p2") < natural_key("img-123_p10"));
    }
}
