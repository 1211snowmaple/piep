//! 読む画面の HTML を転送用のページに割り、目次と本文内検索を作る。
//! ページの切れ目は読書位置としおりの基準なので、デスクトップと携帯で必ず同じにする。

use regex::Regex;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

/// 作品全体の見出し 1 つ。読書画面の目次が使う。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaderOutlineEntry {
    /// 1 起点のページ番号。
    pub page: usize,
    /// そのページの中で何番目の見出しか（0 起点）。
    pub index: usize,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaderSearchHit {
    /// One-based transport/source page number for direct navigation.
    pub page: usize,
    pub snippet: String,
    pub count: usize,
}

pub const READER_PAGE_TARGET_BYTES: usize = 128 * 1024;

/// 転送用のページの大きさとして数える長さ。画像・添付の置き場（`data-local-path` の値）は数えない。
///
/// 置き場の長さは本文と関係がない（保存先のフォルダーを含むパスのことも、ファイルの
/// 識別子のこともある）。数えると、同じ本文でも保存先や棚が替わるだけでページの切れ目が動き、
/// 「何ページ目の何行目」で憶えた読書位置としおりが別の行を指す。
pub fn transport_len(html: &str) -> usize {
    static PLACE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r#"data-local-path="[^"]*""#).expect("valid local path regex")
    });
    html.len()
        - PLACE
            .find_iter(html)
            .map(|found| found.len())
            .sum::<usize>()
}

/// Converts source-level boundaries into reasonably sized transport pages.
/// Boundaries always sit between complete source blocks, so a page can be
/// sanitized independently without producing malformed markup.
pub fn paginate_reader_html(html: &str, source: &str) -> (Vec<String>, Vec<usize>) {
    if html.trim().is_empty() {
        return (Vec::new(), Vec::new());
    }
    let marker = if source.eq_ignore_ascii_case("pixiv") {
        "<!-- newpage -->"
    } else {
        "<!-- content-block -->"
    };
    let blocks = html
        .split(marker)
        .map(str::trim)
        .filter(|part| !part.is_empty());
    let mut pages = Vec::new();
    // pixiv の [newpage] が作る「原稿のページ」が、転送用の何ページ目から
    // 始まるか。長い原稿ページは転送のために割られるので、この対応表が無いと
    // `[jump:5]` が 5 ページ目ではない場所へ飛ぶ。
    let mut source_page_starts = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;
    for block in blocks {
        // Pixiv's [newpage] is user-visible page semantics and must never be
        // coalesced. A source page can nevertheless be several megabytes long;
        // break that one transport response at generated line boundaries so a
        // single IPC message does not have to carry the entire novel.
        if source.eq_ignore_ascii_case("pixiv") {
            source_page_starts.push(pages.len());
            let mut chunk = String::new();
            let mut chunk_len = 0usize;
            for line in block.split_inclusive("<br />\n") {
                let line_len = transport_len(line);
                if !chunk.is_empty()
                    && chunk_len.saturating_add(line_len) > READER_PAGE_TARGET_BYTES
                {
                    pages.push(std::mem::take(&mut chunk));
                    chunk_len = 0;
                }
                chunk.push_str(line);
                chunk_len += line_len;
            }
            if !chunk.trim().is_empty() {
                pages.push(chunk);
            }
            continue;
        }
        let block_len = transport_len(block);
        if !current.is_empty() && current_len.saturating_add(block_len) > READER_PAGE_TARGET_BYTES {
            pages.push(std::mem::take(&mut current));
            current_len = 0;
        }
        if !current.is_empty() {
            current.push('\n');
            current_len += 1;
        }
        current.push_str(block);
        current_len += block_len;
    }
    if !current.trim().is_empty() {
        pages.push(current);
    }
    if pages.is_empty() {
        pages.push(html.to_string());
    }
    // 割られなかった原稿ページは、転送用のページとそのまま 1 対 1。
    if source_page_starts.is_empty() {
        source_page_starts = (0..pages.len()).collect();
    }
    (pages, source_page_starts)
}

pub fn plain_text_from_reader_html(html: &str) -> String {
    static TAGS: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").expect("valid HTML tag regex"));
    let separated = html
        .replace("<br />", "\n")
        .replace("<br>", "\n")
        .replace("</p>", "\n")
        .replace("</h2>", "\n")
        .replace("</blockquote>", "\n");
    TAGS.replace_all(&separated, "")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .trim()
        .to_string()
}

