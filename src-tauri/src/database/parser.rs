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
pub(crate) fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// リンク先として置いてよい URL だけを返す。
///
/// 取得元の JSON に入っていた文字列を、そのまま `href` にしていた。
/// `escape_html` は `<` や `"` を潰すだけで**スキームは見ない**ので、
/// `javascript:` が押せるリンクとして読書画面に並ぶ。EPUB 側
/// （`epub::xhtml::sanitize_uri`）は最初からこれを弾いており、画面側だけが
/// 素通しだった。取り込んだ書庫を復元する経路では、中身は他人が書いたもので
/// ありうる。
///
/// 通すのは http / https と、アプリが自分で組み立てる `#` だけ。
pub(crate) fn safe_link_href(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 制御文字（改行やタブ）を途中に挟んでスキームを隠す逃げ道を、先に潰す。
    let compact: String = trimmed.chars().filter(|c| !c.is_control()).collect();
    let lowered = compact.to_ascii_lowercase();
    if lowered.starts_with("http://") || lowered.starts_with("https://") {
        Some(escape_html(&compact))
    } else {
        None
    }
}

/// 埋め込みの `contentId` として置いてよい形。
///
/// 取得元の値をそのまま URL へ差し込むので、経路を書き換えられる文字は
/// 通さない。空白や `<` が混ざったものは、そもそも埋め込みの id ではない。
pub(crate) fn safe_embed_content_id(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 256 {
        return None;
    }
    let allowed = trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_.~:/?#[]@!$&'()*+,;=%".contains(c));
    allowed.then_some(trimmed)
}

/// FANBOX の `embed` ブロックが指す先を、公開 URL へ直す。
///
/// FANBOX は埋め込みを「提供元の名前 + その中での id」で持っている。URL は
/// 保存された JSON のどこにも書かれていないので、ここで組み直さないと
/// **本文に貼られた動画や音源へ辿る手立てが無くなる**。
pub(crate) fn embed_service_url(provider: &str, content_id: &str) -> Option<String> {
    let id = safe_embed_content_id(content_id)?;
    // 取得元が完全な URL を入れてくることもある。そのときは組み立てない。
    let lowered_id = id.to_ascii_lowercase();
    if lowered_id.starts_with("http://") || lowered_id.starts_with("https://") {
        return Some(id.to_string());
    }
    let url = match provider.trim().to_ascii_lowercase().as_str() {
        "youtube" => format!("https://www.youtube.com/watch?v={id}"),
        "vimeo" => format!("https://vimeo.com/{id}"),
        "soundcloud" => format!("https://soundcloud.com/{id}"),
        "twitter" => format!("https://twitter.com/i/web/status/{id}"),
        "gist" => format!("https://gist.github.com/{id}"),
        "google_forms" => format!("https://docs.google.com/forms/d/e/{id}/viewform"),
        _ => return None,
    };
    Some(url)
}

/// 埋め込みの提供元を、読み手に見せる名前へ。
pub(crate) fn embed_service_label(provider: &str) -> String {
    match provider.trim().to_ascii_lowercase().as_str() {
        "youtube" => "YouTube".to_string(),
        "vimeo" => "Vimeo".to_string(),
        "soundcloud" => "SoundCloud".to_string(),
        "twitter" => "X (Twitter)".to_string(),
        "gist" => "GitHub Gist".to_string(),
        "google_forms" => "Google フォーム".to_string(),
        "" => "埋め込み".to_string(),
        // 知らない提供元は、名乗っている名前をそのまま見せる。長すぎるものは
        // 詰める ―― 取得元の文字列をそのまま札にするので、際限を持たせない。
        other => other.replace('_', " ").chars().take(40).collect(),
    }
}

static HTML_EMBED_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)(href|src)\s*=\s*["'](https?://[^"'\s]+)["']"#).unwrap());

/// 埋め込みの中継先。ここを指しても、読み手の役には立たない。
const EMBED_RELAY_HOSTS: [&str; 5] = [
    "iframely",
    "embedly",
    "embed.ly",
    "cdn.iframe.ly",
    "if-cdn.com",
];

