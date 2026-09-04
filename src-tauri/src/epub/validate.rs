//! 生成した EPUB を開き直して検査する。
//!
//! 「書き出せた」と「読める本になった」は別のことで、これまでは前者しか
//! 確かめていなかった。取り込み側が黙って弾く原因 — 整形式でない XHTML、
//! 指す先の無い参照、重複した ID、書式の違う日時 — をここで捕まえ、
//! 出来上がった直後に利用者へ伝える。
//!
//! epubcheck の完全な代わりではないが、実際に本を壊す種類の不備は網羅する。

use crate::epub::intermediate::{EpubValidationIssue, EpubValidationReport};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

// Validation is also exposed for user-selected EPUB files, so none of these
// limits may rely on the archive having been produced by our own builder.
// Kindle delivery is capped at 200 MiB compressed; 512 MiB expanded leaves
// ample room for normal image compression while bounding validation memory.
const MAX_EPUB_ENTRIES: usize = 10_000;
const MAX_EPUB_ENTRY_BYTES: u64 = 256 * 1024 * 1024;
const MAX_EPUB_EXPANDED_BYTES: u64 = 512 * 1024 * 1024;
const MAX_EPUB_COMPRESSION_RATIO: u64 = 250;

pub fn validate_epub(path: &Path) -> Result<EpubValidationReport, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("EPUBを開けません: {}", e))?;
    let file_size_bytes = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("ZIPとして読めません: {}", e))?;

    if archive.len() > MAX_EPUB_ENTRIES {
        return Err(format!(
            "EPUB内のファイル数が安全上限を超えています（{}件 / 上限{}件）",
            archive.len(),
            MAX_EPUB_ENTRIES
        ));
    }

    let mut issues = Vec::new();
    let mut entries: HashMap<String, Vec<u8>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut expanded_bytes = 0u64;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| format!("ZIP項目の読み込みに失敗: {}", e))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_string();
        let entry_bytes = entry.size();
        if entry_bytes > MAX_EPUB_ENTRY_BYTES {
            return Err(format!(
                "EPUB内のファイルが安全上限を超えています: '{}'（{} bytes）",
                name, entry_bytes
            ));
        }
        expanded_bytes = expanded_bytes
            .checked_add(entry_bytes)
            .ok_or_else(|| "EPUBの展開サイズが計算可能な範囲を超えています".to_string())?;
        if expanded_bytes > MAX_EPUB_EXPANDED_BYTES {
            return Err(format!(
                "EPUBの展開サイズが安全上限を超えています（{} bytes）",
                expanded_bytes
            ));
        }
        let compressed_bytes = entry.compressed_size();
        if compressed_bytes > 0
            && entry_bytes / compressed_bytes.max(1) > MAX_EPUB_COMPRESSION_RATIO
        {
            return Err(format!("EPUB内のファイルの圧縮率が異常です: '{}'", name));
        }
        if index == 0 && name == "mimetype" && entry.compression() != zip::CompressionMethod::Stored
        {
            issues.push(EpubValidationIssue::error(
                "OCF-MIMETYPE-COMPRESSED",
                "mimetype",
                "mimetype は無圧縮で格納しなければなりません",
            ));
        }
        // Presence checks need every path, but validation only inspects
        // container/package/document text. Keeping every JPEG/PNG payload in
        // this map made image-heavy books consume their full expanded size.
        let mut bytes = Vec::new();
        if retain_entry_contents(&name) {
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| format!("'{}' の読み込みに失敗: {}", name, e))?;
        }
        order.push(name.clone());
        if entries.insert(name.clone(), bytes).is_some() {
            issues.push(EpubValidationIssue::error(
                "OCF-DUPLICATE-ENTRY",
                &name,
                "ZIP 内で同じファイル名が重複しています",
            ));
        }
    }

    check_ocf(&order, &entries, &mut issues);
    let package_path = rootfile_path(&entries).unwrap_or_else(|| "OEBPS/content.opf".to_string());
    hydrate_cover_images(&mut archive, &package_path, &mut entries)?;
    check_package(&package_path, &entries, &mut issues);
    check_documents(&entries, &mut issues);
    check_kindle_limits(file_size_bytes, &entries, &mut issues);

    Ok(EpubValidationReport {
        path: path.to_string_lossy().to_string(),
        valid: !issues.iter().any(|issue| issue.severity == "error"),
        file_size_bytes,
        issues,
    })
}

fn retain_entry_contents(name: &str) -> bool {
    if name == "mimetype" {
        return true;
    }
    let lowercase = name.to_ascii_lowercase();
    [".xml", ".opf", ".xhtml", ".html", ".ncx"]
        .iter()
        .any(|extension| lowercase.ends_with(extension))
}

