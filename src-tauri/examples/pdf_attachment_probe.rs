//! 棚の実物で、添付からの取り出しを確かめる。
//!
//! 単体試験の材料はこちらで組み立てた最小の PDF で、作り手ごとの癖は入っていない。
//! **切り方の主張は実物でしか裏が取れない。** Word と Google ドキュメントが
//! 実際に書き出した PDF を通し、段落数・文字数・経路を目で見る。
//!
//! ```text
//! cargo run --example pdf_attachment_probe -- "<PDFの道>" ...
//! cargo run --example pdf_attachment_probe -- "<作品の版のフォルダ>"
//! ```
//!
//! 道を渡さなければ、既定の保管場所（`%APPDATA%/com.hiron.piep/downloads`）を
//! 掘って、見つかった PDF をすべて通す。
//!
//! `original.json` を含むフォルダを渡すと、**取り出しの先まで**見る。保存済みの
//! 投稿へ差し込み、組み上がったブロックと検索用の平文を数える。

use piep_lib::database::attachment::apply_attachment_text;
use piep_lib::database::models::AssetEntry;
use piep_lib::database::parser::parse_fanbox_value_to_html;
use piep_lib::database::search::extract_search_body;
use piep_lib::pdf::extract_text;
use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let targets: Vec<PathBuf> = if args.is_empty() {
        collect_from_library()
    } else {
        args.iter().map(PathBuf::from).collect()
    };
    if targets.is_empty() {
        eprintln!("PDF が見つからない。道を引数で渡すこと。");
        std::process::exit(1);
    }

    let dump = std::env::var("DUMP").is_ok();
    for path in targets {
        if path.is_dir() {
            probe_post(&path);
            continue;
        }
        let name = path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        let started = std::time::Instant::now();
        match extract_text(&path) {
            Ok(text) => {
                let blanks = text.paragraphs.iter().filter(|p| p.is_empty()).count();
                println!(
                    "{:.44}\n  経路={} ページ={} 段落={}（空行 {}） 文字={} {:?}",
                    name,
                    text.method.as_str(),
                    text.page_count,
                    text.paragraphs.len() - blanks,
                    blanks,
                    text.char_count,
                    started.elapsed()
                );
                if dump {
                    for paragraph in text.paragraphs.iter().take(8) {
                        println!("  | {paragraph}");
                    }
                }
                if let Some(dir) = std::env::var_os("RENDER") {
                    render_first_page(&path, &name, Path::new(&dir));
                }
            }
            Err(error) => println!("{:.44}\n  取り出せない: {error}", name),
        }
    }
}

/// 保存済みの投稿へ差し込むところまでを見る。
fn probe_post(dir: &Path) {
    let json_path = dir.join("original.json");
    let Ok(raw) = std::fs::read_to_string(&json_path) else {
        eprintln!("{} に original.json が無い", dir.display());
        return;
    };
    let mut data: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("{}: JSON を読めない: {error}", dir.display());
            return;
        }
    };
    let assets = assets_of(dir);
    let before = extract_search_body(&data, "fanbox").chars().count();
    let imported = apply_attachment_text(&mut data, "fanbox", &assets);
    let after = extract_search_body(&data, "fanbox").chars().count();
    let html = parse_fanbox_value_to_html(&data, &assets);
    println!("{}", dir.display());
    for record in &imported {
        println!(
            "  取り込み: {} 経路={} ページ={} 文字={}",
            record.file_name, record.method, record.page_count, record.char_count
        );
    }
    println!(
        "  検索の材料 {before} 文字 -> {after} 文字／本文 HTML {} バイト（区切り {} 個）",
        html.len(),
        html.matches("<!-- content-block -->").count()
    );
    if std::env::var("DUMP").is_ok() {
        for line in html.lines().take(20) {
            println!("  | {line}");
        }
    }
}

/// 保存済みのアセットを、DB を開かずに組み立てる。
fn assets_of(dir: &Path) -> Vec<AssetEntry> {
    let mut assets = Vec::new();
    let mut found = Vec::new();
    walk(&dir.join("data_assets"), 0, &mut found, None);
    for (index, path) in found.into_iter().enumerate() {
        assets.push(AssetEntry {
            id: index as i64 + 1,
            download_id: 0,
            asset_type: "file".to_string(),
            filename: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            local_path: path.to_string_lossy().to_string(),
            original_url: None,
            mime_type: match path.extension().and_then(|value| value.to_str()) {
                Some(value) if value.eq_ignore_ascii_case("pdf") => {
                    Some("application/pdf".to_string())
                }
                Some(value) if value.eq_ignore_ascii_case("txt") => Some("text/plain".to_string()),
                _ => None,
            },
            file_size_bytes: 0,
        });
    }
    assets
}

/// 原本表示のための描画を確かめる。`RENDER=<出力先>` を渡したときだけ走る。
fn render_first_page(path: &Path, name: &str, dir: &Path) {
    let started = std::time::Instant::now();
    match piep_lib::pdf::render_page(path, 0, 1200) {
        Ok(page) => {
            let elapsed = started.elapsed();
            let _ = std::fs::create_dir_all(dir);
            let stem: String = name.chars().take(20).collect();
            let out = dir.join(format!("{stem}-p1.webp"));
            match std::fs::write(&out, &page.webp) {
                Ok(()) => println!(
                    "  描画={}x{} {}KB {:?} -> {}",
                    page.width,
                    page.height,
                    page.webp.len() / 1024,
                    elapsed,
                    out.display()
                ),
                Err(error) => println!("  描画は出来たが書き出せない: {error}"),
            }
        }
        Err(error) => println!("  描けない: {error}"),
    }
}

fn collect_from_library() -> Vec<PathBuf> {
    let Some(base) = std::env::var_os("APPDATA").map(PathBuf::from) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    walk(
        &base.join("com.hiron.piep").join("downloads"),
        0,
        &mut found,
        Some("pdf"),
    );
    found.sort();
    found
}

/// `only` に拡張子を渡すとそれだけを、`None` ならすべてのファイルを集める。
///
/// **差し込みを見るときは PDF に絞ってはいけない。** 本文を抱えた添付には
/// `.txt` もあり、絞ったままだと「取り込むものが無かった」と区別が付かない。
fn walk(dir: &Path, depth: usize, found: &mut Vec<PathBuf>, only: Option<&str>) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, depth + 1, found, only);
            continue;
        }
        let keep = match only {
            None => true,
            Some(wanted) => path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case(wanted)),
        };
        if keep {
            found.push(path);
        }
    }
}
