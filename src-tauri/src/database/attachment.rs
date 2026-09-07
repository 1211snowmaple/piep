//! 添付ファイルの中の本文を、投稿と同じ形へ差し込む。
//!
//! FANBOX には、本文を PDF で添付する作者がいる。投稿側には挨拶しか無く、
//! 作品は添付の中にある。取り込まなければ、棚にあっても読めず検索にも出ない。
//!
//! # なぜ「投稿と同じ形」なのか
//!
//! 本文の HTML も検索用の平文も EPUB も、すべて保存済みの JSON から毎回組んで
//! いる。取り出した段落を投稿と同じ `blocks` の形で差し込めば、**読書・検索・
//! 書き出し・編集のどれも、この先を知らないまま今までどおり動く。**
//! 経路ごとに受け渡しを足すより、入口を一つにするほうが食い違いが起きない。
//!
//! 同じ手はすでに `commands/epub.rs` の `apply_active_edit_to_epub_data` が
//! 使っている。編集した本文を書き出すときも、平文ではなく取得元と同じ形で渡す。
//!
//! # 原本は書き換えない
//!
//! 触るのは読み込んだあとのメモリ上の値だけである。`original.json` も PDF も
//! そのまま残る（[設計原則 1](../../../docs/policy/02-principles.md)）。

use super::models::AssetEntry;
use serde_json::{json, Map, Value};
use std::path::Path;

/// 差し込んだ添付一つぶんの記録。取り込めた事実を画面や記録に出すために返す。
#[derive(Debug, Clone)]
pub struct ImportedAttachment {
    pub file_name: String,
    pub method: &'static str,
    pub page_count: usize,
    pub char_count: usize,
}

/// 取り込んだ本文の前に置く印。
///
/// FANBOX が使わない型にしてあるので、`p` と `header` だけを読む検索用の平文には
/// 入らない。**「この先は取り込んだ本文だ」と読み手に伝えるための一行であって、
/// 作品の一部ではない。**
pub const NOTICE_BLOCK_TYPE: &str = "piep_attachment_notice";

/// 保存済みの投稿へ、添付から取り出した本文を差し込む。
///
/// 取り出せなかった添付には何もしない。**取り込みが失敗しても、今までどおりの
/// 📎 の箱が残るだけで、作品の見え方は壊れない。**
pub fn apply_attachment_text(
    data: &mut Value,
    source: &str,
    assets: &[AssetEntry],
) -> Vec<ImportedAttachment> {
    if source != "fanbox" || assets.is_empty() {
        return Vec::new();
    }
    let post = crate::fanbox_api::payload::post_mut_or_self(data);
    let Some(body) = post.get_mut("body").and_then(Value::as_object_mut) else {
        return Vec::new();
    };
    if body.contains_key("blocks") {
        apply_to_blocks(body, assets)
    } else {
        apply_to_files(body, assets)
    }
}

/// 保存済みの JSON を読み、添付の中の本文を差し込んだ値を返す。
pub(crate) fn materialize(raw_json: &str, source: &str, assets: &[AssetEntry]) -> Value {
    let mut value: Value = match serde_json::from_str(raw_json) {
        Ok(value) => value,
        Err(_) => return Value::Null,
    };
    apply_attachment_text(&mut value, source, assets);
    value
}

/// 保存済みの JSON から、読書用の HTML と検索用の平文をまとめて作る。
///
/// **この二つは同じ値から作らなければならない。** 別々に読むと、片方にだけ
/// 添付の本文が入って「読めるのに検索に出てこない」作品ができる。
pub(crate) fn content_from_json(
    raw_json: &str,
    source: &str,
    assets: &[AssetEntry],
) -> (String, String) {
    let value = materialize(raw_json, source, assets);
    if value.is_null() {
        return (String::new(), String::new());
    }
    let html = if source == "pixiv" {
        super::parser::parse_pixiv_value_to_html(&value, assets)
    } else if source == "fanbox" {
        super::parser::parse_fanbox_value_to_html(&value, assets)
    } else {
        String::new()
    };
    let plain_text = super::search::extract_search_body(&value, source);
    (html, plain_text)
}

/// 検索用の平文だけを作る。索引を組み直すときは本文 HTML が要らない。
pub(crate) fn search_body_from_json(raw_json: &str, source: &str, assets: &[AssetEntry]) -> String {
    let value = materialize(raw_json, source, assets);
    if value.is_null() {
        return String::new();
    }
    super::search::extract_search_body(&value, source)
}

