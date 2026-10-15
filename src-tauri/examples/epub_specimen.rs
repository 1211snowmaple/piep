//! 組み込みテンプレートの情報ページを、ブラウザで見られる 1 枚の HTML に書き出す。
//!
//! テンプレートの体裁はアプリを起動しないと確認できなかった。これは同じ原稿を
//! 3 つのテンプレートに通し、CSS を埋め込んだ自己完結の HTML を並べて出す。
//!
//!   cargo run --example epub_specimen -- <出力ディレクトリ>
//!
//! 端末のダーク表示は、ブラウザ側の配色設定を切り替えると再現できる
//! （テンプレートの色は prefers-color-scheme に追随する）。

use piep_lib::epub::intermediate::*;
use piep_lib::epub::template::{builtin_file_content, DefaultTemplates, EpubRenderer};
use std::collections::HashMap;
use std::path::PathBuf;

/// 表紙の実物は要らない。枠だけの SVG を data URI で埋める。
const COVER_PLACEHOLDER: &str = "data:image/svg+xml;utf8,\
<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 300 420'>\
<rect width='300' height='420' fill='%23ececea'/>\
<text x='150' y='215' font-size='18' fill='%23888' text-anchor='middle'>表紙画像</text></svg>";

fn contents_for(template_name: &str) -> HashMap<String, String> {
    let mut contents: HashMap<String, String> = DefaultTemplates::get_all()
        .into_iter()
        .map(|(filename, _)| {
            let body = builtin_file_content(template_name, filename)
                .unwrap_or_else(|| panic!("{template_name}/{filename} がありません"));
            (filename.to_string(), body.to_string())
        })
        .collect();
    let base = contents["_base_style.css.j2"].clone();
    let style = contents["style.css.j2"].replace("{% include \"_base_style.css.j2\" %}", &base);
    contents.insert("style.css.j2".into(), style);
    contents
}

fn settings_for(template_name: &str) -> TemplateSettings {
    use piep_lib::epub::template::{FanboxTemplates, PixivTemplates};
    let json = match template_name {
        "pixiv" => PixivTemplates::settings(),
        "fanbox" => FanboxTemplates::settings(),
        _ => DefaultTemplates::settings(),
    };
    serde_json::from_str::<TemplateSettings>(json)
        .expect("template.json")
        .normalized()
}

fn sample(source: &str) -> EpubManifest {
    let pixiv = source == "pixiv";
    EpubManifest {
        core: EpubCore {
            id_: "urn:piep:specimen:1".into(),
            name: if pixiv {
                "夜明けの糸".into()
            } else {
                "制作ノート #25 — 線を減らす練習".into()
            },
            author: EpubAuthor {
                name: if pixiv { "青葉しおり" } else { "mizu atelier" }.into(),
                id: "7".into(),
                account: Some(if pixiv { "aoba_shiori" } else { "mizu-atelier" }.into()),
                url: None,
                icon_url: None,
            },
            description: Some(
                "<p>三年ぶりに戻った港町で、彼女は編みかけの糸を見つける。<br />前話はこちら → \
                 <a href=\"https://www.pixiv.net/novel/show.php?id=8841902\">https://www.pixiv.net/novel/show.php?id=8841902</a></p>"
                    .into(),
            ),
            description_text: Some("三年ぶりに戻った港町で、彼女は編みかけの糸を見つける。".into()),
            keywords: vec!["創作".into(), "ファンタジー".into()],
            tags: ["創作", "ファンタジー", "長編", "オリジナル小説", "港町"]
                .iter()
                .map(|name| EpubTag {
                    name: (*name).to_string(),
                    translated: None,
                })
                .collect(),
            date_published: "2026-04-18T21:52:01+09:00".into(),
            date_modified: Some("2026-05-02T10:03:00+09:00".into()),
            main_entity_of_page: if pixiv {
                "https://www.pixiv.net/novel/show.php?id=8842013".into()
            } else {
                "https://mizu-atelier.fanbox.cc/posts/10920".into()
            },
            is_part_of: pixiv.then(|| EpubSeries {
                name: "星を編む人".into(),
                order: Some(13),
                id: Some("778120".into()),
                url: None,
            }),
            language: "ja".into(),
            publisher: if pixiv { "pixiv" } else { "pixivFANBOX" }.into(),
        },
        provider: ProviderData {
            source: source.to_string(),
            novel_id: pixiv.then(|| "8842013".to_string()),
            post_id: (!pixiv).then(|| "10920".to_string()),
            series_id: None,
            post_type: (!pixiv).then(|| "article".to_string()),
        },
        content: EpubContent {
            pages: Vec::new(),
            cover_image: None,
            illustrations: Vec::new(),
            attachments: if pixiv {
                Vec::new()
            } else {
                vec![
                    EpubAttachment {
                        name: "rough_0501.clip".into(),
                        extension: Some("clip".into()),
                        size_bytes: Some(4_200_000),
                    },
                    EpubAttachment {
                        name: "line_practice.psd".into(),
                        extension: Some("psd".into()),
                        size_bytes: Some(18_400_000),
                    },
                ]
            },
            text_length: 12_840,
        },
        stats: EpubStats {
            text_length: 12_840,
            page_count: 3,
            chapter_count: 3,
            image_count: 2,
            attachment_count: if pixiv { 0 } else { 2 },
            like_count: Some(482),
            comment_count: Some(17),
            fee_required: (!pixiv).then_some(500),
            adult: false,
        },
    }
}

fn main() -> Result<(), String> {
    let out_dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: cargo run --example epub_specimen -- <出力ディレクトリ>")?,
    );
    std::fs::create_dir_all(&out_dir).map_err(|error| error.to_string())?;

    for template_name in ["default", "pixiv", "fanbox"] {
        let source = if template_name == "fanbox" {
            "fanbox"
        } else {
            "pixiv"
        };
        let manifest = sample(source);
        let renderer = EpubRenderer::new(contents_for(template_name), settings_for(template_name));
        let css = renderer.render_style(&manifest)?;
        let info = renderer.render_info_page(&manifest, Some(COVER_PLACEHOLDER))?;

        // 情報ページから中身だけを取り出し、CSS を埋めた 1 枚の HTML にする。
        let body = info
            .split_once("<body")
            .and_then(|(_, rest)| rest.split_once('>'))
            .map(|(_, rest)| rest.split("</body>").next().unwrap_or(rest))
            .unwrap_or(&info)
            .to_string();
        let class = if info.contains("class=\"info-page\"") {
            "info-page"
        } else {
            ""
        };
        let page = format!(
            "<!DOCTYPE html>\n<html lang=\"ja\"><head><meta charset=\"utf-8\" />\
             <title>{template_name} — 情報ページ</title><style>\n{css}\n</style></head>\
             <body class=\"{class}\">{body}</body></html>\n"
        );
        let path = out_dir.join(format!("{template_name}-info.html"));
        std::fs::write(&path, page).map_err(|error| error.to_string())?;
        println!("書き出しました: {}", path.display());
    }

    Ok(())
}
