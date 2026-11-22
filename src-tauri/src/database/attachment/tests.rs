//! 差し込みの試験。
//!
//! 材料の PDF は [`crate::pdf::fixture`] が組み立てる。ここで見るのは、
//! **取り出した段落が投稿と同じ形で入るか**と、**取り込めなかったときに
//! 元の見え方が残るか**である。

use super::*;
use crate::pdf::fixture;

fn asset(local_path: &str, filename: &str) -> AssetEntry {
    AssetEntry {
        id: 1,
        download_id: 1,
        asset_type: "file".to_string(),
        filename: filename.to_string(),
        local_path: local_path.to_string(),
        original_url: None,
        mime_type: Some("application/pdf".to_string()),
        file_size_bytes: 0,
    }
}

/// FANBOX の「ファイル」形式。本文は挨拶だけで、作品は添付の中にある。
fn file_post(file_name: &str) -> Value {
    json!({
        "id": "1",
        "type": "file",
        "body": {
            "text": "良ければお納めください。",
            "files": [{
                "id": "abcdef",
                "name": file_name,
                "extension": "pdf",
                "size": 1234
            }]
        }
    })
}

/// 「記事」形式。段落の間に `file` ブロックが挟まっている。
fn article_post(file_name: &str) -> Value {
    json!({
        "id": "2",
        "type": "article",
        "body": {
            "blocks": [
                {"type": "p", "text": "前置き。"},
                {"type": "file", "fileId": "abcdef"},
                {"type": "p", "text": "後書き。"}
            ],
            "fileMap": {
                "abcdef": {"id": "abcdef", "name": file_name, "extension": "pdf", "size": 1234}
            }
        }
    })
}

