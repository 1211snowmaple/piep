//! EPUB 3.3 自作ビルダー (所有型)。
//!
//! `zip` crate を使用して EPUB 3.3 / OCF 3.3 準拠の ZIP アーカイブを生成する。
//!
//! 規格上ゆるがせにできない点をここで担保する:
//!   * `mimetype` が先頭の項目で、無圧縮・追加フィールドなしであること
//!   * マニフェストの ID が NCName で、重複しないこと
//!   * href が URL として書かれ、指す先が実在すること
//!   * 本文が参照する資源がすべてマニフェストに載っていること

use crate::epub::image_processor;
use crate::epub::intermediate::*;
use crate::epub::meta;
use crate::epub::template::{EpubRenderer, ManifestItem, NavEntry, SpineItemref};
use crate::epub::xhtml::{self, ImageRef};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

// Images are still owned while references and the OPF are assembled. Keep a
// strict upper bound so an EPUB with thousands of assets cannot exhaust the
// process before the bytes are streamed into the ZIP.
const MAX_SOURCE_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TOTAL_PACKAGED_IMAGE_BYTES: usize = 128 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 40_000_000;
const MAX_PACKAGED_IMAGE_COUNT: usize = 10_000;

const CONTAINER_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

/// EPUB に収録した画像一枚。
struct PackagedImage {
    /// ZIP 内のパス (OEBPS/images/...)。
    zip_path: String,
    /// マニフェストの ID。
    id: String,
    media_type: String,
    bytes: Vec<u8>,
    /// 本文がこの画像を指すときに使いうる名前。拡張子は含まない。
    keys: Vec<String>,
    is_cover: bool,
    dimensions: Option<(u32, u32)>,
}

/// 所有型 EPUB ビルダー — spawn_blocking 内で安全に使用可能
pub struct EpubBuilder {
    manifest: EpubManifest,
    template_contents: HashMap<String, String>,
    settings: TemplateSettings,
    compress_options: ImageCompressOptions,
}

impl EpubBuilder {
    pub fn new(
        manifest: EpubManifest,
        template_contents: HashMap<String, String>,
        settings: TemplateSettings,
        compress_options: ImageCompressOptions,
    ) -> Self {
        Self {
            manifest,
            template_contents,
            settings,
            compress_options,
        }
    }

    /// EPUB ファイルを生成して指定パスに書き出す
    pub fn build(&self, output_path: &Path) -> Result<(), String> {
        log::info!("EPUB ビルド開始: {:?}", output_path);

        let file = std::fs::File::create(output_path)
            .map_err(|e| format!("出力ファイルの作成に失敗: {}", e))?;
        let mut zip = zip::ZipWriter::new(file);
        let renderer = EpubRenderer::new(self.template_contents.clone(), self.settings.clone());
        let settings = renderer.settings().clone();

        // 1. mimetype — OCF はこれが先頭かつ無圧縮であることを要求する
        self.write_mimetype(&mut zip)?;
        self.write_container_xml(&mut zip)?;

        // 2. 画像 — 先に確定させないと、本文の参照を解決できない
        log::info!("  画像処理中...");
        let images = self.package_images()?;
        log::info!("  画像処理完了: {} 件", images.len());
        let cover = images.iter().find(|image| image.is_cover);

        // 3. スタイルシート
        let css = renderer.render_style(&self.manifest)?;
        self.write_text_entry(&mut zip, "OEBPS/style/style.css", &css)?;

        // 4. 表紙
        let has_cover_page = settings.include_cover_page && cover.is_some();
        let mut cover_uses_svg = false;
        if let Some(cover) = cover.filter(|_| has_cover_page) {
            let href = relative_href(&cover.zip_path, "OEBPS/text");
            let (width, height) = cover.dimensions.unwrap_or((0, 0));
            let page = renderer.render_cover_page(&self.manifest, &href, width, height)?;
            // コード編集で SVG を使うテンプレートも正しく宣言し、既定は Kindle
            // 互換性を優先した単純な img のままにする。
            cover_uses_svg = page.contains("<svg") || page.contains("< svg");
            self.write_text_entry(&mut zip, "OEBPS/text/cover.xhtml", &page)?;
        }

        // 5. 作品情報ページ
        let cover_href = cover.map(|image| relative_href(&image.zip_path, "OEBPS/text"));
        if settings.include_info_page {
            let info_page = renderer.render_info_page(&self.manifest, cover_href.as_deref())?;
            self.write_text_entry(&mut zip, "OEBPS/text/info.xhtml", &info_page)?;
        }

        // 6. 本文
        log::info!(
            "  本文ページ生成: {} ページ",
            self.manifest.content.pages.len()
        );
        for page in &self.manifest.content.pages {
            let html = self.resolve_page_images(&page.html_content, &images);
            let rendered = renderer.render_page(&self.manifest, page, &html)?;
            let filename = format!("OEBPS/text/{}", page_filename(page.order));
            self.write_text_entry(&mut zip, &filename, &rendered)?;
        }

        // 7. 画像本体
        for image in &images {
            self.write_binary_entry(&mut zip, &image.zip_path, &image.bytes)?;
        }

        // 8. 目次
        let entries = self.nav_entries(&settings);
        let nav = renderer.render_nav(
            &self.manifest,
            &entries,
            has_cover_page,
            self.manifest
                .content
                .pages
                .first()
                .map(|page| format!("text/{}", page_filename(page.order)))
                .as_deref(),
        )?;
        self.write_text_entry(&mut zip, "OEBPS/nav.xhtml", &nav)?;
        if settings.include_ncx {
            let ncx = renderer.render_ncx(&self.manifest, &entries, &self.manifest.core.id_)?;
            self.write_text_entry(&mut zip, "OEBPS/toc.ncx", &ncx)?;
        }

        // 9. パッケージ文書
        let (manifest_items, spine_items) =
            self.build_manifest_and_spine(&settings, has_cover_page, cover_uses_svg, &images);
        let modified = self.modified_timestamp();
        let published = meta::to_utc_timestamp(&self.manifest.core.date_published);
        let opf = renderer.render_content_opf(
            &self.manifest,
            &manifest_items,
            &spine_items,
            cover.map(|image| image.id.as_str()),
            &modified,
            published.as_deref(),
        )?;
        self.write_text_entry(&mut zip, "OEBPS/content.opf", &opf)?;

        let file = zip
            .finish()
            .map_err(|e| format!("ZIPの終了に失敗: {}", e))?;
        file.sync_all()
            .map_err(|e| format!("EPUBの同期に失敗: {}", e))?;
        log::info!("EPUB ビルド完了: {:?}", output_path);
        Ok(())
    }