/// 本文内検索のための、**1 文字を 1 文字へ**畳む正規化。
///
/// 棚の検索は形態素解析まで通して表記ゆれを吸収するのに、本文内検索だけが
/// `to_lowercase` の部分一致だった。同じアプリの中で探し方の水準が二段違って
/// いたので、「カタカナ」で書かれた語を「かたかな」で探すと出てこなかった。
///
/// ここで畳み方を 1 対 1 に留めているのは、見つけた位置をそのまま元の本文の
/// 文字位置として使うため。長さが変わる畳み方（半角カナの濁点など）は
/// 扱わない ―― 位置がずれるほうが、少し見つからないことより困る。
pub fn fold_char(ch: char) -> char {
    if ch == '\u{3000}' {
        return ' ';
    }
    // 全角英数などは NFKC で半角へ。1 文字に収まるものだけを受け取る。
    let single = ch.to_string();
    let compat = {
        let mut chars = single.nfkc();
        match (chars.next(), chars.next()) {
            (Some(only), None) => only,
            _ => ch,
        }
    };
    let lowered = {
        let mut chars = compat.to_lowercase();
        match (chars.next(), chars.next()) {
            (Some(only), None) => only,
            _ => compat,
        }
    };
    // カタカナはひらがなへ寄せる。ヴ（U+30F4）までを対象にする。
    match lowered {
        'ァ'..='ヶ' => char::from_u32(lowered as u32 - 0x60).unwrap_or(lowered),
        other => other,
    }
}

pub static READER_HEADING: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?is)<h2\b[^>]*>(.*?)</h2>").expect("valid heading regex")
});

pub fn fold_for_reader_search(query: &str) -> String {
    query.chars().map(fold_char).collect()
}

/// 畳んだ本文の中で、畳んだ検索語が始まる文字位置をすべて返す。
///
/// 重なる一致は数えない。「ああ」を「あああ」から 2 件と数えると、画面側が
/// 入れ子の印を描くことになり、一覧の件数と本文の印がずれる。
pub fn folded_match_positions(haystack: &[char], needle: &str) -> Vec<usize> {
    let needle = needle.chars().collect::<Vec<_>>();
    if needle.is_empty() || haystack.len() < needle.len() {
        return Vec::new();
    }
    let mut positions = Vec::new();
    let mut start = 0;
    while start + needle.len() <= haystack.len() {
        if haystack[start..start + needle.len()] == needle[..] {
            positions.push(start);
            start += needle.len();
        } else {
            start += 1;
        }
    }
    positions
}

/// 作品全体の見出しと、それが載っているページ（1 から）。デスクトップと携帯の読む画面で共通。
pub fn reader_outline(pages: &[String]) -> Vec<ReaderOutlineEntry> {
    let mut entries = Vec::new();
    for (page, html) in pages.iter().enumerate() {
        for (index, captures) in READER_HEADING.captures_iter(html).enumerate() {
            let title = plain_text_from_reader_html(&captures[1]);
            let title = title.trim();
            if title.is_empty() {
                continue;
            }
            entries.push(ReaderOutlineEntry {
                page: page + 1,
                index,
                title: title.to_string(),
            });
            // 目次が本文より長くなっては意味がない。
            if entries.len() >= 2_000 {
                return entries;
            }
        }
    }
    entries
}

/// 本文内検索。ページ（1 から）ごとに、一致の数と最初の一致のまわりを返す。デスクトップと携帯で共通。
pub fn search_reader_pages(pages: &[String], query: &str, limit: usize) -> Vec<ReaderSearchHit> {
    let needle = fold_for_reader_search(query.trim());
    if needle.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for (page, html) in pages.iter().enumerate() {
        let plain = plain_text_from_reader_html(html);
        let chars = plain.chars().collect::<Vec<_>>();
        // 1 文字が 1 文字に対応する畳み方なので、畳んだ側で見つけた位置は
        // そのまま元の本文の文字位置になる。
        let folded = chars.iter().map(|ch| fold_char(*ch)).collect::<Vec<_>>();
        let positions = folded_match_positions(&folded, &needle);
        if positions.is_empty() {
            continue;
        }
        let char_index = positions[0];
        let start = char_index.saturating_sub(42);
        let end = (char_index + needle.chars().count() + 70).min(chars.len());
        let snippet = format!(
            "{}{}{}",
            if start > 0 { "…" } else { "" },
            chars[start..end].iter().collect::<String>(),
            if end < chars.len() { "…" } else { "" }
        );
        hits.push(ReaderSearchHit {
            page: page + 1,
            snippet,
            count: positions.len(),
        });
        if hits.len() >= limit.clamp(1, 200) {
            break;
        }
    }
    hits
}