fn hydrate_cover_images(
    archive: &mut zip::ZipArchive<std::fs::File>,
    package_path: &str,
    entries: &mut HashMap<String, Vec<u8>>,
) -> Result<(), String> {
    let Some(opf) = entries.get(package_path).and_then(|bytes| text_of(bytes)) else {
        return Ok(());
    };
    let base_dir = package_path
        .rsplit_once('/')
        .map(|(directory, _)| directory)
        .unwrap_or("");
    let covers = parse_manifest_items(&opf)
        .into_iter()
        .filter(|item| {
            item.properties
                .as_deref()
                .is_some_and(|value| value.split_whitespace().any(|token| token == "cover-image"))
        })
        .map(|item| join_path(base_dir, &decode_path(&item.href)))
        .collect::<Vec<_>>();
    for cover in covers {
        if !entries.get(&cover).is_some_and(Vec::is_empty) {
            continue;
        }
        let mut entry = archive
            .by_name(&cover)
            .map_err(|error| format!("表紙画像 '{}' の読み込みに失敗: {error}", cover))?;
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| format!("表紙画像 '{}' の読み込みに失敗: {error}", cover))?;
        entries.insert(cover, bytes);
    }
    Ok(())
}

// ============================================================
// 容器 (OCF)
// ============================================================

fn check_ocf(
    order: &[String],
    entries: &HashMap<String, Vec<u8>>,
    issues: &mut Vec<EpubValidationIssue>,
) {
    match order.first() {
        Some(first) if first == "mimetype" => {}
        _ => issues.push(EpubValidationIssue::error(
            "OCF-MIMETYPE-POSITION",
            "mimetype",
            "mimetype は書庫の先頭になければなりません",
        )),
    }
    match entries.get("mimetype").map(|bytes| bytes.as_slice()) {
        Some(b"application/epub+zip") => {}
        Some(_) => issues.push(EpubValidationIssue::error(
            "OCF-MIMETYPE-CONTENT",
            "mimetype",
            "mimetype の内容が application/epub+zip ではありません",
        )),
        None => issues.push(EpubValidationIssue::error(
            "OCF-MIMETYPE-MISSING",
            "mimetype",
            "mimetype がありません",
        )),
    }
    if !entries.contains_key("META-INF/container.xml") {
        issues.push(EpubValidationIssue::error(
            "OCF-CONTAINER-MISSING",
            "META-INF/container.xml",
            "container.xml がありません",
        ));
    }
}

fn rootfile_path(entries: &HashMap<String, Vec<u8>>) -> Option<String> {
    let container = text_of(entries.get("META-INF/container.xml")?)?;
    let at = container.find("full-path=")?;
    let rest = &container[at + "full-path=".len()..];
    // 直後の1文字を「引用符（1バイト）」と決め打ちして `rest[1..]` を切っていた。
    // そこにマルチバイト文字が来ると文字の途中を切ることになり、**パニックする**。
    // release は `panic = "abort"` なので、その場でプロセスが死ぬ。検証器は
    // 他人が作った EPUB を読む道具なので、中身が何であれ落ちてはいけない。
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let end = rest[1..].find(quote)? + 1;
    Some(rest[1..end].to_string())
}

// ============================================================
// パッケージ文書 (OPF)
// ============================================================