    /// `dcterms:modified` に書く時刻。
    ///
    /// これは配信元の投稿更新日時ではなく、この EPUB パッケージを最後に
    /// 変更した時刻を表す。テンプレートや変換処理だけが変わった再書き出しも
    /// 別版なので、ビルド時刻を UTC・秒精度で記録する。
    fn modified_timestamp(&self) -> String {
        chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
    }

    fn write_mimetype(&self, zip: &mut zip::ZipWriter<std::fs::File>) -> Result<(), String> {
        // 追加フィールドを持たせないため、既定のオプションのまま無圧縮で書く。
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("mimetype", options)
            .map_err(|e| format!("mimetype: {}", e))?;
        zip.write_all(b"application/epub+zip")
            .map_err(|e| format!("mimetype write: {}", e))?;
        Ok(())
    }

    fn write_container_xml(&self, zip: &mut zip::ZipWriter<std::fs::File>) -> Result<(), String> {
        self.write_text_entry(zip, "META-INF/container.xml", CONTAINER_XML)
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
        // 画像はすでに圧縮済み。もう一度縮めても大きさは変わらず時間だけかかる。
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file(path, options)
            .map_err(|e| format!("'{}': {}", path, e))?;
        zip.write_all(data)
            .map_err(|e| format!("'{}' write: {}", path, e))?;
        Ok(())
    }

    // ------------------------------------------------------------
    // 画像
    // ------------------------------------------------------------

