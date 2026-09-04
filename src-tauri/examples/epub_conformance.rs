//! 組み込みテンプレートごとに EPUB を1冊ずつ組み立てて、書き出す。
//!
//! EPUBCheck へ渡すための原稿を、利用者のライブラリ無しで用意するのが目的で
//! ある。方針書は「EPUBCheck で検証できる範囲を適合と書く」と定めているので、
//! その根拠は手で走らせた一度きりではなく、変更のたびに取り直せなければ
//! ならない。内蔵の検証器は自分で書いた規則しか知らないため、外の権威と
//! 置き換えることはできない。
//!
//!   cargo run --example epub_conformance -- <出力ディレクトリ>
//!
//! 原稿は「通れば嬉しい」ものではなく、**壊れるとしたらここ**という形を選んで
//! ある。エスケープの要る文字、ファイル名に使えない文字、二階層の目次、
//! シリーズと訳語つきタグ、閲覧制限の印。内蔵の検証器にも通し、そこで落ちる
//! ものは EPUBCheck へ渡す前に止める。

use piep_lib::epub::builder::EpubBuilder;
use piep_lib::epub::intermediate::*;
use piep_lib::epub::template::TemplateManager;
use piep_lib::epub::validate;
use std::path::PathBuf;

/// XHTML の中で意味を持つ文字と、ファイル名に使えない文字を両方入れた題名。
const HOSTILE_TITLE: &str = r#"AT&T <script> "引用" & C:\path/to\file *?|"#;

fn conformance_manifest() -> EpubManifest {
    EpubManifest {
        core: EpubCore {
            id_: "conformance-1".into(),
            name: HOSTILE_TITLE.into(),
            author: EpubAuthor {
                name: r#"作者 & <b>太字</b>"#.into(),
                id: "42".into(),
                account: Some("author_&_account".into()),
                url: Some("https://www.pixiv.net/users/42".into()),
                icon_url: None,
            },
            description: Some("<p>紹介文に &amp; と &lt;タグ&gt; が混ざっている。</p>".into()),
            description_text: Some("紹介文に & と <タグ> が混ざっている。".into()),
            keywords: vec!["創作".into(), "R-18".into()],
            tags: vec![
                EpubTag {
                    name: "創作".into(),
                    translated: Some("original".into()),
                },
                EpubTag {
                    name: r#"引用"つき"#.into(),
                    translated: None,
                },
            ],
            date_published: "2026-01-02T03:04:05+09:00".into(),
            date_modified: Some("2026-02-03T04:05:06+09:00".into()),
            main_entity_of_page: "https://www.pixiv.net/novel/show.php?id=1".into(),
            is_part_of: Some(EpubSeries {
                name: r#"シリーズ & その続き"#.into(),
                order: Some(2),
                id: Some("7".into()),
                url: Some("https://www.pixiv.net/novel/series/7".into()),
            }),
            language: "ja".into(),
            publisher: "piep".into(),
        },
        provider: ProviderData {
            source: "pixiv".into(),
            novel_id: Some("1".into()),
            post_id: None,
            series_id: Some("7".into()),
            post_type: None,
        },
        content: EpubContent {
            pages: vec![
                EpubPage {
                    title: Some("一枚目".into()),
                    // 見出しの id は目次から指されるので、本文側にも必ず置く。
                    html_content: concat!(
                        "<h2 id=\"ch-1\">最初の見出し &amp; その先</h2>",
                        "<p>本文に &amp; と &lt; と &gt; と &quot; を入れる。</p>",
                        "<p><ruby>漢字<rt>かんじ</rt></ruby>のルビ。</p>",
                        "<h2 id=\"ch-2\">二つめの見出し</h2>",
                        "<p><a href=\"https://example.com/?a=1&amp;b=2\">外部リンク</a></p>",
                    )
                    .into(),
                    order: 1,
                    chapters: vec![
                        EpubChapter {
                            id: "ch-1".into(),
                            title: "最初の見出し & その先".into(),
                        },
                        EpubChapter {
                            id: "ch-2".into(),
                            title: "二つめの見出し".into(),
                        },
                    ],
                    part: 0,
                },
                EpubPage {
                    title: None,
                    html_content: "<p>題名の無いページ。</p>".into(),
                    order: 2,
                    chapters: Vec::new(),
                    part: 0,
                },
            ],
            cover_image: None,
            illustrations: Vec::new(),
            attachments: vec![EpubAttachment {
                name: r#"添付 & ファイル"#.into(),
                extension: Some("psd".into()),
                size_bytes: Some(1024),
            }],
            text_length: 120,
        },
        stats: EpubStats {
            text_length: 120,
            page_count: 2,
            chapter_count: 2,
            image_count: 0,
            attachment_count: 1,
            like_count: Some(12),
            comment_count: Some(3),
            fee_required: None,
            adult: true,
        },
    }
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let out_dir = PathBuf::from(
        args.get(1)
            .ok_or("usage: epub_conformance <出力ディレクトリ>")?,
    );
    std::fs::create_dir_all(&out_dir).map_err(|error| error.to_string())?;

    let template_root = std::env::temp_dir().join("piep-epub-conformance-templates");
    let manager = TemplateManager::new(template_root);
    manager.initialize_defaults()?;

    let mut failed = false;
    for template in ["default", "pixiv", "fanbox"] {
        let contents = manager.load_template_contents(template)?;
        let settings = manager.read_settings(template);
        let output = out_dir.join(format!("{template}.epub"));
        EpubBuilder::new(
            conformance_manifest(),
            contents,
            settings,
            ImageCompressOptions::default(),
        )
        .build(&output)?;

        // 内蔵の検証器で先に落とす。EPUBCheck を起動する前に分かることを、
        // わざわざ外まで持っていかない。
        let report = validate::validate_epub(&output)?;
        if report.valid {
            println!("built {}", output.display());
        } else {
            failed = true;
            eprintln!("internal validation failed: {}", output.display());
            for issue in &report.issues {
                eprintln!("  {issue:?}");
            }
        }
    }

    if failed {
        return Err("内蔵の検証器が問題を見つけました".into());
    }
    Ok(())
}
