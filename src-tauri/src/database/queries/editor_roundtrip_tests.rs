//! 取り込んだ本文を編集画面へ通して書き戻すまでの、行の保存についての試験。
//!
//! ここが守るのは一つだけ ――「編集画面を開いて、何も変えずに反映する」と、
//! 読書画面の見え方が変わらないこと。以前はこれが守られておらず、往復した
//! だけで作品全体が改行のない一続きの塊になっていた。

use super::*;
use crate::database::parser;

fn no_assets() -> Vec<AssetEntry> {
    Vec::new()
}

fn pixiv_html(text: &str) -> String {
    let raw = serde_json::json!({ "text": text }).to_string();
    parser::parse_pixiv_to_html(&raw, &no_assets())
}

/// 読み手に見える形だけを比べる。`<!-- content-block -->` は転送用の印で
/// 画面には出ないので取り除く。HTML では畳まれる改行も、見え方を変えない。
fn rendered(html: &str) -> String {
    let marker = Regex::new(r"(?i)<!--\s*content-block\s*-->").expect("valid marker regex");
    marker
        .replace_all(html, "")
        .replace('\n', "")
        .replace("<br />", "<br />\n")
}

fn roundtrip(html: &str) -> String {
    let blocks = html_to_editor_blocks(html, &no_assets());
    blocks_to_html(&blocks, &no_assets())
}

#[test]
fn publishing_an_untouched_work_does_not_change_how_it_reads() {
    for text in [
        "一行目\n二行目\n\n段落が変わる",
        "「ねえ」\n「なに」\n\n彼は答えた。",
        "　字下げのある段落。\n　次の行も字下げ。\n\n　新しい段落。",
        "場面の切れ目\n\n\n\n次の場面",
        "[chapter:序章]\n本文がここから始まる。\n\n続きの段落。",
        "前置き\n[newpage]\n次のページの本文",
        "行末に何も無い一行だけの作品",
    ] {
        let source = pixiv_html(text);
        let once = roundtrip(&source);
        assert_eq!(
            rendered(&source),
            rendered(&once),
            "取り込んだ本文と往復後で見え方が変わった\n入力: {text:?}\n元:   {source:?}\n後:   {once:?}"
        );
        // 二度目も同じ。少しずつずれていく作りになっていないことを見る。
        let twice = roundtrip(&once);
        assert_eq!(
            rendered(&once),
            rendered(&twice),
            "往復のたびに本文がずれていく\n入力: {text:?}"
        );
    }
}

#[test]
fn a_single_line_break_stays_a_line_break() {
    let blocks = html_to_editor_blocks(&pixiv_html("一行目\n二行目"), &no_assets());
    assert_eq!(blocks.len(), 1, "改行だけで段落が割れている: {blocks:?}");
    assert_eq!(blocks[0].text.as_deref(), Some("一行目\n二行目"));
    assert!(
        rendered(&blocks_to_html(&blocks, &no_assets())).contains("一行目<br />"),
        "改行が失われている"
    );
}

#[test]
fn a_blank_line_separates_paragraphs_and_is_counted() {
    let blocks = html_to_editor_blocks(&pixiv_html("前\n\n\n後"), &no_assets());
    assert_eq!(
        blocks
            .iter()
            .map(|block| block.text.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("前"), Some("後")]
    );
    assert_eq!(block_gap(&blocks[0]), Some(0));
    assert_eq!(block_gap(&blocks[1]), Some(2), "空行の本数が失われている");
}

#[test]
fn leading_indentation_survives_a_save() {
    let blocks = html_to_editor_blocks(&pixiv_html("　その日は雨だった。"), &no_assets());
    assert_eq!(blocks[0].text.as_deref(), Some("　その日は雨だった。"));

    let inputs = blocks
        .iter()
        .map(|block| WorkBlockInput {
            block_type: block.block_type.clone(),
            text: block.text.clone(),
            asset_id: block.asset_id,
            attrs_json: block.attrs_json.clone(),
        })
        .collect::<Vec<_>>();
    let normalized = normalize_block_inputs(&inputs);
    assert_eq!(
        normalized[0].text.as_deref(),
        Some("　その日は雨だった。"),
        "保存の正規化で字下げが削られている"
    );
}

#[test]
fn a_heading_does_not_push_the_body_down_on_every_pass() {
    let source = pixiv_html("[chapter:一章]\n本文");
    let once = roundtrip(&source);
    let twice = roundtrip(&once);
    let count = |html: &str| rendered(html).matches("<br />").count();
    assert_eq!(count(&source), count(&once), "見出しの後に空行が増えた");
    assert_eq!(count(&once), count(&twice), "往復のたびに空行が増えていく");
}