    /// 収録する画像を確定させる。ID もファイル名もここで一意にする。
    fn package_images(&self) -> Result<Vec<PackagedImage>, String> {
        let source_count = self.manifest.content.illustrations.len()
            + usize::from(self.manifest.content.cover_image.is_some());
        if source_count > MAX_PACKAGED_IMAGE_COUNT {
            return Err(format!(
                "EPUB内画像の件数が上限（{}件）を超えています",
                MAX_PACKAGED_IMAGE_COUNT
            ));
        }
        let mut packaged: Vec<PackagedImage> = Vec::new();
        let mut used_names: HashSet<String> = HashSet::new();
        let mut used_ids: HashSet<String> = HashSet::new();
        let mut total_packaged_bytes = 0usize;

        let sources = self
            .manifest
            .content
            .cover_image
            .iter()
            .map(|image| (image, true))
            .chain(
                self.manifest
                    .content
                    .illustrations
                    .iter()
                    .map(|image| (image, false)),
            );

        for (image, is_cover) in sources {
            let path = Path::new(&image.local_path);
            if !path.exists() {
                log::warn!("画像が見つからないため除外します: {}", image.local_path);
                continue;
            }
            let source_size = path
                .metadata()
                .map_err(|error| format!("画像情報を取得できません ({}): {error}", path.display()))?
                .len();
            // **一枚の大きさで本ごと落とさない。** すぐ下のデコード失敗は
            // 「その一枚を除いて続ける」のに、上限超えだけが書き出し全体を
            // 止めていた。挿絵50枚のうち1枚が大きいだけで、本文も残り49枚も
            // 出てこない。方針は一つにする - 入らない絵は置いていく。
            if source_size > MAX_SOURCE_IMAGE_BYTES {
                log::warn!(
                    "画像が1ファイルの上限（{} MiB）を超えるため除外します: {}",
                    MAX_SOURCE_IMAGE_BYTES / 1024 / 1024,
                    path.display()
                );
                continue;
            }
            if let Some((width, height)) = image_dimensions_from_path(path) {
                if u64::from(width).saturating_mul(u64::from(height)) > MAX_IMAGE_PIXELS {
                    log::warn!(
                        "画像の画素数が上限（{} MP）を超えるため除外します: {} ({}x{})",
                        MAX_IMAGE_PIXELS / 1_000_000,
                        path.display(),
                        width,
                        height
                    );
                    continue;
                }
            }
            let (bytes, media_type) =
                match image_processor::process_image(path, &self.compress_options) {
                    Ok(result) => result,
                    Err(error) => {
                        // 一枚の失敗で本ごと落とさない。参照はあとで取り除かれる。
                        log::warn!(
                            "画像を処理できないため除外します ({}): {}",
                            image.local_path,
                            error
                        );
                        continue;
                    }
                };
            total_packaged_bytes = total_packaged_bytes
                .checked_add(bytes.len())
                .ok_or_else(|| "画像容量の合計が大きすぎます".to_string())?;
            if total_packaged_bytes > MAX_TOTAL_PACKAGED_IMAGE_BYTES {
                return Err(format!(
                    "EPUB内画像の合計が上限（{} MiB）を超えています",
                    MAX_TOTAL_PACKAGED_IMAGE_BYTES / 1024 / 1024
                ));
            }
            let extension = image_processor::mime_to_extension(&media_type);
            let stem = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("image");

            // A merged collection can contain files with the same original
            // stem (for example every FANBOX post may have `image-0`).  The
            // manifest image id is namespaced by the collection merger and is
            // therefore the authoritative packaged name.
            let base = if is_cover { "cover" } else { image.id.as_str() };
            let filename = unique_name(base, extension, &mut used_names);
            let id = unique_id(
                &if is_cover {
                    "cover-image".to_string()
                } else {
                    format!("img-{}", xhtml::ncname(stem))
                },
                &mut used_ids,
            );
            let dimensions = image_dimensions(&bytes);

            let mut keys = reference_keys(stem, is_cover);
            keys.push(image.id.clone());
            keys.sort();
            keys.dedup();
            packaged.push(PackagedImage {
                zip_path: format!("OEBPS/images/{}", filename),
                id,
                media_type,
                bytes,
                keys,
                is_cover,
                dimensions,
            });
        }

        Ok(packaged)
    }

    /// 本文の `<img src>` を、実際に収録した画像へつなぎ直す。
    ///
    /// 変換の段階では拡張子まで確定できない（元が png でも圧縮で jpg になる）。
    /// つなげられなかった参照は、その `<img>` ごと落とす。指す先の無い参照は
    /// EPUB を無効にし、取り込み側に丸ごと拒否される。
    fn resolve_page_images(&self, html: &str, images: &[PackagedImage]) -> String {
        xhtml::sanitize_fragment_with(html, &mut |src| {
            let Some(reference) = src.rsplit('/').next() else {
                return ImageRef::Drop;
            };
            let stem = Path::new(reference)
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or(reference);

            let matched = images
                .iter()
                .find(|image| image.keys.iter().any(|key| key == stem))
                // 参照が「作品 ID だけ」で来ることがある。その作品の一枚目に寄せる。
                .or_else(|| {
                    images.iter().find(|image| {
                        image
                            .keys
                            .iter()
                            .any(|key| key.starts_with(&format!("{stem}_p")))
                    })
                });

            match matched {
                Some(image) => ImageRef::Keep(relative_href(&image.zip_path, "OEBPS/text")),
                None => {
                    log::warn!("参照先の画像が無いため除外します: {}", src);
                    ImageRef::Drop
                }
            }
        })
    }

    // ------------------------------------------------------------
    // 目次とパッケージ文書
    // ------------------------------------------------------------