/// 埋め込みの HTML から、人が辿れる URL を1つ取り出す。
///
/// URL を持たず HTML だけを返す埋め込みがある。カードを組めないからと
/// 捨てていたが、その HTML の中には元のページの宛先が書いてある。
pub(crate) fn first_public_url_in_html(html: &str) -> Option<String> {
    let mut from_src: Option<String> = None;
    for captures in HTML_EMBED_URL.captures_iter(html) {
        let raw = captures.get(2)?.as_str();
        let url = raw
            .replace("&amp;", "&")
            .replace("&#39;", "'")
            .replace("&quot;", "\"");
        let lowered = url.to_ascii_lowercase();
        if EMBED_RELAY_HOSTS.iter().any(|host| lowered.contains(host)) {
            continue;
        }
        if captures
            .get(1)
            .is_some_and(|kind| kind.as_str().eq_ignore_ascii_case("href"))
        {
            return Some(url);
        }
        from_src.get_or_insert(url);
    }
    from_src
}

/// 本文に置くリンクカード1枚。
///
/// 宛先を確かめられたものだけが押せるカードになり、確かめられないものは
/// 同じ姿のまま押せない札になる。**消してしまうと、そこに何かが貼られて
/// いた事実ごと失われる。**
fn link_card_html(
    href: Option<&str>,
    provider: &str,
    kicker: &str,
    title: &str,
    meta: &str,
) -> String {
    let brand = match provider {
        "fanbox" => "F",
        _ => "↗",
    };
    let body = format!(
        r##"<span class="link-card-brand">{}</span>
                                <span class="link-card-info">
                                    <span class="link-card-kicker">{}</span>
                                    <span class="link-card-title">{}</span>
                                    <span class="link-card-host">{}</span>
                                </span>"##,
        brand,
        escape_html(kicker),
        escape_html(title),
        escape_html(meta)
    );
    let card_class = if provider == "fanbox" {
        "novel-link-card novel-link-card--fanbox"
    } else {
        "novel-link-card"
    };
    match href {
        Some(href) => format!(
            r##"<a href="{href}" target="_blank" rel="noopener noreferrer" class="{card_class}" data-provider="{provider}">
                                {body}
                                <span class="link-card-arrow">↗</span>
                            </a>"##
        ),
        None => format!(
            r##"<div class="{card_class} novel-link-card--plain" data-provider="{provider}">
                                {body}
                            </div>"##
        ),
    }
}

/// 本文を pixiv の改ページで割る。
///
/// 読書画面と EPUB が**同じ見分け方**を使うための一点。片方が正規表現で
/// もう片方が完全一致だったころは、`[[newpage]]` と書かれた作品が、読むと
/// ページが分かれ、本にすると 1 ページに融合していた。
pub(crate) fn split_pixiv_pages(text: &str) -> Vec<&str> {
    RE_NEWPAGE.split(text).collect()
}

/// 編集画面で足した区切り線を、本文の記法として表す印。
///
/// pixiv 本来の記法には区切り線が無いので、取り込んだ本文には現れない。
/// 反映済みの編集を EPUB へ渡すときだけ使う。
pub(crate) const EDITOR_SEPARATOR_NOTATION: &str = "[piep:separator]";