fn check_package(
    package_path: &str,
    entries: &HashMap<String, Vec<u8>>,
    issues: &mut Vec<EpubValidationIssue>,
) {
    let Some(opf) = entries.get(package_path).and_then(|bytes| text_of(bytes)) else {
        issues.push(EpubValidationIssue::error(
            "OPF-MISSING",
            package_path,
            "パッケージ文書が見つかりません",
        ));
        return;
    };
    if let Err(error) = check_well_formed(&opf) {
        issues.push(EpubValidationIssue::error(
            "OPF-NOT-WELL-FORMED",
            package_path,
            format!("パッケージ文書が整形式ではありません: {}", error),
        ));
        return;
    }

    let base_dir = package_path
        .rsplit_once('/')
        .map(|(dir, _)| dir.to_string())
        .unwrap_or_default();

    // 必須のメタデータ
    for (needle, code, message) in [
        ("<dc:title", "OPF-TITLE-MISSING", "dc:title がありません"),
        (
            "<dc:language",
            "OPF-LANGUAGE-MISSING",
            "dc:language がありません",
        ),
        (
            "<dc:identifier",
            "OPF-IDENTIFIER-MISSING",
            "dc:identifier がありません",
        ),
    ] {
        if !opf.contains(needle) {
            issues.push(EpubValidationIssue::error(code, package_path, message));
        }
    }

    // 有るだけでは足りない。テンプレートの綴り違いは、その場でも組み立てでも
    // 検証でも何も言われないまま、**題の空いた本**を「検証合格」で通していた。
    for (needle, code, label) in [
        ("<dc:title", "OPF-TITLE-EMPTY", "dc:title"),
        ("<dc:identifier", "OPF-IDENTIFIER-EMPTY", "dc:identifier"),
        ("<dc:language", "OPF-LANGUAGE-EMPTY", "dc:language"),
    ] {
        if opf.contains(needle) && !has_non_empty_element(&opf, needle) {
            issues.push(EpubValidationIssue::error(
                code,
                package_path,
                format!("{label} が空です。テンプレートの差し込みを確かめてください"),
            ));
        }
    }
    if opf.contains("<dc:creator") && !has_non_empty_element(&opf, "<dc:creator") {
        issues.push(EpubValidationIssue::warning(
            "OPF-CREATOR-EMPTY",
            package_path,
            "dc:creator が空です",
        ));
    }

    check_modified(&opf, package_path, issues);
    check_unique_identifier(&opf, package_path, issues);

    // マニフェスト
    let items = parse_manifest_items(&opf);
    if items.is_empty() {
        issues.push(EpubValidationIssue::error(
            "OPF-MANIFEST-EMPTY",
            package_path,
            "マニフェストに項目がありません",
        ));
    }
    let mut seen_ids: HashSet<&str> = HashSet::new();
    let mut hrefs: HashSet<String> = HashSet::new();
    let mut nav_ids = Vec::new();
    let mut nav_paths = Vec::new();
    let mut cover_paths = Vec::new();
    for item in &items {
        if !seen_ids.insert(item.id.as_str()) {
            issues.push(EpubValidationIssue::error(
                "OPF-ID-DUPLICATE",
                package_path,
                format!("マニフェストの id '{}' が重複しています", item.id),
            ));
        }
        if !is_ncname(&item.id) {
            issues.push(EpubValidationIssue::error(
                "OPF-ID-INVALID",
                package_path,
                format!("id '{}' は XML の名前として使えません", item.id),
            ));
        }
        if item
            .properties
            .as_deref()
            .is_some_and(|value| value.split_whitespace().any(|token| token == "nav"))
        {
            nav_ids.push(item.id.clone());
        }
        let resolved = join_path(&base_dir, &decode_path(&item.href));
        if item.href.starts_with("http://") || item.href.starts_with("https://") {
            issues.push(EpubValidationIssue::warning(
                "OPF-REMOTE-RESOURCE",
                package_path,
                format!("外部の資源を参照しています: {}", item.href),
            ));
            continue;
        }
        if !entries.contains_key(&resolved) {
            issues.push(EpubValidationIssue::error(
                "OPF-ITEM-MISSING",
                package_path,
                format!("マニフェストが指すファイルがありません: {}", item.href),
            ));
        }
        if !hrefs.insert(resolved.clone()) {
            issues.push(EpubValidationIssue::error(
                "OPF-HREF-DUPLICATE",
                package_path,
                format!("同じ資源がマニフェストに複数回あります: {}", item.href),
            ));
        }
        if item
            .properties
            .as_deref()
            .is_some_and(|value| value.split_whitespace().any(|token| token == "nav"))
        {
            nav_paths.push(resolved);
        }
        if item
            .properties
            .as_deref()
            .is_some_and(|value| value.split_whitespace().any(|token| token == "cover-image"))
        {
            cover_paths.push(join_path(&base_dir, &decode_path(&item.href)));
        }
    }
    match nav_ids.len() {
        1 => {}
        0 => issues.push(EpubValidationIssue::error(
            "OPF-NAV-MISSING",
            package_path,
            "properties=\"nav\" を持つナビゲーション文書がありません",
        )),
        count => issues.push(EpubValidationIssue::error(
            "OPF-NAV-DUPLICATE",
            package_path,
            format!("ナビゲーション文書は1つだけ必要です（{count}個あります）"),
        )),
    }

    // 背表紙 (spine)
    let idrefs = parse_spine_idrefs(&opf);
    if idrefs.is_empty() {
        issues.push(EpubValidationIssue::error(
            "OPF-SPINE-EMPTY",
            package_path,
            "spine に項目がありません",
        ));
    }
    for idref in &idrefs {
        if !seen_ids.contains(idref.as_str()) {
            issues.push(EpubValidationIssue::error(
                "OPF-SPINE-UNRESOLVED",
                package_path,
                format!("spine の idref '{}' がマニフェストにありません", idref),
            ));
        }
    }
    for nav_id in &nav_ids {
        if !idrefs.contains(nav_id) {
            issues.push(EpubValidationIssue::error(
                "KINDLE-NAV-NOT-SPINE",
                package_path,
                "Kindle互換のHTML目次を表示するため、nav文書をspineに含めてください",
            ));
        }
    }

    for nav_path in nav_paths {
        let Some(nav) = entries.get(&nav_path).and_then(|bytes| text_of(bytes)) else {
            continue;
        };
        let toc_count = count_toc_navs(&nav);
        if toc_count != 1 {
            issues.push(EpubValidationIssue::error(
                "NAV-TOC-CARDINALITY",
                &nav_path,
                format!("epub:type=\"toc\" のnav要素は1つ必要です（{toc_count}個あります）"),
            ));
        }
    }

    match cover_paths.as_slice() {
        [] => issues.push(EpubValidationIssue::warning(
            "KINDLE-COVER-MISSING",
            package_path,
            "Kindle向けの内部表紙画像がありません",
        )),
        [cover] => check_cover_image(cover, entries, issues),
        covers => issues.push(EpubValidationIssue::error(
            "OPF-COVER-DUPLICATE",
            package_path,
            format!(
                "cover-image は1つだけ指定できます（{}個あります）",
                covers.len()
            ),
        )),
    }

    // 本文が参照する資源はすべてマニフェストに載っていなければならない
    for (name, bytes) in entries {
        if !name.ends_with(".xhtml") {
            continue;
        }
        let Some(text) = text_of(bytes) else { continue };
        let dir = name.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        for reference in local_references(&text) {
            let resolved = join_path(dir, &decode_path(&reference));
            if !entries.contains_key(&resolved) {
                issues.push(EpubValidationIssue::error(
                    "XHTML-REFERENCE-MISSING",
                    name,
                    format!("参照先のファイルがありません: {}", reference),
                ));
            } else if !hrefs.contains(&resolved) {
                issues.push(EpubValidationIssue::error(
                    "XHTML-REFERENCE-UNDECLARED",
                    name,
                    format!("参照先がマニフェストに載っていません: {}", reference),
                ));
            }
        }
    }
}