#[test]
fn fanbox_transport_markers_do_not_grow_the_gaps() {
    // FANBOX の本文は「ブロック + <br /> + ブロック」を転送用の印で継いだ形。
    let source =
        "一つ目の段落\n<!-- content-block -->\n<br />\n<!-- content-block -->\n二つ目の段落";
    let blocks = html_to_editor_blocks(source, &no_assets());
    assert_eq!(
        blocks
            .iter()
            .map(|block| block.text.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("一つ目の段落"), Some("二つ目の段落")]
    );
    assert_eq!(
        block_gap(&blocks[1]),
        Some(0),
        "転送用の印の前後の改行が空行として数えられている"
    );
    let once = blocks_to_html(&blocks, &no_assets());
    assert_eq!(rendered(source), rendered(&once));
}

#[test]
fn an_image_keeps_the_reference_the_source_used() {
    let assets = vec![AssetEntry {
        id: 7,
        download_id: 1,
        asset_type: "image".to_string(),
        filename: "12345.jpg".to_string(),
        local_path: "data_assets/12345.jpg".to_string(),
        original_url: None,
        mime_type: Some("image/jpeg".to_string()),
        file_size_bytes: 10,
    }];
    let html = parser::parse_pixiv_to_html(
        &serde_json::json!({ "text": "前\n[uploadedimage:12345]\n後" }).to_string(),
        &assets,
    );
    let blocks = html_to_editor_blocks(&html, &assets);
    let image = blocks
        .iter()
        .find(|block| block.block_type == "image")
        .expect("画像ブロックが無い");
    assert_eq!(image.asset_id, Some(7));
    assert_eq!(
        block_source_ref(image).as_deref(),
        Some("12345"),
        "元の参照を控えていないと、EPUB で同じ絵を指し直せない"
    );
    // 生成された alt は説明ではないので、キャプション欄には流し込まない。
    assert_eq!(image.text, None);
}

#[test]
fn a_caption_written_in_the_editor_reaches_the_page() {
    let assets = vec![AssetEntry {
        id: 3,
        download_id: 1,
        asset_type: "image".to_string(),
        filename: "scene.webp".to_string(),
        local_path: "data_assets/scene.webp".to_string(),
        original_url: None,
        mime_type: Some("image/webp".to_string()),
        file_size_bytes: 10,
    }];
    let blocks = vec![WorkBlock {
        id: 0,
        edit_revision_id: 0,
        order: 0,
        block_type: "image".to_string(),
        text: Some("波打ち際の二人".to_string()),
        asset_id: Some(3),
        attrs_json: None,
    }];
    let html = blocks_to_html(&blocks, &assets);
    assert!(
        html.contains("波打ち際の二人"),
        "書いた説明が捨てられている: {html}"
    );
}

/// 本文内検索の畳み方。棚の検索と同じ表記ゆれを、位置を保ったまま吸収する。
#[test]
fn reader_search_absorbs_the_usual_spelling_differences() {
    let fold = |value: &str| super::reader::fold_for_reader_search(value);
    assert_eq!(fold("カタカナ"), fold("かたかな"));
    assert_eq!(fold("ＡＢＣ"), fold("abc"));
    assert_eq!(fold("Piep"), fold("ｐｉｅｐ"));
    assert_eq!(fold("１２３"), fold("123"));
    assert_eq!(fold("　"), fold(" "));
    // 位置がずれないこと。ずれると一致箇所へ飛べなくなる。
    for text in ["カタカナ", "ＡＢＣ", "混ざったＴＥＸＴです", "ヴァイオリン"]
    {
        assert_eq!(
            fold(text).chars().count(),
            text.chars().count(),
            "畳んだら文字数が変わった: {text}"
        );
    }
}

#[test]
fn reader_search_finds_every_occurrence_in_order() {
    let haystack = "あかさたなあかさたな".chars().collect::<Vec<_>>();
    let positions = super::reader::folded_match_positions(&haystack, "あか");
    assert_eq!(positions, vec![0, 5]);
    assert!(super::reader::folded_match_positions(&haystack, "まみむ").is_empty());
    // 検索語のほうが長い本文より長い場合に、境界で落ちないこと。
    assert!(super::reader::folded_match_positions(&haystack, &"あ".repeat(50)).is_empty());
}