/// `blocks` を持つ投稿。`file` ブロックの直後へ差し込む。
fn apply_to_blocks(
    body: &mut Map<String, Value>,
    assets: &[AssetEntry],
) -> Vec<ImportedAttachment> {
    let Some(blocks) = body.get("blocks").and_then(Value::as_array).cloned() else {
        return Vec::new();
    };
    let file_map = body.get("fileMap").cloned().unwrap_or(Value::Null);
    let mut imported = Vec::new();
    let mut said_something = false;
    let mut rebuilt = Vec::with_capacity(blocks.len());
    for block in blocks {
        let file_id = block
            .get("type")
            .and_then(Value::as_str)
            .filter(|kind| *kind == "file")
            .and_then(|_| block.get("fileId"))
            .and_then(Value::as_str)
            .map(str::to_string);
        rebuilt.push(block);
        let Some(file_id) = file_id else {
            continue;
        };
        let Some(entry) = file_map.get(&file_id) else {
            continue;
        };
        match import_one(entry, assets) {
            Import::Imported(record, mut inserted) => {
                imported.push(record);
                rebuilt.append(&mut inserted);
                said_something = true;
            }
            Import::Reported(mut notice) => {
                rebuilt.append(&mut notice);
                said_something = true;
            }
            Import::Skipped => {}
        }
    }
    if !said_something {
        return Vec::new();
    }
    body.insert("blocks".to_string(), Value::Array(rebuilt));
    imported
}

/// `files` を持つ投稿（FANBOX の「ファイル」形式）。**この形は `blocks` を持たない。**
///
/// 取り込む本文があるときだけ `blocks` を組み立てる。組み立てると読書画面の
/// ページ割りが段落の境で入るようになり、2万字が一枚に載らずに済む。
/// 取り込めなければ何もしないので、これまでの見え方は変わらない。
fn apply_to_files(body: &mut Map<String, Value>, assets: &[AssetEntry]) -> Vec<ImportedAttachment> {
    let Some(files) = body.get("files").and_then(Value::as_array).cloned() else {
        return Vec::new();
    };
    let mut imported = Vec::new();
    let mut said_something = false;
    let mut blocks = Vec::new();
    let mut file_map = Map::new();

    for line in body
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .split('\n')
    {
        blocks.push(json!({ "type": "p", "text": line.trim_end_matches('\r') }));
    }

    for file in &files {
        let Some(id) = file.get("id").and_then(Value::as_str) else {
            continue;
        };
        file_map.insert(id.to_string(), file.clone());
        blocks.push(json!({ "type": "file", "fileId": id }));
        match import_one(file, assets) {
            Import::Imported(record, mut inserted) => {
                imported.push(record);
                blocks.append(&mut inserted);
                said_something = true;
            }
            Import::Reported(mut notice) => {
                blocks.append(&mut notice);
                said_something = true;
            }
            Import::Skipped => {}
        }
    }

    if !said_something {
        return Vec::new();
    }
    body.insert("blocks".to_string(), Value::Array(blocks));
    // `fileMap` が無いと、組み立てた `file` ブロックから元の名前を引けない。
    body.entry("fileMap".to_string())
        .or_insert_with(|| Value::Object(file_map));
    imported
}

/// 取り込んだ中身。取り出し方が違っても、差し込む形は同じにする。
struct ImportedBody {
    paragraphs: Vec<String>,
    method: &'static str,
    /// 紙面を持つ添付だけが数を持つ。テキストファイルは 0。
    page_count: usize,
    char_count: usize,
}

/// 添付一つを見た結果。
enum Import {
    /// 本文を持ちうる形ではない（音源、動画、書庫）。何も足さない。
    Skipped,
    /// 試したが読めなかった。**黙って 📎 の箱だけを残さない。**
    Reported(Vec<Value>),
    /// 取り込めた。印と段落を返す。
    Imported(ImportedAttachment, Vec<Value>),
}