fn check_modified(opf: &str, location: &str, issues: &mut Vec<EpubValidationIssue>) {
    let values: Vec<&str> = opf
        .match_indices("property=\"dcterms:modified\"")
        .filter_map(|(at, _)| {
            let rest = &opf[at..];
            let start = rest.find('>')? + 1;
            let end = rest[start..].find('<')? + start;
            Some(rest[start..end].trim())
        })
        .collect();
    match values.len() {
        1 => {
            let value = values[0];
            // CCYY-MM-DDThh:mm:ssZ の 20 文字ちょうどでなければならない。
            let shaped = value.len() == 20
                && value.ends_with('Z')
                && value.as_bytes()[10] == b'T'
                && value.chars().enumerate().all(|(index, ch)| match index {
                    4 | 7 => ch == '-',
                    10 => ch == 'T',
                    13 | 16 => ch == ':',
                    19 => ch == 'Z',
                    _ => ch.is_ascii_digit(),
                });
            if !shaped {
                issues.push(EpubValidationIssue::error(
                    "OPF-MODIFIED-FORMAT",
                    location,
                    format!("dcterms:modified は CCYY-MM-DDThh:mm:ssZ の形式が必要です: {value}"),
                ));
            }
        }
        0 => issues.push(EpubValidationIssue::error(
            "OPF-MODIFIED-MISSING",
            location,
            "dcterms:modified がありません",
        )),
        count => issues.push(EpubValidationIssue::error(
            "OPF-MODIFIED-DUPLICATE",
            location,
            format!("dcterms:modified は 1 つだけにしてください（{count} 個あります）"),
        )),
    }
}

fn check_unique_identifier(opf: &str, location: &str, issues: &mut Vec<EpubValidationIssue>) {
    let Some(unique) = attribute_value(opf, "unique-identifier") else {
        issues.push(EpubValidationIssue::error(
            "OPF-UNIQUE-IDENTIFIER-MISSING",
            location,
            "package に unique-identifier がありません",
        ));
        return;
    };
    let resolved = opf
        .match_indices("<dc:identifier")
        .filter_map(|(at, _)| {
            let rest = &opf[at..];
            let close = rest.find('>')?;
            let tag = &rest[..close];
            let id = attribute_value(tag, "id")?;
            let start = close + 1;
            let end = rest[start..].find('<')? + start;
            (id == unique).then(|| rest[start..end].trim().to_string())
        })
        .next();
    match resolved {
        Some(value) if !value.is_empty() => {}
        Some(_) => issues.push(EpubValidationIssue::error(
            "OPF-IDENTIFIER-EMPTY",
            location,
            "unique-identifier が指す dc:identifier が空です",
        )),
        None => issues.push(EpubValidationIssue::error(
            "OPF-UNIQUE-IDENTIFIER-UNRESOLVED",
            location,
            format!("unique-identifier '{unique}' に対応する dc:identifier がありません"),
        )),
    }
}

// ============================================================
// XHTML 文書
// ============================================================

fn check_documents(entries: &HashMap<String, Vec<u8>>, issues: &mut Vec<EpubValidationIssue>) {
    let mut names: Vec<&String> = entries
        .keys()
        .filter(|name| name.ends_with(".xhtml") || name.ends_with(".ncx"))
        .collect();
    names.sort();
    for name in names {
        let Some(text) = entries.get(name).and_then(|bytes| text_of(bytes)) else {
            issues.push(EpubValidationIssue::error(
                "XHTML-ENCODING",
                name,
                "UTF-8 として読めません",
            ));
            continue;
        };
        if let Err(error) = check_well_formed(&text) {
            issues.push(EpubValidationIssue::error(
                "XHTML-NOT-WELL-FORMED",
                name,
                format!("整形式ではありません: {}", error),
            ));
        }
        if name.ends_with(".ncx") {
            check_ncx_order(&text, name, issues);
        }
    }
}

fn check_kindle_limits(
    file_size_bytes: u64,
    entries: &HashMap<String, Vec<u8>>,
    issues: &mut Vec<EpubValidationIssue>,
) {
    const SEND_TO_KINDLE_LIMIT: u64 = 200 * 1024 * 1024;
    const HTML_LIMIT: usize = 30 * 1024 * 1024;
    if file_size_bytes > SEND_TO_KINDLE_LIMIT {
        issues.push(EpubValidationIssue::error(
            "KINDLE-FILE-TOO-LARGE",
            "EPUB",
            "Send to Kindle の上限 200 MB を超えています",
        ));
    }
    let html: Vec<_> = entries
        .iter()
        .filter(|(name, _)| name.ends_with(".xhtml") || name.ends_with(".html"))
        .collect();
    if html.len() >= 300 {
        issues.push(EpubValidationIssue::error(
            "KINDLE-HTML-COUNT",
            "EPUB",
            format!("Kindleの上限はHTML 300未満です（{}件あります）", html.len()),
        ));
    }
    for (name, bytes) in html {
        if bytes.len() >= HTML_LIMIT {
            issues.push(EpubValidationIssue::error(
                "KINDLE-HTML-TOO-LARGE",
                name,
                "Kindleの上限 30 MB 未満になるよう本文を分割してください",
            ));
        }
    }
}