    fn nav_entries(&self, settings: &TemplateSettings) -> Vec<NavEntry> {
        let mut entries = Vec::new();
        let mut order = 0u32;

        if settings.include_info_page {
            order += 1;
            entries.push(NavEntry {
                id: "info".into(),
                order,
                href: "text/info.xhtml".into(),
                title: settings
                    .strings
                    .get("INFO_TITLE")
                    .cloned()
                    .unwrap_or_else(|| "作品情報".into()),
                children: Vec::new(),
            });
        }

        for page in &self.manifest.content.pages {
            order += 1;
            let page_order = order;
            let href = format!("text/{}", page_filename(page.order));
            let mut children = Vec::new();
            if settings.chapter_toc {
                // 見出しがページの題を兼ねているときは、目次に二度出さない。
                let skip_first = page
                    .chapters
                    .first()
                    .is_some_and(|chapter| Some(&chapter.title) == page.title.as_ref());
                for chapter in page.chapters.iter().skip(usize::from(skip_first)) {
                    order += 1;
                    children.push(NavEntry {
                        id: chapter.id.clone(),
                        order,
                        href: format!("{}#{}", href, chapter.id),
                        title: chapter.title.clone(),
                        children: Vec::new(),
                    });
                }
            }
            entries.push(NavEntry {
                id: format!("page-{:03}", page.order),
                order: page_order,
                href,
                title: page
                    .title
                    .clone()
                    .unwrap_or_else(|| format!("ページ {}", page.order)),
                children,
            });
        }

        entries
    }

    fn build_manifest_and_spine(
        &self,
        settings: &TemplateSettings,
        has_cover_page: bool,
        cover_uses_svg: bool,
        images: &[PackagedImage],
    ) -> (Vec<ManifestItem>, Vec<SpineItemref>) {
        let mut items = Vec::new();
        let mut spine = Vec::new();

        items.push(ManifestItem {
            id: "nav".into(),
            href: "nav.xhtml".into(),
            media_type: "application/xhtml+xml".into(),
            properties: Some("nav".into()),
        });
        if settings.include_ncx {
            items.push(ManifestItem {
                id: "ncx".into(),
                href: "toc.ncx".into(),
                media_type: "application/x-dtbncx+xml".into(),
                properties: None,
            });
        }
        items.push(ManifestItem {
            id: "style".into(),
            href: "style/style.css".into(),
            media_type: "text/css".into(),
            properties: None,
        });

        if has_cover_page {
            items.push(ManifestItem {
                id: "cover".into(),
                href: "text/cover.xhtml".into(),
                media_type: "application/xhtml+xml".into(),
                // 宣言と中身が食い違うと検証に落ちる。SVG を使ったときだけ立てる。
                properties: cover_uses_svg.then(|| "svg".to_string()),
            });
            spine.push(SpineItemref {
                idref: "cover".into(),
                linear: settings.cover_in_reading_order,
                properties: None,
            });
        }

        if settings.include_info_page {
            items.push(ManifestItem {
                id: "info".into(),
                href: "text/info.xhtml".into(),
                media_type: "application/xhtml+xml".into(),
                properties: None,
            });
            spine.push(SpineItemref {
                idref: "info".into(),
                linear: true,
                properties: None,
            });
        }

        // nav.xhtml を本文の早い位置に置く。EPUB 3 では nav 文書を spine 外にも
        // 置けるが、Kindle は HTML 目次を強く推奨し、landmarks の toc が指す先も
        // spine 項目であることを要求する。情報ページの直後なら試し読みでも届く。
        spine.push(SpineItemref {
            idref: "nav".into(),
            linear: true,
            properties: None,
        });

        for page in &self.manifest.content.pages {
            let id = format!("page-{:03}", page.order);
            items.push(ManifestItem {
                id: id.clone(),
                href: format!("text/{}", page_filename(page.order)),
                media_type: "application/xhtml+xml".into(),
                properties: None,
            });
            spine.push(SpineItemref {
                idref: id,
                linear: true,
                properties: None,
            });
        }

        for image in images {
            items.push(ManifestItem {
                id: image.id.clone(),
                href: xhtml::encode_path(
                    image
                        .zip_path
                        .strip_prefix("OEBPS/")
                        .unwrap_or(&image.zip_path),
                ),
                media_type: image.media_type.clone(),
                properties: image.is_cover.then(|| "cover-image".to_string()),
            });
        }

        (items, spine)
    }
}

// ============================================================
// ヘルパー
// ============================================================

fn page_filename(order: u32) -> String {
    format!("page_{:03}.xhtml", order)
}