/// 編集した作品を EPUB へ渡す道。取得元と同じ書式へ組み直せているかを見る。
#[test]
fn an_edited_work_reaches_the_epub_with_its_pictures_and_pages() {
    let assets = vec![AssetEntry {
        id: 5,
        download_id: 1,
        asset_type: "image".to_string(),
        filename: "98765.jpg".to_string(),
        local_path: "data_assets/98765.jpg".to_string(),
        original_url: None,
        mime_type: Some("image/jpeg".to_string()),
        file_size_bytes: 10,
    }];
    let source = parser::parse_pixiv_to_html(
        &serde_json::json!({
            "text": "[chapter:一章]\n本文の一行目\n本文の二行目\n\n[uploadedimage:98765]\n\n[newpage]\n次のページ"
        })
        .to_string(),
        &assets,
    );
    let blocks = html_to_editor_blocks(&source, &assets);
    let text = blocks_to_pixiv_text(&blocks, &assets);

    assert!(
        text.contains("[chapter:一章]"),
        "見出しが落ちている: {text:?}"
    );
    assert!(
        text.contains("[uploadedimage:98765]"),
        "挿絵が落ちている: {text:?}"
    );
    assert!(text.contains("[newpage]"), "改ページが落ちている: {text:?}");
    assert!(
        text.contains("本文の一行目\n本文の二行目"),
        "行が潰れている: {text:?}"
    );
    // 取り込んだ本文そのものに戻れている ―― これなら書き出しは元と同じ本になる。
    let original = "[chapter:一章]\n本文の一行目\n本文の二行目\n\n[uploadedimage:98765]\n\n[newpage]\n次のページ";
    assert_eq!(text, original);
}

#[test]
fn an_edited_fanbox_post_keeps_its_headings_and_pictures() {
    let assets = vec![AssetEntry {
        id: 9,
        download_id: 1,
        asset_type: "image".to_string(),
        filename: "abc123.png".to_string(),
        local_path: "data_assets/abc123.png".to_string(),
        original_url: None,
        mime_type: Some("image/png".to_string()),
        file_size_bytes: 10,
    }];
    let blocks = vec![
        WorkBlock {
            id: 0,
            edit_revision_id: 0,
            order: 0,
            block_type: "heading".to_string(),
            text: Some("まえがき".to_string()),
            asset_id: None,
            attrs_json: None,
        },
        WorkBlock {
            id: 0,
            edit_revision_id: 0,
            order: 1,
            block_type: "image".to_string(),
            text: None,
            asset_id: Some(9),
            attrs_json: Some(serde_json::json!({ "sourceRef": "abc123" }).to_string()),
        },
    ];
    let fanbox = blocks_to_fanbox_blocks(&blocks, &assets);
    assert_eq!(fanbox[0]["type"], "header");
    assert_eq!(fanbox[0]["text"], "まえがき");
    let image = fanbox
        .iter()
        .find(|block| block["type"] == "image")
        .expect("画像ブロックが落ちている");
    assert_eq!(image["imageId"], "abc123");
}

#[test]
fn ruby_and_links_survive_the_editor() {
    let source = pixiv_html("[[rb:漢字>かんじ]]と[[jumpuri:前編>https://example.com/a]]と[jump:3]");
    let blocks = html_to_editor_blocks(&source, &no_assets());
    assert_eq!(
        blocks[0].text.as_deref(),
        Some("[[rb:漢字>かんじ]]と[[jumpuri:前編>https://example.com/a]]と[jump:3]"),
        "編集画面へ渡す前にルビと宛先が落ちている"
    );

    let once = roundtrip(&source);
    assert!(once.contains("<ruby>漢字<rt>かんじ</rt></ruby>"), "{once}");
    assert!(once.contains(r#"href="https://example.com/a""#), "{once}");
    assert!(once.contains(r#"data-page="3""#), "{once}");
    assert_eq!(rendered(&source), rendered(&once));
    assert_eq!(rendered(&once), rendered(&roundtrip(&once)));
}

#[test]
fn ruby_written_in_the_editor_becomes_ruby() {
    let blocks = vec![WorkBlock {
        id: 0,
        edit_revision_id: 0,
        order: 0,
        block_type: "paragraph".to_string(),
        text: Some("[[rb:黄昏>たそがれ]]の街".to_string()),
        asset_id: None,
        attrs_json: None,
    }];
    assert!(blocks_to_html(&blocks, &no_assets()).contains("<ruby>黄昏<rt>たそがれ</rt></ruby>"));
}

#[test]
fn a_heading_keeps_its_ruby() {
    let blocks = html_to_editor_blocks(
        &pixiv_html("[chapter:[[rb:序章>じょしょう]]]"),
        &no_assets(),
    );
    let heading = blocks
        .iter()
        .find(|block| block.block_type == "heading")
        .expect("見出しが無い");
    assert_eq!(heading.text.as_deref(), Some("[[rb:序章>じょしょう]]"));
    assert!(blocks_to_html(&blocks, &no_assets()).contains("<rt>じょしょう</rt>"));
}