fn check_cover_image(
    path: &str,
    entries: &HashMap<String, Vec<u8>>,
    issues: &mut Vec<EpubValidationIssue>,
) {
    let Some(bytes) = entries.get(path) else {
        return;
    };
    let dimensions = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.into_dimensions().ok());
    if dimensions.is_some_and(|(width, height)| width.max(height) < 1200) {
        let (width, height) = dimensions.unwrap();
        issues.push(EpubValidationIssue::warning(
            "KINDLE-COVER-SMALL",
            path,
            format!(
                "内部表紙は幅または高さ1200px以上が推奨です（{}×{}px）",
                width, height
            ),
        ));
    }
}

fn count_toc_navs(document: &str) -> usize {
    let regex = regex::Regex::new(r#"(?is)<nav\b[^>]*\bepub:type\s*=\s*(?:\"toc\"|'toc')[^>]*>"#)
        .expect("fixed regex");
    regex.find_iter(document).count()
}

fn check_ncx_order(ncx: &str, location: &str, issues: &mut Vec<EpubValidationIssue>) {
    let regex = regex::Regex::new(r#"(?i)\bplayOrder\s*=\s*\"([0-9]+)\""#).expect("fixed regex");
    let orders: Vec<u32> = regex
        .captures_iter(ncx)
        .filter_map(|capture| capture[1].parse().ok())
        .collect();
    if orders.is_empty() {
        return;
    }
    let mut sorted = orders.clone();
    sorted.sort_unstable();
    sorted.dedup();
    let expected: Vec<u32> = (1..=orders.len() as u32).collect();
    if sorted != expected {
        issues.push(EpubValidationIssue::error(
            "NCX-PLAYORDER-INVALID",
            location,
            "NCX の playOrder は重複や欠番のない 1 からの連番にしてください",
        ));
    }
}

/// 本文から参照される、書庫内のファイル。
fn local_references(text: &str) -> Vec<String> {
    let mut references = Vec::new();
    for attribute in ["src=\"", "href=\"", "xlink:href=\""] {
        let mut cursor = 0;
        while let Some(at) = text[cursor..].find(attribute) {
            let start = cursor + at + attribute.len();
            let Some(end_offset) = text[start..].find('"') else {
                break;
            };
            let value = &text[start..start + end_offset];
            cursor = start + end_offset;
            let target = value.split('#').next().unwrap_or(value);
            if target.is_empty()
                || target.contains("://")
                || target.starts_with("mailto:")
                || target.starts_with("data:")
            {
                continue;
            }
            references.push(target.to_string());
        }
    }
    references
}

// ============================================================
// XML の整形式チェック
// ============================================================

/// 最小限の整形式判定。要素の対応、属性の引用、実体参照、不正文字を見る。
pub fn check_well_formed(xml: &str) -> Result<(), String> {
    let chars: Vec<char> = xml.chars().collect();
    let mut stack: Vec<String> = Vec::new();
    let mut index = 0;
    let mut saw_root = false;

    while index < chars.len() {
        if chars[index] != '<' {
            if !is_allowed_char(chars[index]) {
                return Err(format!(
                    "XML に置けない文字が含まれています (U+{:04X})",
                    chars[index] as u32
                ));
            }
            if chars[index] == '&' {
                index = check_entity(&chars, index)?;
                continue;
            }
            index += 1;
            continue;
        }
        if starts_with(&chars, index, "<!--") {
            index = skip_to(&chars, index + 4, "-->")
                .ok_or_else(|| "閉じていないコメントがあります".to_string())?;
            continue;
        }
        if starts_with(&chars, index, "<![CDATA[") {
            index = skip_to(&chars, index + 9, "]]>")
                .ok_or_else(|| "閉じていない CDATA があります".to_string())?;
            continue;
        }
        if starts_with(&chars, index, "<?") {
            index = skip_to(&chars, index + 2, "?>")
                .ok_or_else(|| "閉じていない処理命令があります".to_string())?;
            continue;
        }
        if starts_with(&chars, index, "<!") {
            index = skip_to(&chars, index + 2, ">")
                .ok_or_else(|| "閉じていない宣言があります".to_string())?;
            continue;
        }

        let closing = chars.get(index + 1) == Some(&'/');
        let mut cursor = index + if closing { 2 } else { 1 };
        let name_start = cursor;
        while cursor < chars.len() && is_name_char(chars[cursor]) {
            cursor += 1;
        }
        if cursor == name_start {
            return Err("'<' の後ろに要素名がありません".to_string());
        }
        let name: String = chars[name_start..cursor].iter().collect();

        if closing {
            match stack.pop() {
                Some(open) if open == name => {}
                Some(open) => {
                    return Err(format!("</{}> の位置に </{}> が必要です", name, open));
                }
                None => return Err(format!("対応する開始タグの無い </{}> があります", name)),
            }
            cursor = skip_to(&chars, cursor, ">")
                .ok_or_else(|| format!("</{}> が閉じていません", name))?;
            index = cursor;
            continue;
        }

        // 属性
        let mut self_closing = false;
        let mut seen_attrs: HashSet<String> = HashSet::new();
        loop {
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                cursor += 1;
            }
            match chars.get(cursor) {
                None => return Err(format!("<{}> が閉じていません", name)),
                Some('>') => {
                    cursor += 1;
                    break;
                }
                Some('/') if chars.get(cursor + 1) == Some(&'>') => {
                    self_closing = true;
                    cursor += 2;
                    break;
                }
                _ => {}
            }
            let key_start = cursor;
            while cursor < chars.len() && is_name_char(chars[cursor]) {
                cursor += 1;
            }
            if cursor == key_start {
                return Err(format!("<{}> の属性が読めません", name));
            }
            let key: String = chars[key_start..cursor].iter().collect();
            if !seen_attrs.insert(key.clone()) {
                return Err(format!("<{}> に属性 '{}' が二度あります", name, key));
            }
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                cursor += 1;
            }
            if chars.get(cursor) != Some(&'=') {
                return Err(format!("属性 '{}' に値がありません", key));
            }
            cursor += 1;
            while cursor < chars.len() && chars[cursor].is_whitespace() {
                cursor += 1;
            }
            let Some(quote) = chars
                .get(cursor)
                .copied()
                .filter(|ch| *ch == '"' || *ch == '\'')
            else {
                return Err(format!("属性 '{}' の値が引用符で囲まれていません", key));
            };
            cursor += 1;
            let value_start = cursor;
            while cursor < chars.len() && chars[cursor] != quote {
                if chars[cursor] == '<' {
                    return Err(format!("属性 '{}' の値に生の '<' があります", key));
                }
                cursor += 1;
            }
            if cursor >= chars.len() {
                return Err(format!("属性 '{}' の値が閉じていません", key));
            }
            let mut scan = value_start;
            while scan < cursor {
                if chars[scan] == '&' {
                    scan = check_entity(&chars, scan)?;
                    continue;
                }
                scan += 1;
            }
            cursor += 1;
        }

        if !self_closing {
            stack.push(name);
        } else if stack.is_empty() {
            saw_root = true;
        }
        if stack.len() == 1 {
            saw_root = true;
        }
        index = cursor;
    }

    if let Some(open) = stack.last() {
        return Err(format!("<{}> が閉じられていません", open));
    }
    if !saw_root {
        return Err("要素がありません".to_string());
    }
    Ok(())
}