/// 添付一つを取り込む。差し込むブロックの列を返す。
fn import_one(entry: &Value, assets: &[AssetEntry]) -> Import {
    let name = entry.get("name").and_then(Value::as_str).unwrap_or("");
    let extension = entry.get("extension").and_then(Value::as_str).unwrap_or("");
    let file_name = if extension.is_empty() {
        name.to_string()
    } else {
        format!("{name}.{extension}")
    };
    let supported = extension.eq_ignore_ascii_case("pdf") || extension.eq_ignore_ascii_case("txt");
    if !supported {
        return Import::Skipped;
    }
    let id = entry.get("id").and_then(Value::as_str).unwrap_or("");
    let Some(asset) = super::parser::find_file_asset(assets, id, &file_name) else {
        // 添付そのものが手元に無い。取得に失敗したか、まだ落としていない。
        // これは取り出しの失敗ではないので、何も言わない。
        return Import::Skipped;
    };
    let path = Path::new(&asset.local_path);

    let body = if extension.eq_ignore_ascii_case("pdf") {
        read_pdf(path)
    } else {
        read_text_file(path)
    };
    let body = match body {
        Ok(body) => body,
        Err(error) => {
            // 読めない添付は珍しくない（画像だけの PDF、鍵付き、壊れたもの、
            // 見慣れない文字コード）。作品を開けなくするほどのことではないので、
            // 記録して、読み手には一行で伝える。
            log::info!("添付 {} から本文を取り出せない: {error}", asset.filename);
            return Import::Reported(vec![json!({
                "type": NOTICE_BLOCK_TYPE,
                "fileName": file_name,
                "error": error,
            })]);
        }
    };

    let mut blocks = Vec::with_capacity(body.paragraphs.len() + 1);
    blocks.push(json!({
        "type": NOTICE_BLOCK_TYPE,
        "fileName": file_name,
        "method": body.method,
        "pageCount": body.page_count,
        "charCount": body.char_count,
        // 原本を開くために要る。取り込んだ本文と原本を並べて見られなければ、
        // 取り込みが正しいかを利用者が確かめる手立てが無い。紙面を持つ添付だけ。
        "localPath": if body.page_count > 0 { Value::String(asset.local_path.clone()) } else { Value::Null },
    }));
    for paragraph in &body.paragraphs {
        blocks.push(json!({ "type": "p", "text": paragraph }));
    }
    Import::Imported(
        ImportedAttachment {
            file_name,
            method: body.method,
            page_count: body.page_count,
            char_count: body.char_count,
        },
        blocks,
    )
}

fn read_pdf(path: &Path) -> Result<ImportedBody, String> {
    let text = crate::pdf::extract_text(path)?;
    Ok(ImportedBody {
        method: text.method.as_str(),
        page_count: text.page_count,
        char_count: text.char_count,
        paragraphs: text.paragraphs,
    })
}

/// 本文をそのまま入れたテキストファイル。
///
/// PDF と違って**組み直すものが無い。** 作者の改行がそのまま段落の切れ目で、
/// 折り返しか段落末かを当てにいく必要はない。だから経路も推定ではなく、
/// 読み手にもそう伝える。
fn read_text_file(path: &Path) -> Result<ImportedBody, String> {
    // 本文として置かれた `.txt` が数 MB になることはない。ここを開けているのは
    // 作品を読むためであって、置いてあるものを何でも読み込むためではない。
    const MAX_BYTES: u64 = 8 * 1024 * 1024;
    let size = std::fs::metadata(path)
        .map_err(|error| format!("大きさを見られません: {error}"))?
        .len();
    if size > MAX_BYTES {
        return Err(format!("本文にしては大きすぎます（{size} バイト）"));
    }
    let bytes = std::fs::read(path).map_err(|error| format!("読めません: {error}"))?;
    let text = decode_text(&bytes).ok_or_else(|| "文字コードが分かりません".to_string())?;

    let mut paragraphs: Vec<String> = Vec::new();
    for line in text.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        let line = line.trim_end();
        // 空行が続いても一つにまとめる。原稿の切れ目に入れた三行の空きが、
        // 読書画面でそのまま三行の空白になると、送りの途中で本文が消える。
        if line.is_empty() && paragraphs.last().is_some_and(String::is_empty) {
            continue;
        }
        paragraphs.push(line.to_string());
    }
    while paragraphs.first().is_some_and(String::is_empty) {
        paragraphs.remove(0);
    }
    while paragraphs.last().is_some_and(String::is_empty) {
        paragraphs.pop();
    }

    let char_count = paragraphs
        .iter()
        .flat_map(|paragraph| paragraph.chars())
        .filter(|ch| !ch.is_whitespace())
        .count();
    if char_count == 0 {
        return Err("中身が空です".to_string());
    }
    Ok(ImportedBody {
        paragraphs,
        method: "text",
        page_count: 0,
        char_count,
    })
}

/// バイト列を文字にする。
///
/// **UTF-8 を先に試す。** Shift_JIS の復号はほとんどのバイト列を受け取って
/// しまうので、順番を変えると UTF-8 の日本語が文字化けしたまま通る。棚にある
/// 7 つはすべて UTF-8 だったが、日本語のテキストファイルには Shift_JIS も多い。
fn decode_text(bytes: &[u8]) -> Option<String> {
    // BOM はここで落ちる。
    let (text, _, had_errors) = encoding_rs::UTF_8.decode(bytes);
    if !had_errors {
        return Some(text.into_owned());
    }
    let (text, _, had_errors) = encoding_rs::SHIFT_JIS.decode(bytes);
    if !had_errors {
        return Some(text.into_owned());
    }
    None
}

#[cfg(test)]
mod tests;
