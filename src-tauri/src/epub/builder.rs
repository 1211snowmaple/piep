//! EPUB 3.0 自作ビルダー (所有型)。
//!
//! `zip` crate を使用して EPUB 3.0 準拠の ZIP アーカイブを生成する。

use crate::epub::image_processor;
use crate::epub::intermediate::*;
use crate::epub::template::{EpubRenderer, ManifestItem, SpineItemref};
use regex::Regex;
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

const CONTAINER_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

type ProcessedImage = (String, Vec<u8>, String);
type RenameMap = HashMap<String, String>;

/// 所有型 EPUB ビルダー — spawn_blocking 内で安全に使用可能
pub struct EpubBuilder {
    manifest: EpubManifest,
    template_contents: HashMap<String, String>,
    compress_options: ImageCompressOptions,
}

impl EpubBuilder {
    pub fn new(
        manifest: EpubManifest,
        template_contents: HashMap<String, String>,
        compress_options: ImageCompressOptions,
    ) -> Self {
        Self {
            manifest,
            template_contents,
            compress_options,
        }
    }

    /// EPUB ファイルを生成して指定パスに書き出す
    pub fn build(&self, output_path: &Path) -> Result<(), String> {
        log::info!("EPUB ビルド開始: {:?}", output_path);

        let file = std::fs::File::create(output_path)
            .map_err(|e| format!("出力ファイルの作成に失敗: {}", e))?;
        let mut zip = zip::ZipWriter::new(file);

        let renderer = EpubRenderer::new(self.template_contents.clone());

        // 1. mimetype
        log::info!("  mimetype 書き込み");
        self.write_mimetype(&mut zip)?;

        // 2. META-INF/container.xml
        self.write_container_xml(&mut zip)?;

        // 3. 画像の処理
        log::info!("  画像処理中...");
        let (processed_images, rename_map) = self.process_images()?;
        log::info!("  画像処理完了: {} 件", processed_images.len());

        // 4. CSS
        log::info!("  スタイルシート生成");
        let css = renderer.render_style()?;
        self.write_text_entry(&mut zip, "OEBPS/style/style.css", &css)?;

        // 5. カバーページ
        let has_cover = self.manifest.content.cover_image.is_some();
        if has_cover {
            log::info!("  カバーページ生成");
            let cover_ext = processed_images
                .iter()
                .find(|(p, _, _)| p.contains("cover"))
                .map(|(p, _, _)| {
                    Path::new(p)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("jpg")
                })
                .unwrap_or("jpg");
            let cover_page =
                renderer.render_cover_page(&format!("../images/cover.{}", cover_ext))?;
            self.write_text_entry(&mut zip, "OEBPS/text/cover.xhtml", &cover_page)?;
        }

        // 6. 作品情報ページ
        log::info!("  作品情報ページ生成");
        let cover_href = if has_cover {
            let ext = processed_images
                .iter()
                .find(|(p, _, _)| p.contains("cover"))
                .map(|(p, _, _)| {
                    Path::new(p)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("jpg")
                })
                .unwrap_or("jpg");
            Some(format!("../images/cover.{}", ext))
        } else {
            None
        };
        let info_page = renderer.render_info_page(&self.manifest, cover_href.as_deref())?;
        self.write_text_entry(&mut zip, "OEBPS/text/info.xhtml", &info_page)?;

        // 7. 本文ページ
        log::info!(
            "  本文ページ生成: {} ページ",
            self.manifest.content.pages.len()
        );
        let image_path_re = Regex::new(r#"\.\./images/([^<'"\s>]+)"#).unwrap();
        for page in &self.manifest.content.pages {
            let page_title = page
                .title
                .clone()
                .unwrap_or_else(|| format!("ページ {}", page.order));

            // 画像パスの置換 (正規表現によるスマートマッチング)
            let mut html_content = page.html_content.clone();
            let mut replacements = Vec::new();

            for cap in image_path_re.captures_iter(&html_content) {
                let matched_path = cap.get(0).unwrap().as_str();
                let ref_filename = cap.get(1).unwrap().as_str();

                let (ref_core, ref_page) = parse_image_filename(ref_filename);
                let mut best_match = None;

                for (old_name, new_name) in &rename_map {
                    let (old_core, old_page) = parse_image_filename(old_name);

                    if ref_core == old_core {
                        if ref_page == old_page {
                            best_match = Some(new_name.clone());
                            break;
                        }
                        if best_match.is_none() {
                            best_match = Some(new_name.clone());
                        }
                    }
                }

                if let Some(matched_new_name) = best_match {
                    let new_path = format!("../images/{}", matched_new_name);
                    replacements.push((matched_path.to_string(), new_path));
                }
            }

            for (old_path, new_path) in replacements {
                html_content = html_content.replace(&old_path, &new_path);
            }

            let rendered = renderer.render_page(&page_title, &html_content)?;
            let filename = format!("OEBPS/text/page_{:03}.xhtml", page.order);
            self.write_text_entry(&mut zip, &filename, &rendered)?;
        }

        // 8. 画像ファイル
        for (epub_path, bytes, _mime) in &processed_images {
            self.write_binary_entry(&mut zip, epub_path, bytes)?;
        }

        // 9. NAV
        log::info!("  ナビゲーション生成");
        let nav = renderer.render_nav(&self.manifest, has_cover)?;
        self.write_text_entry(&mut zip, "OEBPS/nav.xhtml", &nav)?;

        // 10. content.opf
        log::info!("  content.opf 生成");
        let (manifest_items, spine_items) =
            self.build_manifest_and_spine(has_cover, &processed_images);
        let cover_image_id = if has_cover { Some("cover-image") } else { None };
        let opf = renderer.render_content_opf(
            &self.manifest,
            &manifest_items,
            &spine_items,
            cover_image_id,
        )?;
        self.write_text_entry(&mut zip, "OEBPS/content.opf", &opf)?;

        zip.finish()
            .map_err(|e| format!("ZIPの終了に失敗: {}", e))?;
        log::info!("EPUB ビルド完了: {:?}", output_path);
        Ok(())
    }

    fn write_mimetype(&self, zip: &mut zip::ZipWriter<std::fs::File>) -> Result<(), String> {
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("mimetype", options)
            .map_err(|e| format!("mimetype: {}", e))?;
        zip.write_all(b"application/epub+zip")
            .map_err(|e| format!("mimetype write: {}", e))?;
        Ok(())
    }

    fn write_container_xml(&self, zip: &mut zip::ZipWriter<std::fs::File>) -> Result<(), String> {
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file("META-INF/container.xml", options)
            .map_err(|e| format!("container.xml: {}", e))?;
        zip.write_all(CONTAINER_XML.as_bytes())
            .map_err(|e| format!("container.xml write: {}", e))?;
        Ok(())
    }

    fn write_text_entry(
        &self,
        zip: &mut zip::ZipWriter<std::fs::File>,
        path: &str,
        content: &str,
    ) -> Result<(), String> {
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        zip.start_file(path, options)
            .map_err(|e| format!("'{}': {}", path, e))?;
        zip.write_all(content.as_bytes())
            .map_err(|e| format!("'{}' write: {}", path, e))?;
        Ok(())
    }

    fn write_binary_entry(
        &self,
        zip: &mut zip::ZipWriter<std::fs::File>,
        path: &str,
        data: &[u8],
    ) -> Result<(), String> {
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file(path, options)
            .map_err(|e| format!("'{}': {}", path, e))?;
        zip.write_all(data)
            .map_err(|e| format!("'{}' write: {}", path, e))?;
        Ok(())
    }

    fn process_images(&self) -> Result<(Vec<ProcessedImage>, RenameMap), String> {
        let mut images = Vec::new();
        let mut rename_map = HashMap::new();

        if let Some(ref cover) = self.manifest.content.cover_image {
            let path = Path::new(&cover.local_path);
            if path.exists() {
                let (bytes, mime) = image_processor::process_image(path, &self.compress_options)?;
                let ext = image_processor::mime_to_extension(&mime);
                let new_filename = format!("cover.{}", ext);

                let original_filename = Path::new(&cover.local_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("cover.jpg")
                    .to_string();

                images.push((format!("OEBPS/images/{}", new_filename), bytes, mime));
                rename_map.insert(original_filename, new_filename);
            }
        }

        for illust in &self.manifest.content.illustrations {
            let path = Path::new(&illust.local_path);
            if path.exists() {
                let (bytes, mime) = image_processor::process_image(path, &self.compress_options)?;

                let original_filename = Path::new(&illust.local_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("image.jpg")
                    .to_string();

                let file_stem = Path::new(&original_filename)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("image")
                    .to_string();

                let ext = image_processor::mime_to_extension(&mime);
                let new_filename = format!("{}.{}", file_stem, ext);

                images.push((format!("OEBPS/images/{}", new_filename), bytes, mime));
                rename_map.insert(original_filename, new_filename);
            }
        }

        Ok((images, rename_map))
    }

    fn build_manifest_and_spine(
        &self,
        has_cover: bool,
        images: &[(String, Vec<u8>, String)],
    ) -> (Vec<ManifestItem>, Vec<SpineItemref>) {
        let mut items = Vec::new();
        let mut spine = Vec::new();

        items.push(ManifestItem {
            id: "nav".into(),
            href: "nav.xhtml".into(),
            media_type: "application/xhtml+xml".into(),
            properties: Some("nav".into()),
        });
        items.push(ManifestItem {
            id: "style".into(),
            href: "style/style.css".into(),
            media_type: "text/css".into(),
            properties: None,
        });

        if has_cover {
            items.push(ManifestItem {
                id: "cover".into(),
                href: "text/cover.xhtml".into(),
                media_type: "application/xhtml+xml".into(),
                properties: None,
            });
            spine.push(SpineItemref {
                idref: "cover".into(),
                linear: false,
            });
        }

        items.push(ManifestItem {
            id: "info".into(),
            href: "text/info.xhtml".into(),
            media_type: "application/xhtml+xml".into(),
            properties: None,
        });
        spine.push(SpineItemref {
            idref: "info".into(),
            linear: true,
        });

        for page in &self.manifest.content.pages {
            let id = format!("page-{:03}", page.order);
            items.push(ManifestItem {
                id: id.clone(),
                href: format!("text/page_{:03}.xhtml", page.order),
                media_type: "application/xhtml+xml".into(),
                properties: None,
            });
            spine.push(SpineItemref {
                idref: id,
                linear: true,
            });
        }

        for (epub_path, _, mime) in images {
            let href = epub_path.strip_prefix("OEBPS/").unwrap_or(epub_path);
            let filename = Path::new(epub_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("image");
            let id = if href.contains("cover") {
                "cover-image".into()
            } else {
                format!("img-{}", filename)
            };
            items.push(ManifestItem {
                id,
                href: href.to_string(),
                media_type: mime.clone(),
                properties: if href.contains("cover") {
                    Some("cover-image".into())
                } else {
                    None
                },
            });
        }

        (items, spine)
    }
}

pub fn sanitize_epub_filename(title: &str) -> String {
    let mut cleaned: String = title
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            _ => c,
        })
        .collect();
    cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() {
        cleaned = "epub_export".to_string();
    }
    if cleaned.len() > 195 {
        cleaned = cleaned[..195].to_string();
    }
    format!("{}.epub", cleaned)
}

/// 画像ファイル名からコアID（拡張子・プレフィックス・ページサフィックスを除外）とページインデックスを抽出する
fn parse_image_filename(filename: &str) -> (String, usize) {
    // プレフィックスの除外
    let mut clean = filename;
    if clean.starts_with("illust_") || clean.starts_with("inline_") {
        clean = &clean[7..];
    }

    // 拡張子の除外
    let stem = Path::new(clean)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(clean);

    // ページ番号のパース
    // "123456_p1" -> core = "123456", page = 1
    // "123456" -> core = "123456", page = 0
    // "123456-2" -> core = "123456", page = 1 (-2はページ2、つまりインデックス1)
    let mut core = stem.to_string();
    let mut page_idx = 0;

    if let Some(pos) = stem.rfind("_p") {
        if let Ok(idx) = stem[pos + 2..].parse::<usize>() {
            core = stem[..pos].to_string();
            page_idx = idx;
        }
    } else if let Some(pos) = stem.rfind('-') {
        if let Ok(num) = stem[pos + 1..].parse::<usize>() {
            core = stem[..pos].to_string();
            if num > 0 {
                page_idx = num - 1;
            }
        }
    }

    (core, page_idx)
}