fn blocks_of(value: &Value) -> Vec<(String, String)> {
    value["body"]["blocks"]
        .as_array()
        .expect("blocks があること")
        .iter()
        .map(|block| {
            (
                block["type"].as_str().unwrap_or_default().to_string(),
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

#[test]
fn a_file_post_gains_blocks_so_the_body_can_be_read_and_paged() {
    let dir = fixture::temp_dir();
    let pdf = fixture::write_pdf(&dir, true);
    let assets = [asset(&pdf.to_string_lossy(), "sample.pdf")];

    let mut data = file_post("sample");
    let imported = apply_attachment_text(&mut data, "fanbox", &assets);

    assert_eq!(imported.len(), 1);
    assert_eq!(imported[0].method, "tagged");
    assert_eq!(imported[0].page_count, 2);

    let blocks = blocks_of(&data);
    assert_eq!(
        blocks[0],
        ("p".to_string(), "良ければお納めください。".to_string())
    );
    assert_eq!(blocks[1].0, "file");
    assert_eq!(blocks[2].0, NOTICE_BLOCK_TYPE);
    assert_eq!(
        blocks[3],
        (
            "p".to_string(),
            "Alpha beta gamma delta epsilon zetaX etatheta".to_string()
        )
    );
    assert_eq!(blocks.len(), 7, "挨拶 + 添付 + 印 + 段落4つ");

    // 元の形は残す。原本と同じ鍵を読む経路が、これまでどおり動くこと。
    assert!(data["body"]["files"].is_array());
    assert_eq!(
        data["body"]["text"].as_str(),
        Some("良ければお納めください。")
    );
    assert!(data["body"]["fileMap"]["abcdef"].is_object());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_article_post_keeps_its_own_blocks_around_the_imported_body() {
    let dir = fixture::temp_dir();
    let pdf = fixture::write_pdf(&dir, true);
    let assets = [asset(&pdf.to_string_lossy(), "sample.pdf")];

    let mut data = article_post("sample");
    assert_eq!(apply_attachment_text(&mut data, "fanbox", &assets).len(), 1);

    let kinds: Vec<String> = blocks_of(&data).into_iter().map(|(kind, _)| kind).collect();
    assert_eq!(
        kinds,
        ["p", "file", NOTICE_BLOCK_TYPE, "p", "p", "p", "p", "p"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>(),
        "file ブロックの直後に印と段落が入り、前置きと後書きは残る"
    );
    let blocks = blocks_of(&data);
    assert_eq!(blocks[0].1, "前置き。");
    assert_eq!(blocks.last().unwrap().1, "後書き。");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_attachment_that_cannot_be_read_adds_a_note_but_no_body() {
    let dir = fixture::temp_dir();
    let broken = fixture::write(&dir, "broken.pdf", b"not a pdf".to_vec());
    let assets = [asset(&broken.to_string_lossy(), "broken.pdf")];

    let mut data = file_post("broken");
    let before = data.clone();
    // 取り込めた作品としては数えない。数えると、画面が「取り込み済み」と言う。
    assert!(apply_attachment_text(&mut data, "fanbox", &assets).is_empty());

    // 足すのは伝える一行だけ。本文は一段落も作らない。
    let kinds: Vec<String> = blocks_of(&data).into_iter().map(|(kind, _)| kind).collect();
    assert_eq!(
        kinds,
        ["p", "file", NOTICE_BLOCK_TYPE]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>(),
        "読めなかった添付から段落を作らないこと"
    );

    // 取得元から来た鍵はそのまま。原本は書き換えない。
    assert_eq!(data["body"]["files"], before["body"]["files"]);
    assert_eq!(data["body"]["text"], before["body"]["text"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_attachment_that_is_not_a_pdf_is_left_alone() {
    let dir = fixture::temp_dir();
    let assets = [asset("C:/nowhere/track.mp3", "track.mp3")];
    let mut data = json!({
        "id": "3",
        "type": "file",
        "body": {
            "text": "音源です。",
            "files": [{"id": "abcdef", "name": "track", "extension": "mp3", "size": 10}]
        }
    });
    let before = data.clone();
    assert!(apply_attachment_text(&mut data, "fanbox", &assets).is_empty());
    assert_eq!(data, before);
    let _ = std::fs::remove_dir_all(dir);
}

/// `.txt` の添付。棚には4話ぶんを添付で配っている作品がある。
fn text_post(file_name: &str) -> Value {
    json!({
        "id": "4",
        "type": "file",
        "body": {
            "text": "続きです。",
            "files": [{"id": "abcdef", "name": file_name, "extension": "txt", "size": 10}]
        }
    })
}

fn write_text(dir: &std::path::Path, name: &str, bytes: Vec<u8>) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn a_text_attachment_keeps_the_authors_own_line_breaks() {
    // PDF と違って組み直すものが無い。**作者の改行がそのまま段落である。**
    // 折り返しを繋ぐ処理をここへ持ち込むと、書かれた形が壊れる。
    let dir = fixture::temp_dir();
    let body = "一行目。\r\n\r\n\r\n\r\n二行目。\r\n三行目。\r\n";
    let file = write_text(&dir, "本文.txt", body.as_bytes().to_vec());
    let assets = [asset(&file.to_string_lossy(), "本文.txt")];
    let mut data = text_post("本文");

    let imported = apply_attachment_text(&mut data, "fanbox", &assets);
    assert_eq!(imported.len(), 1);
    assert_eq!(imported[0].method, "text");
    assert_eq!(
        imported[0].page_count, 0,
        "紙面を持たない添付にページは無い"
    );
    // 「一行目。」「二行目。」「三行目。」で 4 文字ずつ。空行は数えない。
    assert_eq!(imported[0].char_count, 12);

    let blocks = blocks_of(&data);
    let paragraphs: Vec<&str> = blocks
        .iter()
        .filter(|(kind, _)| kind == "p")
        .map(|(_, text)| text.as_str())
        .collect();
    assert!(
        paragraphs.ends_with(&["一行目。", "", "二行目。", "三行目。"]),
        // 空行が四つ続いても一つにまとめる。原稿の切れ目の空きがそのまま
        // 出ると、送りの途中で本文の無いページができる。
        "空行はまとめ、前後の空きは落とすこと: {paragraphs:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_shift_jis_text_attachment_is_read_without_mojibake() {
    // 日本語のテキストファイルには Shift_JIS も多い。読めないより、読めた形で
    // 入れる。**UTF-8 を先に試すのが要点で**、順番を逆にすると UTF-8 の本文が
    // 化けたまま通る。
    let dir = fixture::temp_dir();
    let (bytes, _, had_errors) = encoding_rs::SHIFT_JIS.encode("雨が降っている。");
    assert!(!had_errors);
    let file = write_text(&dir, "sjis.txt", bytes.into_owned());
    let assets = [asset(&file.to_string_lossy(), "sjis.txt")];
    let mut data = text_post("sjis");

    assert_eq!(apply_attachment_text(&mut data, "fanbox", &assets).len(), 1);
    let blocks = blocks_of(&data);
    assert!(
        blocks
            .iter()
            .any(|(kind, text)| kind == "p" && text == "雨が降っている。"),
        "{blocks:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn pixiv_posts_are_not_touched() {
    // pixiv の小説に添付ファイルは無い。**探しに行くだけ無駄で、
    // 万一 files を持つ形が来ても、こちらの都合で書き換えない。**
    let mut data = json!({"text": "本文", "body": {"files": []}});
    let before = data.clone();
    assert!(apply_attachment_text(&mut data, "pixiv", &[asset("x", "y")]).is_empty());
    assert_eq!(data, before);
}