/// 行内記法 ―― ルビ・ページ内移動・外部リンク ―― を HTML へ開く。
///
/// 取り込みと、編集した本文の書き戻しの**両方**がここを通る。編集画面が扱う
/// のは記法のままの文字列なので、書いたルビはそのまま読書画面と EPUB に効く。
/// 開くだけの片道だったころは、編集画面を一度開くと `[[rb:漢字>かんじ]]` が
/// 「漢字かんじ」という地の文になり、外部リンクは宛先を失っていた。
///
/// 受け取るのは **エスケープ済み** の文字列。記法の `>` は `&gt;` になって
/// いるので、正規表現もそちらでマッチする。
pub(crate) fn expand_pixiv_inline_notation(escaped_html: &str) -> String {
    let html = RE_RUBY.replace_all(escaped_html, "<ruby>$kanji<rt>$kana</rt></ruby>");
    let html = RE_JUMP.replace_all(
        &html,
        r##"<a href="#" class="jump-link" data-page="$page">$pageページへ</a>"##,
    );
    RE_JUMPURI
        .replace_all(
            &html,
            r##"<a href="$url" target="_blank" rel="noopener noreferrer">$title</a>"##,
        )
        .to_string()
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

    // 2. 行内記法（ルビ・ページ内移動・外部リンク）を開く
    html = expand_pixiv_inline_notation(&html);

    // 3. 見出し [chapter:タイトル] -> <h2>タイトル</h2>
    html = RE_CHAPTER.replace_all(&html, "<h2>$title</h2>").to_string();

    // 3.5 改ページ [newpage] -> HTMLコメント <!-- newpage --> (大カッコ一重・二重、大文字小文字、スペース完全許容)
    html = RE_NEWPAGE
        .replace_all(&html, "<!-- newpage -->")
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

    // 保存元やAPI世代によって異なるラッパーを共通の投稿形へ寄せる。
    let post = crate::fanbox_api::payload::post_or_self(&v);

    // 2. 古いプレーンテキスト形式 (PostBodyText) の場合
    //
    // 空文字の `text` を持ったまま `body.blocks` に本文がある形もある。中身の
    // 無い `text` でここを抜けると、**投稿全体が空になる**。
    if let Some(text) = post
        .get("text")
        .and_then(|t| t.as_str())
        .filter(|text| !text.trim().is_empty())
    {
        let escaped = escape_html(text);
        return escaped.replace("\n", "<br />\n");
    }

    let body = match post.get("body") {
        Some(b) => b,
        None => return String::new(),
    };

    // image/file 投稿は text と配列を併せ持つ。text だけを返してしまうと、
    // 正常に保存済みの画像・添付が閲覧画面から消える。
    let Some(blocks) = body.get("blocks").and_then(|b| b.as_array()) else {
        let mut parts = Vec::new();
        if let Some(text) = body.get("text").and_then(|t| t.as_str()) {
            let escaped = escape_html(text).replace("\n", "<br />\n");
            if !escaped.trim().is_empty() {
                parts.push(escaped);
            }
        }
        if let Some(images) = body.get("images").and_then(|value| value.as_array()) {
            for image in images {
                let image_id = image
                    .get("id")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let found = assets.iter().find(|asset| {
                    (!image_id.is_empty()
                        && (asset.filename.contains(image_id)
                            || asset.local_path.contains(image_id)))
                        || image
                            .get("localPath")
                            .and_then(|value| value.as_str())
                            .is_some_and(|path| {
                                asset.local_path.ends_with(path.trim_start_matches("./"))
                            })
                });
                parts.push(match found {
                    Some(asset) => format!(
                        r#"<img class="novel-image" data-local-path="{}" alt="image_{}" />"#,
                        escape_html(&asset.local_path),
                        escape_html(image_id)
                    ),
                    None => format!(
                        r#"<div class="missing-image-placeholder">画像 {}(見つかりません)</div>"#,
                        escape_html(image_id)
                    ),
                });
            }
        }
        if let Some(files) = body.get("files").and_then(|value| value.as_array()) {
            for file in files {
                let id = file
                    .get("id")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let name = file
                    .get("name")
                    .and_then(|value| value.as_str())
                    .unwrap_or("添付ファイル");
                let extension = file
                    .get("extension")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let full_name = if extension.is_empty() {
                    name.to_string()
                } else {
                    format!("{name}.{extension}")
                };
                let size = file
                    .get("size")
                    .and_then(|value| value.as_i64())
                    .unwrap_or(0);
                let found = assets.iter().find(|asset| {
                    asset.filename
                        == crate::downloader::asset_downloader::sanitize_filename(&full_name)
                        || (!id.is_empty() && asset.local_path.contains(id))
                });
                parts.push(match found {
                    Some(asset) => format!(
                        r##"<div class="novel-file-attachment clickable-file" data-local-path="{}">
                            <span class="file-icon">📎</span>
                            <div class="file-info">
                                <span class="file-name">{}</span>
                                <span class="file-size">{:.1} KB</span>
                            </div>
                        </div>"##,
                        escape_html(&asset.local_path),
                        escape_html(&full_name),
                        (size as f64) / 1024.0
                    ),
                    None => format!(
                        r#"<div class="missing-file-placeholder">ファイル {}(見つかりません)</div>"#,
                        escape_html(&full_name)
                    ),
                });
            }
        }
        return parts.join("\n");
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
                // `imageId` は応答の文字列そのまま。pixiv 側と違って数字とは
                // 限らないので、他の値と同じように通してから埋める。
                let safe_image_id = escape_html(image_id);
                if let Some(asset) = found_asset {
                    format!(
                        r#"<img class="novel-image" data-local-path="{}" alt="image_{}" />"#,
                        escape_html(&asset.local_path),
                        safe_image_id
                    )
                } else {
                    format!(
                        r#"<div class="missing-image-placeholder">画像 {}(見つかりません)</div>"#,
                        safe_image_id
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
                        match info.get("type").and_then(|t| t.as_str()) {
                            Some("fanbox.post") => {
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
                            // 作者を指す埋め込み。手元にその作者がいれば、
                            // アプリの中の作者ページへ入っていける。
                            Some("fanbox.creator") => {
                                let creator_id = info
                                    .get("profile")
                                    .and_then(|profile| profile.get("creatorId"))
                                    .or_else(|| info.get("creatorId"))
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("");
                                if !creator_id.is_empty() {
                                    url = format!("https://{creator_id}.fanbox.cc");
                                }
                            }
                            _ => {}
                        }
                    }

                    // URL を持たず HTML だけを返す埋め込みがある。その HTML の
                    // 中には元のページの宛先が書いてあるので、そこから拾う。
                    if url.trim().is_empty() {
                        if let Some(html) = info.get("html").and_then(|value| value.as_str()) {
                            if let Some(found) = first_public_url_in_html(html) {
                                url = found;
                            }
                        }
                    }

                    // FANBOX sometimes returns the creator-only management URL in an
                    // embed. Saved documents must always point at the public post URL.
                    if url.contains(".fanbox.cc/manage/posts/") {
                        url = url.replace(".fanbox.cc/manage/posts/", ".fanbox.cc/posts/");
                    }

                    // 通せない相手は押せるリンクにしない。ただし**カードごと
                    // 消しはしない** ―― そこに何かが貼られていた事実まで
                    // 失われると、読み手は本文が欠けたことにも気づけない。
                    let safe_url = safe_link_href(&url);
                    let host = info
                        .get("host")
                        .and_then(|h| h.as_str())
                        .filter(|value| !value.trim().is_empty())
                        .map(|s| s.to_string())
                        .or_else(|| {
                            url::Url::parse(&url)
                                .ok()
                                .and_then(|u| u.host_str().map(|h| h.to_string()))
                        })
                        .unwrap_or_else(|| "外部リンク".to_string());
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
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or_else(|| {
                            if url.trim().is_empty() {
                                "リンク先を取り出せない埋め込み".to_string()
                            } else {
                                url.clone()
                            }
                        });

                    let is_fanbox = host == "fanbox.cc"
                        || host.ends_with(".fanbox.cc")
                        || info.get("type").and_then(|value| value.as_str()) == Some("fanbox.post");
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
                    link_card_html(
                        safe_url.as_deref(),
                        if is_fanbox { "fanbox" } else { "web" },
                        if is_fanbox {
                            "pixivFANBOX"
                        } else {
                            "外部リンク"
                        },
                        &title,
                        &meta,
                    )
                } else {
                    String::new()
                }
            }
            // 動画・音源・フォームの埋め込み。URL は保存された JSON に書かれて
            // いないので、提供元と id から組み直す。組み直せない提供元でも、
            // 何が貼られていたかは残す。
            "embed" => {
                let embed_id = block
                    .get("embedId")
                    .and_then(|id| id.as_str())
                    .unwrap_or("");
                let embed_info = body
                    .get("embedMap")
                    .or_else(|| body.get("embed_map"))
                    .or_else(|| post.get("embedMap"))
                    .or_else(|| post.get("embed_map"))
                    .and_then(|m| m.get(embed_id));

                match embed_info {
                    Some(info) => {
                        let provider = info
                            .get("serviceProvider")
                            .or_else(|| info.get("service_provider"))
                            .and_then(|value| value.as_str())
                            .unwrap_or("");
                        let content_id = info
                            .get("contentId")
                            .or_else(|| info.get("content_id"))
                            .and_then(|value| value.as_str())
                            .unwrap_or("");
                        let url = embed_service_url(provider, content_id);
                        let safe_url = url.as_deref().and_then(safe_link_href);
                        let label = embed_service_label(provider);
                        let title = match &url {
                            Some(url) => url.clone(),
                            None if content_id.trim().is_empty() => label.clone(),
                            None => content_id.trim().to_string(),
                        };
                        link_card_html(safe_url.as_deref(), "web", &label, &title, "埋め込み")
                    }
                    None => String::new(),
                }
            }
            _ => {
                // 知らないブロックでも、文字を持っているなら本文として出す。
                // 「表示できません」の札だけを置いていたころは、取得元が種類を
                // 増やすたびに、読める本文が札に置き換わっていた。
                if block
                    .get("text")
                    .and_then(|value| value.as_str())
                    .is_some_and(|text| !text.trim().is_empty())
                {
                    parse_fanbox_paragraph(block)
                } else {
                    format!(
                        r#"<div class="fallback-block">この形式のブロック（{}）は表示できません。</div>"#,
                        escape_html(block_type)
                    )
                }
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

    // FANBOX の `offset` / `length` は JS の文字列の添字、つまり **UTF-16 の
    // 単位** である。Rust の `char` はコードポイントなので、絵文字のように
    // UTF-16 で2つぶんを占める文字が混ざると、そこから先が1文字ずつずれる。
    // ずれても例外にはならないぶん、太字やリンクが隣の文字に掛かった本文が
    // 黙って保存されていた。
    //
    // 文字ごとの UTF-16 位置を先に並べておき、取得元の添字はここを引いて
    // `chars` の位置へ直す。
    let mut utf16_positions: Vec<usize> = Vec::with_capacity(chars.len() + 1);
    let mut utf16_len = 0usize;
    for character in &chars {
        utf16_positions.push(utf16_len);
        utf16_len += character.len_utf16();
    }
    utf16_positions.push(utf16_len);
    let position_of = |utf16_offset: usize| -> usize {
        // 本文の外へ出た位置は末尾へ寄せる。捨てると閉じタグだけが消え、
        // 開いたままのタグが以降の本文を飲み込む。
        let bounded = utf16_offset.min(utf16_len);
        match utf16_positions.binary_search(&bounded) {
            Ok(index) => index,
            // サロゲートの途中を指していたら、その文字の頭へ寄せる。
            Err(index) => index.saturating_sub(1),
        }
    };

    let mut inserts_map: Vec<Vec<String>> = vec![Vec::new(); chars.len() + 1];
    let mut all_tags = Vec::new();

    if let Some(styles) = block.get("styles").and_then(|s| s.as_array()) {
        for style in styles {
            let style_type = style.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let offset = style.get("offset").and_then(|o| o.as_u64()).unwrap_or(0) as usize;
            let length = style.get("length").and_then(|l| l.as_u64()).unwrap_or(0) as usize;

            if style_type == "bold" {
                all_tags.push((position_of(offset), "<b>".to_string(), true));
                all_tags.push((
                    position_of(offset.saturating_add(length)),
                    "</b>".to_string(),
                    false,
                ));
            }
        }
    }

    if let Some(links) = block.get("links").and_then(|l| l.as_array()) {
        for link in links {
            let url = link.get("url").and_then(|u| u.as_str()).unwrap_or("");
            let offset = link.get("offset").and_then(|o| o.as_u64()).unwrap_or(0) as usize;
            let length = link.get("length").and_then(|l| l.as_u64()).unwrap_or(0) as usize;

            // 通せない相手はリンクにしない。文字は本文に残る。
            let Some(safe_url) = safe_link_href(url) else {
                continue;
            };
            all_tags.push((
                position_of(offset),
                format!(
                    r#"<a href="{}" target="_blank" rel="noopener noreferrer">"#,
                    safe_url
                ),
                true,
            ));
            all_tags.push((
                position_of(offset.saturating_add(length)),
                "</a>".to_string(),
                false,
            ));
        }
    }

    all_tags.sort_by(|a, b| {
        if a.0 != b.0 {
            a.0.cmp(&b.0)
        } else {
            a.2.cmp(&b.2)
        }
    });

    // `position_of` が必ず `0..=chars.len()` へ収めているので、ここで落とす
    // ものは無い。範囲外だからと捨てていたのが、閉じないタグの原因だった。
    for (offset, tag, _) in all_tags {
        inserts_map[offset].push(tag);
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
    use super::safe_link_href;

    /// 本文のリンク先は取得元から来る。書庫を復元した棚では、それを書いたのは
    /// 他人でありうる。`escape_html` は `<` や `"` を潰すだけでスキームを見ない。
    #[test]
    fn only_http_and_https_become_links() {
        assert!(safe_link_href("https://www.pixiv.net/novel/show.php?id=1").is_some());
        assert!(safe_link_href("http://example.com/").is_some());
        assert!(safe_link_href("HTTPS://EXAMPLE.COM/").is_some());

        assert_eq!(safe_link_href("javascript:alert(1)"), None);
        assert_eq!(safe_link_href("data:text/html,<script>x</script>"), None);
        assert_eq!(safe_link_href("vbscript:msgbox"), None);
        assert_eq!(safe_link_href("file:///C:/Windows/system32"), None);
        assert_eq!(safe_link_href(""), None);
        assert_eq!(safe_link_href("   "), None);
    }

    /// 制御文字を挟んでスキームを隠す形も通さない。
    #[test]
    fn control_characters_cannot_hide_the_scheme() {
        assert_eq!(safe_link_href("java\nscript:alert(1)"), None);
        assert_eq!(safe_link_href("java\tscript:alert(1)"), None);
        // 折り返しの入った本物の URL は、詰めたうえで通す。
        assert_eq!(
            safe_link_href("https://example.com/\u{0001}a").as_deref(),
            Some("https://example.com/a")
        );
    }

    /// 通した URL も、属性値として安全な形にしてから返す。
    #[test]
    fn a_passed_url_is_still_escaped_for_the_attribute() {
        let href = safe_link_href("https://example.com/?a=1&b=\"x\"").expect("http is allowed");
        assert!(!href.contains('"'));
        assert!(href.contains("&amp;"));
    }

    use super::*;

    fn fanbox_block(block: serde_json::Value) -> String {
        let json = serde_json::json!({ "body": { "blocks": [block] } });
        parse_fanbox_to_html(&json.to_string(), &[])
    }

    /// FANBOX の `offset` / `length` は JS の文字列の添字、つまり UTF-16 の
    /// 単位である。Rust の `char` はコードポイントなので、絵文字のように
    /// UTF-16 で2つぶんを占める文字が混ざると、そこから先が1文字ずつずれる。
    ///
    /// ずれても例外にはならないので、**太字やリンクが隣の文字に掛かった本文**
    /// が黙って保存される。
    #[test]
    fn fanbox_styles_are_placed_by_utf16_offsets() {
        // 😀 は UTF-16 で2つぶん。offset 2 は「あ」を指す。
        let html = fanbox_block(serde_json::json!({
            "type": "p",
            "text": "😀あいうえお",
            "styles": [{ "type": "bold", "offset": 2, "length": 3 }]
        }));
        assert!(
            html.contains("<b>あいう</b>"),
            "太字が UTF-16 の位置に置かれていない: {html}"
        );
    }

    #[test]
    fn fanbox_links_are_placed_by_utf16_offsets() {
        let html = fanbox_block(serde_json::json!({
            "type": "p",
            "text": "😀あいうえお",
            "links": [{ "url": "https://example.com/", "offset": 2, "length": 3 }]
        }));
        assert!(
            html.contains(">あいう</a>"),
            "リンクが UTF-16 の位置に置かれていない: {html}"
        );
    }

    /// 範囲が本文の外へ出ていても、開いたタグは必ず閉じる。
    ///
    /// 閉じるほうだけを捨てていたころは、`<b>` や `<a>` が閉じないまま以降の
    /// 本文をすべて飲み込んでいた。取得元が長さを多めに寄越すことは実際に
    /// あるので、ここは落とさずに末尾へ寄せる。
    #[test]
    fn a_style_running_past_the_end_still_closes() {
        let html = fanbox_block(serde_json::json!({
            "type": "p",
            "text": "あいう",
            "styles": [{ "type": "bold", "offset": 0, "length": 99 }]
        }));
        assert!(html.contains("<b>あいう</b>"), "閉じていない: {html}");
    }

    #[test]
    fn a_link_running_past_the_end_still_closes() {
        let html = fanbox_block(serde_json::json!({
            "type": "p",
            "text": "あいう",
            "links": [{ "url": "https://example.com/", "offset": 0, "length": 99 }]
        }));
        assert!(html.contains("</a>"), "閉じていない: {html}");
    }

    /// 取得元から来た値は、どれも同じように通してから組む。
    ///
    /// pixiv の画像IDは正規表現が数字だけに限っているが、FANBOX の `imageId` は
    /// 応答の文字列そのままである。ここだけ素で埋めていたので、`"` や `<` を
    /// 含む値が来ると**組み立てた HTML のほうが壊れる**。取り込んだ書庫を
    /// 復元する経路もあるので、値の出どころは取得元だけとは限らない。
    #[test]
    fn a_fanbox_image_id_cannot_break_out_of_the_markup() {
        let html = fanbox_block(serde_json::json!({
            "type": "image",
            "imageId": "a\"><script>alert(1)</script>"
        }));
        assert!(!html.contains("<script>"), "生のタグが残っている: {html}");
        assert!(!html.contains("\"><"), "属性から抜け出せている: {html}");
    }

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

    /// iframely の HTML しか持たない埋め込みは、安全なリンクへ変換できない。
    /// そこで投稿全体を空にすると、その前後にある正常な本文まで読めなくなる。
    #[test]
    fn an_unrenderable_embed_does_not_erase_the_rest_of_a_fanbox_post() {
        let json = serde_json::json!({
            "id": "12329460",
            "title": "活動報告",
            "body": {
                "blocks": [
                    { "type": "p", "text": "埋め込みより前" },
                    { "type": "url_embed", "urlEmbedId": "card" },
                    { "type": "p", "text": "埋め込みより後" }
                ],
                "urlEmbedMap": {
                    "card": {
                        "type": "html.card",
                        "html": "<iframe src=\"https://iframely.net/example\"></iframe>"
                    }
                }
            }
        });

        let html = parse_fanbox_to_html(&json.to_string(), &[]);
        assert!(
            html.contains("埋め込みより前"),
            "前の本文が消えている: {html}"
        );
        assert!(
            html.contains("埋め込みより後"),
            "後の本文が消えている: {html}"
        );
        assert!(!html.contains("iframe"));
        // 何が貼られていたかの札は残す。跡形もなく消すと、本文が欠けたことに
        // 読み手が気づけない。
        assert!(html.contains("novel-link-card"), "札まで消えている: {html}");
    }

    /// URL を持たず HTML だけを返す埋め込みでも、その HTML に本物の宛先が
    /// 書いてあることがある。中継先ではなく、そちらを拾う。
    #[test]
    fn an_html_only_embed_keeps_the_address_written_inside_it() {
        let json = serde_json::json!({
            "id": "1",
            "title": "紹介",
            "body": {
                "blocks": [{ "type": "url_embed", "urlEmbedId": "card" }],
                "urlEmbedMap": {
                    "card": {
                        "type": "html.card",
                        "html": "<div class=\"iframely-embed\"><a href=\"https://example.com/article/1\">記事</a><iframe src=\"https://cdn.iframe.ly/abc\"></iframe></div>"
                    }
                }
            }
        });

        let html = parse_fanbox_to_html(&json.to_string(), &[]);
        assert!(html.contains("https://example.com/article/1"), "{html}");
        assert!(!html.contains("iframe.ly"), "中継先を指している: {html}");
    }

    /// `embed` ブロックは提供元と id しか持たない。URL を組み直さないと、
    /// 本文に貼られた動画や音源へ辿る手立てがどこにも残らない。
    #[test]
    fn a_service_embed_becomes_a_link_to_the_service() {
        let json = serde_json::json!({
            "id": "1",
            "title": "動画",
            "body": {
                "blocks": [{ "type": "embed", "embedId": "e1" }],
                "embedMap": {
                    "e1": { "id": "e1", "serviceProvider": "youtube", "contentId": "dQw4w9WgXcQ" }
                }
            }
        });

        let html = parse_fanbox_to_html(&json.to_string(), &[]);
        assert!(
            html.contains("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            "{html}"
        );
        assert!(html.contains("YouTube"), "{html}");
        assert!(!html.contains("表示できません"), "{html}");
    }

    /// 知らない提供元でも、貼られていたことは残す。押せないだけ。
    #[test]
    fn an_unknown_service_embed_is_still_shown() {
        let json = serde_json::json!({
            "id": "1",
            "title": "埋め込み",
            "body": {
                "blocks": [{ "type": "embed", "embedId": "e1" }],
                "embedMap": { "e1": { "serviceProvider": "unknown_service", "contentId": "abc" } }
            }
        });

        let html = parse_fanbox_to_html(&json.to_string(), &[]);
        assert!(html.contains("novel-link-card"), "{html}");
        assert!(html.contains("unknown service"), "{html}");
        assert!(
            !html.contains("<a "),
            "宛先を確かめずにリンクにしている: {html}"
        );
    }

    /// 埋め込みの id に経路を書き換える文字が混ざっていても、組み立てない。
    #[test]
    fn an_embed_id_cannot_escape_the_service_url() {
        assert_eq!(embed_service_url("youtube", "abc\" onmouseover=\"x"), None);
        assert_eq!(embed_service_url("youtube", "  "), None);
        assert_eq!(
            embed_service_url("youtube", "https://evil.example/x").as_deref(),
            Some("https://evil.example/x")
        );
    }

    /// 中身の無い `text` を持ったまま、本文は `body.blocks` にある形がある。
    /// そこで抜けてしまうと、投稿全体が空になる。
    #[test]
    fn an_empty_text_field_does_not_hide_the_blocks() {
        let json = serde_json::json!({
            "id": "1",
            "title": "投稿",
            "text": "",
            "body": { "blocks": [{ "type": "p", "text": "本文はこちらにある" }] }
        });
        let html = parse_fanbox_to_html(&json.to_string(), &[]);
        assert!(html.contains("本文はこちらにある"), "{html}");
    }

    /// 作者を指す埋め込みも、辿れる住所にする。手元にその作者がいれば、
    /// 画面側がアプリの中の作者ページへ読み替える。
    #[test]
    fn a_creator_embed_points_at_the_creator_page() {
        let json = serde_json::json!({
            "id": "1",
            "title": "紹介",
            "body": {
                "blocks": [{ "type": "url_embed", "urlEmbedId": "c1" }],
                "urlEmbedMap": {
                    "c1": { "type": "fanbox.creator", "profile": { "creatorId": "mizu-atelier" } }
                }
            }
        });
        let html = parse_fanbox_to_html(&json.to_string(), &[]);
        assert!(html.contains("https://mizu-atelier.fanbox.cc"), "{html}");
    }

    /// 取得元が種類を増やしても、文字を持つブロックは本文として読める。
    #[test]
    fn an_unknown_block_still_shows_the_text_it_carries() {
        let json = serde_json::json!({
            "id": "1",
            "title": "投稿",
            "body": {
                "blocks": [{ "type": "quote", "text": "引用された一文" }]
            }
        });

        let html = parse_fanbox_to_html(&json.to_string(), &[]);
        assert!(html.contains("引用された一文"), "{html}");
        assert!(!html.contains("表示できません"), "{html}");
    }

    #[test]
    fn current_api_wrapper_is_unwrapped_for_reading() {
        let json = serde_json::json!({
            "body": { "post": {
                "id": "5106430",
                "title": "投稿",
                "body": { "text": "包まれた本文" }
            }}
        });
        assert!(parse_fanbox_to_html(&json.to_string(), &[]).contains("包まれた本文"));
    }

    #[test]
    fn image_and_file_posts_keep_text_and_attachments() {
        let json = serde_json::json!({
            "id": "1",
            "title": "画像とファイル",
            "body": {
                "text": "説明文",
                "images": [{ "id": "image-1", "extension": "jpg" }],
                "files": [{
                    "id": "file-1", "name": "資料", "extension": "zip", "size": 2048
                }]
            }
        });
        let assets = vec![
            AssetEntry {
                id: 1,
                download_id: 1,
                asset_type: "illustration".to_string(),
                filename: "image-1.jpg".to_string(),
                local_path: "C:/library/image-1.jpg".to_string(),
                original_url: None,
                mime_type: Some("image/jpeg".to_string()),
                file_size_bytes: 1,
            },
            AssetEntry {
                id: 2,
                download_id: 1,
                asset_type: "file".to_string(),
                filename: "資料.zip".to_string(),
                local_path: "C:/library/資料.zip".to_string(),
                original_url: None,
                mime_type: Some("application/zip".to_string()),
                file_size_bytes: 2048,
            },
        ];
        let html = parse_fanbox_to_html(&json.to_string(), &assets);
        assert!(html.contains("説明文"));
        assert!(html.contains("C:/library/image-1.jpg"));
        assert!(html.contains("資料.zip"));
    }
}