/// `&` から始まる実体参照を検査し、その次の位置を返す。
fn check_entity(chars: &[char], at: usize) -> Result<usize, String> {
    let mut cursor = at + 1;
    let start = cursor;
    while cursor < chars.len() && chars[cursor] != ';' && cursor - start < 32 {
        cursor += 1;
    }
    if chars.get(cursor) != Some(&';') {
        return Err("エスケープされていない '&' があります".to_string());
    }
    let body: String = chars[start..cursor].iter().collect();
    let known = matches!(body.as_str(), "amp" | "lt" | "gt" | "quot" | "apos");
    let numeric = body
        .strip_prefix('#')
        .map(|digits| match digits.strip_prefix(['x', 'X']) {
            Some(hex) => !hex.is_empty() && hex.chars().all(|ch| ch.is_ascii_hexdigit()),
            None => !digits.is_empty() && digits.chars().all(|ch| ch.is_ascii_digit()),
        })
        .unwrap_or(false);
    if !known && !numeric {
        return Err(format!(
            "XHTML では定義されていない実体参照です: &{};",
            body
        ));
    }
    Ok(cursor + 1)
}

fn starts_with(chars: &[char], at: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(offset, ch)| chars.get(at + offset) == Some(&ch))
}

fn skip_to(chars: &[char], from: usize, needle: &str) -> Option<usize> {
    let mut index = from;
    while index < chars.len() {
        if starts_with(chars, index, needle) {
            return Some(index + needle.chars().count());
        }
        index += 1;
    }
    None
}

fn is_name_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':')
}