/// ZIP 内の絶対パスを、ある文書から見た相対 URL にする。
fn relative_href(zip_path: &str, from_dir: &str) -> String {
    let target: Vec<&str> = zip_path.split('/').collect();
    let base: Vec<&str> = from_dir
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let shared = target
        .iter()
        .zip(base.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<String> =
        std::iter::repeat_n("..".to_string(), base.len() - shared).collect();
    parts.extend(target[shared..].iter().map(|part| part.to_string()));
    xhtml::encode_path(&parts.join("/"))
}

/// 本文がこの画像を指すときに使いうる名前。
///
/// 変換側は pixiv の `12345_p0`、FANBOX の画像 ID をそのまま書く。表紙は
/// 元のファイル名でも `cover` でも指されうる。
fn reference_keys(stem: &str, is_cover: bool) -> Vec<String> {
    let mut keys = vec![stem.to_string()];
    if is_cover {
        keys.push("cover".to_string());
    }
    // 保存時に付く接頭辞を外した形でも引けるようにする。
    for prefix in ["illust_", "inline_"] {
        if let Some(rest) = stem.strip_prefix(prefix) {
            keys.push(rest.to_string());
        }
    }
    keys
}

fn unique_name(base: &str, extension: &str, used: &mut HashSet<String>) -> String {
    let sanitized: String = base
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = if sanitized.is_empty() {
        "image".to_string()
    } else {
        sanitized
    };
    let mut candidate = format!("{}.{}", sanitized, extension);
    let mut suffix = 1;
    while !used.insert(candidate.clone()) {
        candidate = format!("{}-{}.{}", sanitized, suffix, extension);
        suffix += 1;
    }
    candidate
}

/// 重複しない NCName。ID がひとつでもぶつかると EPUB 全体が無効になる。
fn unique_id(base: &str, used: &mut HashSet<String>) -> String {
    let base = xhtml::ncname(base);
    let mut candidate = base.clone();
    let mut suffix = 1;
    while !used.insert(candidate.clone()) {
        candidate = format!("{}-{}", base, suffix);
        suffix += 1;
    }
    candidate
}

/// 表紙を SVG で組むために要る実寸。読めなければ SVG は使わない。
fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// Decode-free dimension check used before allocating pixels for compression.
fn image_dimensions_from_path(path: &Path) -> Option<(u32, u32)> {
    image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

pub fn sanitize_epub_filename(title: &str) -> String {
    let mut cleaned: String = title
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            // 制御文字はファイル名に置けない
            c if (c as u32) < 0x20 => '_',
            _ => c,
        })
        .collect();
    cleaned = cleaned.trim().trim_matches('.').to_string();
    if cleaned.is_empty() {
        cleaned = "epub_export".to_string();
    }
    // バイト数で切ると多バジト文字の途中で割れる
    if cleaned.chars().count() > 120 {
        cleaned = cleaned.chars().take(120).collect();
    }
    format!("{}.epub", cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hrefs_are_relative_to_the_document_that_uses_them() {
        assert_eq!(
            relative_href("OEBPS/images/cover.jpg", "OEBPS/text"),
            "../images/cover.jpg"
        );
        assert_eq!(
            relative_href("OEBPS/images/cover.jpg", "OEBPS"),
            "images/cover.jpg"
        );
        assert_eq!(
            relative_href("OEBPS/images/表紙.jpg", "OEBPS/text"),
            "../images/%E8%A1%A8%E7%B4%99.jpg"
        );
    }

    #[test]
    fn identifiers_and_filenames_never_collide() {
        let mut ids = HashSet::new();
        assert_eq!(unique_id("img-1", &mut ids), "img-1");
        assert_eq!(unique_id("img-1", &mut ids), "img-1-1");
        // 数字始まりは XML の名前として使えないので、必ず前置きが付く。
        assert_eq!(unique_id("123_p0", &mut ids), "_123_p0");

        let mut names = HashSet::new();
        assert_eq!(unique_name("a b", "jpg", &mut names), "a_b.jpg");
        assert_eq!(unique_name("a b", "jpg", &mut names), "a_b-1.jpg");
    }

    #[test]
    fn filenames_are_trimmed_without_splitting_characters() {
        assert_eq!(sanitize_epub_filename("a/b:c"), "a_b_c.epub");
        assert_eq!(sanitize_epub_filename("   "), "epub_export.epub");
        let long = "あ".repeat(300);
        let name = sanitize_epub_filename(&long);
        assert!(name.ends_with(".epub"));
        assert_eq!(name.chars().count(), 125);
    }

    #[test]
    fn illustration_references_resolve_through_their_bare_ids() {
        assert!(reference_keys("illust_123_p0", false).contains(&"123_p0".to_string()));
        assert!(reference_keys("ci999", true).contains(&"cover".to_string()));
    }

    #[test]
    fn leaves_out_an_image_before_reading_it_when_the_source_quota_is_exceeded() {
        let path = std::env::temp_dir().join(format!(
            "piep_epub_oversize_{:016x}.jpg",
            rand::random::<u64>()
        ));
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_SOURCE_IMAGE_BYTES + 1).unwrap();
        drop(file);

        let mut manifest = hostile_manifest();
        manifest.content.illustrations.push(EpubImage {
            id: "oversize".into(),
            local_path: path.to_string_lossy().to_string(),
            mime_type: "image/jpeg".into(),
            alt_text: None,
            caption: None,
            width: None,
            height: None,
        });
        let builder = EpubBuilder::new(
            manifest,
            default_template_contents(),
            TemplateSettings::default(),
            ImageCompressOptions::default(),
        );
        // 上限を超えた一枚は**読まずに**除く。読んでしまえば上限の意味が無い。
        // そして本ごと落とさない - 挿絵が一枚入らないことと、本が出てこない
        // ことは別である。
        let packaged = builder
            .package_images()
            .expect("oversized image must not fail the book");
        assert!(
            !packaged.iter().any(|image| image.id == "oversize"),
            "the oversized image must be left out"
        );
        let _ = std::fs::remove_file(path);
    }

    // ------------------------------------------------------------
    // 通しの検査
    // ------------------------------------------------------------

    fn default_template_contents() -> HashMap<String, String> {
        builtin_template_contents("default")
    }

    /// 組み込みテンプレート1つぶんの中身。`load_template_contents` と同じく、
    /// 継承（default から借りるファイル）と CSS の include を先に解決する。
    fn builtin_template_contents(template_name: &str) -> HashMap<String, String> {
        use crate::epub::template::{builtin_file_content, DefaultTemplates};
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

    fn builtin_settings(template_name: &str) -> TemplateSettings {
        use crate::epub::template::{DefaultTemplates, FanboxTemplates, PixivTemplates};
        let json = match template_name {
            "pixiv" => PixivTemplates::settings(),
            "fanbox" => FanboxTemplates::settings(),
            _ => DefaultTemplates::settings(),
        };
        serde_json::from_str::<TemplateSettings>(json)
            .unwrap_or_else(|error| panic!("{template_name}/template.json: {error}"))
            .normalized()
    }

    /// 取り込み側が実際に嫌がる材料を全部入れた作品。
    fn hostile_manifest() -> EpubManifest {
        EpubManifest {
            core: EpubCore {
                id_: "urn:pixiv:novel:42".into(),
                // & と < は、エスケープを忘れると OPF ごと解析不能にする。
                name: "Q&A <試作> \"引用\"".into(),
                author: EpubAuthor {
                    name: "作者 & 仲間".into(),
                    id: "7".into(),
                    account: Some("acct".into()),
                    url: Some("https://www.pixiv.net/users/7".into()),
                    icon_url: None,
                },
                description: Some("<p>紹介 &amp; 文</p>".into()),
                description_text: Some("紹介 & 文".into()),
                keywords: vec!["R-18".into(), "タグ&記号".into()],
                tags: vec![EpubTag {
                    name: "タグ&記号".into(),
                    translated: None,
                }],
                // ローカル時刻のまま。dcterms:modified はここから直す必要がある。
                date_published: "2018-08-21T21:52:01+09:00".into(),
                date_modified: None,
                main_entity_of_page: "https://www.pixiv.net/novel/show.php?id=42&mode=1".into(),
                is_part_of: Some(EpubSeries {
                    name: "連作 & 続編".into(),
                    order: Some(3),
                    id: Some("900".into()),
                    url: None,
                }),
                language: "ja".into(),
                publisher: "pixiv".into(),
            },
            provider: ProviderData {
                source: "pixiv".into(),
                novel_id: Some("42".into()),
                post_id: None,
                series_id: Some("900".into()),
                post_type: None,
            },
            content: EpubContent {
                pages: vec![
                    EpubPage {
                        title: Some("第一章 & そのあと".into()),
                        html_content:
                            "<h2 id=\"chapter-001\">第一章 &amp; そのあと</h2><p>閉じ忘れ<br>と 参照先の無い絵<img src=\"../images/missing.jpg\" alt=\"x\" /></p>"
                                .into(),
                        order: 1,
                        chapters: vec![EpubChapter {
                            id: "chapter-001".into(),
                            title: "第一章 & そのあと".into(),
                        }],
                    },
                    EpubPage {
                        title: Some("ページ 2".into()),
                        html_content: "<p>つづき</p>".into(),
                        order: 2,
                        chapters: Vec::new(),
                    },
                ],
                cover_image: None,
                illustrations: Vec::new(),
                attachments: Vec::new(),
                text_length: 20,
            },
            stats: EpubStats {
                text_length: 20,
                page_count: 2,
                chapter_count: 1,
                image_count: 0,
                attachment_count: 0,
                like_count: Some(5),
                comment_count: None,
                fee_required: None,
                adult: true,
            },
        }
    }

    fn build_to_temp(manifest: EpubManifest, settings: TemplateSettings) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("piep_epub_{}.epub", rand::random::<u64>()));
        EpubBuilder::new(
            manifest,
            default_template_contents(),
            settings,
            ImageCompressOptions::default(),
        )
        .build(&path)
        .expect("build");
        path
    }

    /// ページ内移動のリンクが、本の中で行き先を持っていること。
    ///
    /// 字面のまま残していたころは `[jump:5]` という壊れた文字列が本文に
    /// 見えていた。リンクにするなら、**指した先が本当にあること**まで
    /// 確かめないと、今度は行き止まりを配ることになる。
    #[test]
    fn a_page_jump_points_at_a_file_that_exists() {
        let manifest = crate::epub::converter::convert_to_manifest(
            &serde_json::json!({
                "text": "最初のページ
[jump:2]
[newpage]
二ページ目",
                "detail": {
                    "id": 1,
                    "title": "移動のある作品",
                    "create_date": "2026-01-01T00:00:00+09:00",
                    "user": { "id": 1, "name": "作者" },
                },
            }),
            "pixiv",
            std::path::Path::new("/nonexistent"),
        )
        .expect("manifest");
        let path = build_to_temp(manifest, TemplateSettings::default());
        let report = crate::epub::validate::validate_epub(&path).expect("validate");
        assert!(
            report.valid,
            "検証に失敗しました: {:#?}",
            report
                .issues
                .iter()
                .filter(|issue| issue.severity == "error")
                .collect::<Vec<_>>()
        );

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let mut first = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("OEBPS/text/page_001.xhtml").unwrap(),
            &mut first,
        )
        .unwrap();
        assert!(
            first.contains("page_002.xhtml"),
            "ページ内移動が本文から消えている: {first}"
        );
        // 指した先が本当にあること。検証器の局所参照の検査もここを見ている。
        assert!(archive.by_name("OEBPS/text/page_002.xhtml").is_ok());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_work_full_of_hostile_metadata_still_produces_a_valid_epub() {
        let path = build_to_temp(hostile_manifest(), TemplateSettings::default());
        let report = crate::epub::validate::validate_epub(&path).expect("validate");
        assert!(
            report.valid,
            "検証に失敗しました: {:#?}",
            report
                .issues
                .iter()
                .filter(|issue| issue.severity == "error")
                .collect::<Vec<_>>()
        );

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_string())
            .collect();
        assert_eq!(names[0], "mimetype");
        for expected in [
            "META-INF/container.xml",
            "OEBPS/content.opf",
            "OEBPS/nav.xhtml",
            "OEBPS/toc.ncx",
            "OEBPS/text/info.xhtml",
            "OEBPS/text/page_001.xhtml",
            "OEBPS/text/page_002.xhtml",
        ] {
            assert!(
                names.contains(&expected.to_string()),
                "{expected} がありません"
            );
        }

        let mut opf = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("OEBPS/content.opf").unwrap(), &mut opf)
            .unwrap();
        // 題名の & と < がエスケープされていること。
        assert!(opf.contains("<dc:title id=\"title\">Q&amp;A &lt;試作&gt;"));
        // 配信元の更新時刻ではなく、パッケージを生成した現在時刻が UTC の
        // 秒精度で一つだけ書かれていること。
        let modified_prefix = "<meta property=\"dcterms:modified\">";
        let modified = opf
            .split_once(modified_prefix)
            .and_then(|(_, rest)| rest.split_once("</meta>"))
            .map(|(value, _)| value)
            .expect("dcterms:modified");
        assert!(modified.ends_with('Z'));
        assert!(chrono::DateTime::parse_from_rfc3339(modified).is_ok());
        assert_eq!(opf.matches(modified_prefix).count(), 1);
        assert!(opf.contains("page-progression-direction=\"ltr\""));
        assert!(opf.contains("toc=\"ncx\""));
        assert!(opf.contains("<itemref idref=\"nav\""));

        let mut page = String::new();
        std::io::Read::read_to_string(
            &mut archive.by_name("OEBPS/text/page_001.xhtml").unwrap(),
            &mut page,
        )
        .unwrap();
        // 本文はタグとして生きたまま、参照先の無い画像だけが落ちていること。
        assert!(page.contains("<h2 id=\"chapter-001\">"));
        assert!(page.contains("<br />"));
        assert!(!page.contains("missing.jpg"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn nested_ncx_entries_have_a_strict_play_order() {
        let mut manifest = hostile_manifest();
        // ページ名と章名が同じ場合は重複表示を避けるため章を省く。ここでは
        // 意図的に別名にして、親子の playOrder を検査する。
        manifest.content.pages[0].title = Some("第一ページ".into());
        let builder = EpubBuilder::new(
            manifest,
            default_template_contents(),
            TemplateSettings::default(),
            ImageCompressOptions::default(),
        );
        let entries = builder.nav_entries(&TemplateSettings::default());
        let orders: Vec<u32> = entries
            .iter()
            .flat_map(|entry| {
                std::iter::once(entry.order).chain(entry.children.iter().map(|child| child.order))
            })
            .collect();
        assert_eq!(orders, vec![1, 2, 3, 4]);
        assert!(orders.windows(2).all(|pair| pair[0] < pair[1]));
    }

    /// 3つの組み込みテンプレートが、それぞれの体裁で情報ページを組めること。
    ///
    /// テンプレートは実行時にしか評価されないので、j2 の書き間違いはここでしか
    /// 捕まらない。生成物が検証を通ることと、各体裁の要になる印を確かめる。
    #[test]
    fn every_builtin_template_renders_its_own_front_matter() {
        for template_name in ["default", "pixiv", "fanbox"] {
            let path = std::env::temp_dir().join(format!(
                "piep_epub_{template_name}_{}.epub",
                rand::random::<u64>()
            ));
            EpubBuilder::new(
                hostile_manifest(),
                builtin_template_contents(template_name),
                builtin_settings(template_name),
                ImageCompressOptions::default(),
            )
            .build(&path)
            .unwrap_or_else(|error| panic!("{template_name}: {error}"));

            let report = crate::epub::validate::validate_epub(&path).expect("validate");
            assert!(
                report.valid,
                "{template_name} の検証に失敗しました: {:#?}",
                report
                    .issues
                    .iter()
                    .filter(|issue| issue.severity == "error")
                    .collect::<Vec<_>>()
            );

            let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
            let mut info = String::new();
            std::io::Read::read_to_string(
                &mut archive.by_name("OEBPS/text/info.xhtml").unwrap(),
                &mut info,
            )
            .unwrap();
            let mut css = String::new();
            std::io::Read::read_to_string(
                &mut archive.by_name("OEBPS/style/style.css").unwrap(),
                &mut css,
            )
            .unwrap();

            // 端末のダーク表示に追随する色を、どのテンプレートも持っていること。
            assert!(
                css.contains("prefers-color-scheme: dark"),
                "{template_name}: ダーク表示の指定がありません"
            );
            // 明細は2カラム。ラベルと値が対で出ること。
            assert!(
                info.contains("<div class=\"details\">") && info.contains("<span class=\"label\">"),
                "{template_name}: 明細部がありません"
            );

            match template_name {
                "pixiv" => {
                    assert!(info.contains("class=\"stat-line\""));
                    // 話数は帯の中。右端へ飛ばさない。
                    assert!(info.contains("<span class=\"order\">第3話</span>"));
                }
                "fanbox" => {
                    assert!(info.contains("class=\"post-header\""));
                    assert!(info.contains("class=\"byline\""));
                }
                _ => {
                    assert!(info.contains("<h1 class=\"title\">"));
                }
            }

            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn settings_change_what_the_package_contains() {
        let settings = TemplateSettings {
            include_ncx: false,
            include_info_page: false,
            include_cover_page: false,
            page_progression: "rtl".into(),
            ..TemplateSettings::default()
        };
        let path = build_to_temp(hostile_manifest(), settings);
        let report = crate::epub::validate::validate_epub(&path).expect("validate");
        assert!(report.valid, "検証に失敗しました: {:#?}", report.issues);

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        assert!(archive.by_name("OEBPS/toc.ncx").is_err());
        assert!(archive.by_name("OEBPS/text/info.xhtml").is_err());
        let mut opf = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("OEBPS/content.opf").unwrap(), &mut opf)
            .unwrap();
        assert!(opf.contains("page-progression-direction=\"rtl\""));
        assert!(!opf.contains("toc=\"ncx\""));

        let _ = std::fs::remove_file(path);
    }
}