/// XML 1.0 の Char。**上限も見る。**
///
/// 下限しか見ていなかったので、U+FFFE / U+FFFF のような非文字を通していた。
/// 組み立て側（`xhtml::is_xml_char`）は正しく上限を持っている。検証器のほうが
/// 緩いと、「別の目で見る」という役目を果たせない。
fn is_allowed_char(ch: char) -> bool {
    matches!(ch, '\t' | '\n' | '\r')
        || matches!(ch, ' '..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

fn is_ncname(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.'))
}

// ============================================================
// 小さなヘルパー
// ============================================================

fn text_of(bytes: &[u8]) -> Option<String> {
    String::from_utf8(bytes.to_vec()).ok()
}

/// その要素が、中身のある文字を持っているか。
///
/// 同じ名前の要素がいくつあっても、一つでも中身があればよい（`dc:title` が
/// 副題を伴って二つ書かれることがある）。
fn has_non_empty_element(xml: &str, open_tag: &str) -> bool {
    let name = open_tag.trim_start_matches('<');
    let mut rest = xml;
    while let Some(start) = rest.find(open_tag) {
        let after_tag = &rest[start + open_tag.len()..];
        let Some(open_end) = after_tag.find('>') else {
            return false;
        };
        // 自己終了タグは中身を持たない。
        if after_tag[..open_end].trim_end().ends_with('/') {
            rest = &after_tag[open_end + 1..];
            continue;
        }
        let body = &after_tag[open_end + 1..];
        let close = format!("</{name}>");
        if let Some(end) = body.find(&close) {
            if !body[..end].trim().is_empty() {
                return true;
            }
            rest = &body[end + close.len()..];
        } else {
            return false;
        }
    }
    false
}

fn attribute_value(text: &str, key: &str) -> Option<String> {
    let needle = format!("{}=\"", key);
    let at = text.find(&needle)?;
    let start = at + needle.len();
    let end = text[start..].find('"')? + start;
    Some(text[start..end].to_string())
}

/// XML の文字参照を戻す。`&#x2f;` のような書き方でも参照は同じ場所を指す。
fn decode_xml_entities(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let resolved = tail.find(';').and_then(|end| {
            let body = &tail[1..end];
            let ch = match body {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => {
                    let digits = body.strip_prefix('#')?;
                    let code = match digits.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => digits.parse::<u32>().ok()?,
                    };
                    char::from_u32(code)
                }
            }?;
            Some((ch, end + 1))
        });
        match resolved {
            Some((ch, next)) => {
                out.push(ch);
                rest = &tail[next..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_path(href: &str) -> String {
    let href = &decode_xml_entities(href);
    let bytes = href.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// `..` を畳んで書庫内の絶対パスにする。
fn join_path(base: &str, relative: &str) -> String {
    let mut parts: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

struct OpfItem {
    id: String,
    href: String,
    properties: Option<String>,
}

fn parse_manifest_items(opf: &str) -> Vec<OpfItem> {
    let Some(start) = opf.find("<manifest") else {
        return Vec::new();
    };
    let end = opf[start..]
        .find("</manifest>")
        .map(|offset| start + offset)
        .unwrap_or(opf.len());
    opf[start..end]
        .match_indices("<item ")
        .filter_map(|(at, _)| {
            let rest = &opf[start + at..end];
            let close = rest.find('>')?;
            let tag = &rest[..close];
            Some(OpfItem {
                id: attribute_value(tag, "id")?,
                href: attribute_value(tag, "href")?,
                properties: attribute_value(tag, "properties"),
            })
        })
        .collect()
}

fn parse_spine_idrefs(opf: &str) -> Vec<String> {
    let Some(start) = opf.find("<spine") else {
        return Vec::new();
    };
    let end = opf[start..]
        .find("</spine>")
        .map(|offset| start + offset)
        .unwrap_or(opf.len());
    opf[start..end]
        .match_indices("<itemref")
        .filter_map(|(at, _)| {
            let rest = &opf[start + at..end];
            let close = rest.find('>')?;
            attribute_value(&rest[..close], "idref")
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    #[test]
    fn image_payloads_are_not_retained_unless_validation_needs_them() {
        assert!(super::retain_entry_contents("mimetype"));
        assert!(super::retain_entry_contents("OEBPS/content.opf"));
        assert!(super::retain_entry_contents("OEBPS/text/chapter.XHTML"));
        assert!(!super::retain_entry_contents("OEBPS/images/page-001.jpg"));
        assert!(!super::retain_entry_contents("OEBPS/fonts/book.woff2"));
    }

    /// 他人が作った EPUB を読むための道具なので、中身が何であれ落ちない。
    ///
    /// `full-path=` の直後を「引用符（1バイト）」と決め打ちして切っていたので、
    /// そこにマルチバイト文字が来ると文字の途中を切ってパニックした。
    /// release は `panic = "abort"` なので、その場でプロセスが死ぬ。
    #[test]
    fn a_multibyte_character_after_full_path_is_refused_not_fatal() {
        let mut entries: HashMap<String, Vec<u8>> = HashMap::new();
        entries.insert(
            "META-INF/container.xml".to_string(),
            "<rootfile full-path=\u{201C}OEBPS/content.opf\u{201D} media-type=\"x\"/>"
                .as_bytes()
                .to_vec(),
        );
        assert_eq!(super::rootfile_path(&entries), None);
    }

    #[test]
    fn an_ordinary_container_still_resolves() {
        let mut entries: HashMap<String, Vec<u8>> = HashMap::new();
        entries.insert(
            "META-INF/container.xml".to_string(),
            "<rootfile full-path=\"OEBPS/content.opf\" media-type=\"x\"/>"
                .as_bytes()
                .to_vec(),
        );
        assert_eq!(
            super::rootfile_path(&entries).as_deref(),
            Some("OEBPS/content.opf")
        );
    }

    /// XML 1.0 の Char には上限もある。組み立て側は守っているのに検証側が
    /// 通していたら、「別の目で見る」という役目を果たせない。
    #[test]
    fn xml_noncharacters_are_not_allowed() {
        assert!(super::is_allowed_char('あ'));
        assert!(super::is_allowed_char('\u{FFFD}'));
        assert!(!super::is_allowed_char('\u{FFFE}'));
        assert!(!super::is_allowed_char('\u{FFFF}'));
        assert!(!super::is_allowed_char('\u{0007}'));
    }

    use super::*;
    use std::io::Write;

    fn test_epub_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "piep_epub_validation_{label}_{}.epub",
            rand::random::<u32>()
        ))
    }

    #[test]
    fn well_formed_documents_pass() {
        assert!(check_well_formed("<?xml version=\"1.0\"?><a><b x=\"1\" /></a>").is_ok());
        assert!(check_well_formed("<a><!-- c --><![CDATA[<raw>]]>text</a>").is_ok());
        assert!(check_well_formed("<a>&amp;&#x3042;&#12354;</a>").is_ok());
    }

    #[test]
    fn the_failures_that_break_real_epubs_are_caught() {
        // タイトルの & がエスケープされないまま OPF に入る — 最も多い壊れ方。
        assert!(check_well_formed("<a>Q&A</a>").is_err());
        // XHTML に &nbsp; は定義がない。
        assert!(check_well_formed("<a>a&nbsp;b</a>").is_err());
        assert!(check_well_formed("<a><br></a>").is_err());
        assert!(check_well_formed("<b><i>x</b></i>").is_err());
        assert!(check_well_formed("<a x=1></a>").is_err());
        assert!(check_well_formed("<a x=\"1\" x=\"2\"></a>").is_err());
        assert!(check_well_formed("<a>text").is_err());
        assert!(check_well_formed("text only").is_err());
    }

    /// 差し込みの綴りを間違えたテンプレートは、保存でも組み立てでも何も
    /// 言われない。せめて出来上がった本を見たときに、題の空きに気づくこと。
    #[test]
    fn an_empty_title_is_not_a_valid_book() {
        assert!(has_non_empty_element(
            r#"<dc:title id="title">雨の日</dc:title>"#,
            "<dc:title"
        ));
        assert!(!has_non_empty_element(
            r#"<dc:title id="title"></dc:title>"#,
            "<dc:title"
        ));
        assert!(!has_non_empty_element(
            "<dc:title id=\"title\">\n   \n</dc:title>",
            "<dc:title"
        ));
        // 副題つきで二つ書かれていても、中身があれば通す。
        assert!(has_non_empty_element(
            "<dc:title></dc:title><dc:title>副題</dc:title>",
            "<dc:title"
        ));
        // 閉じていないものは、中身があるとは言えない。
        assert!(!has_non_empty_element("<dc:title>途中", "<dc:title"));
    }

    #[test]
    fn paths_resolve_the_way_a_reader_resolves_them() {
        assert_eq!(
            join_path("OEBPS/text", "../images/a.jpg"),
            "OEBPS/images/a.jpg"
        );
        assert_eq!(
            join_path("OEBPS", "text/page.xhtml"),
            "OEBPS/text/page.xhtml"
        );
        assert_eq!(decode_path("../images/%E8%A1%A8.jpg"), "../images/表.jpg");
        // 文字参照で書かれた区切りも同じ場所を指す。
        assert_eq!(decode_path("text&#x2f;info.xhtml"), "text/info.xhtml");
        assert_eq!(decode_xml_entities("a&amp;b"), "a&b");
    }

    #[test]
    fn manifest_and_spine_are_read_out_of_the_package_document() {
        let opf = r#"<package unique-identifier="bookid"><manifest>
            <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav" />
            <item id="p1" href="text/page_001.xhtml" media-type="application/xhtml+xml" />
        </manifest><spine><itemref idref="p1" /></spine></package>"#;
        let items = parse_manifest_items(opf);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].properties.as_deref(), Some("nav"));
        assert_eq!(parse_spine_idrefs(opf), vec!["p1"]);
        assert_eq!(
            attribute_value(opf, "unique-identifier").as_deref(),
            Some("bookid")
        );
    }

    #[test]
    fn modified_timestamps_are_checked_against_the_required_shape() {
        let mut issues = Vec::new();
        check_modified(
            "<meta property=\"dcterms:modified\">2018-08-21T12:52:01Z</meta>",
            "opf",
            &mut issues,
        );
        assert!(issues.is_empty());

        // pixiv がそのまま返す形。これが通っていたので取り込みに弾かれていた。
        check_modified(
            "<meta property=\"dcterms:modified\">2018-08-21T21:52:01+09:00</meta>",
            "opf",
            &mut issues,
        );
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, "OPF-MODIFIED-FORMAT");
    }

    #[test]
    fn navigation_cardinality_and_ncx_order_are_checked() {
        let nav = r#"<html><body>
            <nav epub:type="toc"><ol><li>x</li></ol></nav>
            <nav epub:type="landmarks"><a epub:type="toc">目次</a></nav>
        </body></html>"#;
        assert_eq!(count_toc_navs(nav), 1);

        let mut issues = Vec::new();
        check_ncx_order(
            r#"<ncx><navPoint playOrder="1"/><navPoint playOrder="1"/><navPoint playOrder="3"/></ncx>"#,
            "toc.ncx",
            &mut issues,
        );
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code, "NCX-PLAYORDER-INVALID");

        issues.clear();
        check_ncx_order(
            r#"<ncx><navPoint playOrder="1"/><navPoint playOrder="2"/></ncx>"#,
            "toc.ncx",
            &mut issues,
        );
        assert!(issues.is_empty());
    }

    #[test]
    fn validation_rejects_an_extreme_compression_ratio_before_expanding_it() {
        let path = test_epub_path("ratio");
        let file = std::fs::File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("mimetype", options).unwrap();
        writer.write_all(b"application/epub+zip").unwrap();
        writer.start_file("OEBPS/text/bomb.xhtml", options).unwrap();
        writer.write_all(&vec![0u8; 2 * 1024 * 1024]).unwrap();
        writer.finish().unwrap();

        let error = validate_epub(&path).unwrap_err();
        assert!(error.contains("圧縮率"));
        let _ = std::fs::remove_file(path);
    }
}
