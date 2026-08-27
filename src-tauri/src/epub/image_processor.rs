//! 画像圧縮・変換モジュール。
//!
//! JPEG/WebP: `image` crate のエンコーダ (品質制御付き)
//! PNG: `oxipng` による高度なロスレス最適化 (20-40% サイズ削減)

use crate::epub::intermediate::ImageCompressOptions;
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, ImageReader};
use std::io::Cursor;
use std::path::Path;
use zenjpeg::encoder::{ChromaSubsampling, EncoderConfig, PixelLayout, Unstoppable};

/// 画像を圧縮・リサイズし、バイト列として返す。
pub fn process_image(
    input_path: &Path,
    options: &ImageCompressOptions,
) -> Result<(Vec<u8>, String), String> {
    let ext = input_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let original_mime = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "application/octet-stream",
    };

    // 圧縮が無効、または未対応形式
    if !options.enabled
        || original_mime == "image/gif"
        || original_mime == "application/octet-stream"
    {
        let original_bytes = std::fs::read(input_path)
            .map_err(|e| format!("画像読み込みエラー ({}): {}", input_path.display(), e))?;
        return Ok((original_bytes, original_mime.to_string()));
    }

    // デコード
    let img = ImageReader::open(input_path)
        .map_err(|e| format!("画像デコードエラー: {}", e))?
        .decode()
        .map_err(|e| format!("画像デコードエラー: {}", e))?;

    // リサイズ
    let img = resize_if_needed(img, options);

    // 出力形式
    let output_format = options
        .output_format
        .as_deref()
        .unwrap_or(match original_mime {
            "image/jpeg" => "jpeg",
            "image/png" => "png",
            "image/webp" => "webp",
            _ => "jpeg",
        });

    let (encoded, mime) = encode_image(&img, output_format, options)?;
    log::info!(
        "画像圧縮: {} → {} bytes ({} → {})",
        input_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?"),
        encoded.len(),
        original_mime,
        mime
    );

    Ok((encoded, mime))
}

fn resize_if_needed(img: DynamicImage, options: &ImageCompressOptions) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    let max_w = options.max_width.unwrap_or(w);
    let max_h = options.max_height.unwrap_or(h);
    if w > max_w || h > max_h {
        img.resize(max_w, max_h, FilterType::Lanczos3)
    } else {
        img
    }
}

fn encode_image(
    img: &DynamicImage,
    format: &str,
    options: &ImageCompressOptions,
) -> Result<(Vec<u8>, String), String> {
    match format {
        "jpeg" | "jpg" => encode_jpeg(img, options),
        "png" => encode_png(img, options),
        "webp" => encode_webp(img, options),
        _ => Err(format!("未対応の出力形式: {}", format)),
    }
}

/// JPEG エンコード (zenjpeg を使用した高度な圧縮と品質制御)
fn encode_jpeg(
    img: &DynamicImage,
    options: &ImageCompressOptions,
) -> Result<(Vec<u8>, String), String> {
    let rgb_img = img.to_rgb8();
    let width = rgb_img.width();
    let height = rgb_img.height();
    let raw_pixels = rgb_img.as_raw();

    // クロマサブサンプリングの設定
    let subsampling = match options.jpeg_chroma_subsampling.as_str() {
        "4:2:2" => ChromaSubsampling::HalfHorizontal,
        "4:4:4" => ChromaSubsampling::None,
        _ => ChromaSubsampling::Quarter, // デフォルト "4:2:0"
    };

    // zenjpeg エンコーダの設定
    let config = EncoderConfig::ycbcr(options.jpeg_quality, subsampling)
        .progressive(options.jpeg_progressive)
        .huffman(options.jpeg_progressive || options.jpeg_auto_optimize)
        .aq_enabled(options.jpeg_auto_optimize)
        .deringing(options.jpeg_deringing)
        .separate_chroma_tables(options.jpeg_separate_chroma_tables)
        .sharp_yuv(options.jpeg_sharp_yuv);

    // エンコード処理の実行
    let mut enc = config
        .encode_from_bytes(width, height, PixelLayout::Rgb8Srgb)
        .map_err(|e| format!("zenjpeg エンコーダ作成エラー: {:?}", e))?;
    enc.push_packed(raw_pixels, Unstoppable)
        .map_err(|e| format!("zenjpeg エンコード処理エラー: {:?}", e))?;
    let jpeg_bytes = enc
        .finish()
        .map_err(|e| format!("zenjpeg エンコード完了エラー: {:?}", e))?;

    Ok((jpeg_bytes, "image/jpeg".to_string()))
}

/// PNG エンコード + oxipng による高度な最適化
fn encode_png(
    img: &DynamicImage,
    options: &ImageCompressOptions,
) -> Result<(Vec<u8>, String), String> {
    // まず image crate で PNG にエンコード
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Png)
        .map_err(|e| format!("PNGエンコードエラー: {}", e))?;
    let png_bytes = buf.into_inner();

    // 圧縮レベルの決定
    let preset = match options.png_compression.as_deref() {
        Some("0") => 0,
        Some("1") => 1,
        Some("2") => 2,
        Some("3") => 3,
        Some("4") => 4,
        Some("5") => 5,
        Some("6") => 6,
        _ => 2, // デフォルト
    };

    // oxipng オプションの構築
    let mut oxipng_options = oxipng::Options::from_preset(preset);

    // インターレース設定
    oxipng_options.interlace = Some(options.png_interlace);

    // メタデータ削除設定
    oxipng_options.strip = if options.png_strip {
        oxipng::StripChunks::Safe
    } else {
        oxipng::StripChunks::None
    };

    // 高度なオプションの適用
    oxipng_options.optimize_alpha = options.png_optimize_alpha;
    oxipng_options.bit_depth_reduction = options.png_bit_depth_reduction;
    oxipng_options.color_type_reduction = options.png_color_type_reduction;
    oxipng_options.palette_reduction = options.png_palette_reduction;
    oxipng_options.grayscale_reduction = options.png_grayscale_reduction;
    oxipng_options.idat_recoding = options.png_idat_recoding;
    oxipng_options.fast_evaluation = options.png_fast_evaluation;
    oxipng_options.force = options.png_force;
    oxipng_options.fix_errors = options.png_fix_errors;

    // oxipng で最適化
    match oxipng::optimize_from_memory(&png_bytes, &oxipng_options) {
        Ok(optimized) => {
            let saved = png_bytes.len() as f64 - optimized.len() as f64;
            let pct = (saved / png_bytes.len() as f64) * 100.0;
            log::info!(
                "  oxipng 最適化 (レベル {}): {:.1}% 削減 ({} → {} bytes)",
                preset,
                pct,
                png_bytes.len(),
                optimized.len()
            );
            Ok((optimized, "image/png".to_string()))
        }
        Err(e) => {
            log::warn!("oxipng 最適化スキップ: {} (元のPNGを使用)", e);
            Ok((png_bytes, "image/png".to_string()))
        }
    }
}

/// WebP エンコード (webp crate を使用した品質指定/可逆圧縮エンコード)
fn encode_webp(
    img: &DynamicImage,
    options: &ImageCompressOptions,
) -> Result<(Vec<u8>, String), String> {
    let encoder =
        webp::Encoder::from_image(img).map_err(|e| format!("WebPエンコーダ作成エラー: {:?}", e))?;

    let mut config = webp::WebPConfig::new().map_err(|_| "WebPConfig初期化エラー".to_string())?;

    // 基本設定
    config.lossless = if options.webp_lossless { 1 } else { 0 };
    config.quality = options.webp_quality as f32;

    // 高度な設定の適用
    config.method = options.webp_method;
    config.filter_strength = options.webp_filter_strength;
    config.filter_sharpness = options.webp_filter_sharpness;
    config.filter_type = options.webp_filter_type;
    config.sns_strength = options.webp_sns_strength;
    config.near_lossless = options.webp_near_lossless;
    config.exact = if options.webp_exact { 1 } else { 0 };
    config.use_sharp_yuv = if options.webp_use_sharp_yuv { 1 } else { 0 };

    let webp_data = encoder
        .encode_advanced(&config)
        .map_err(|e| format!("WebPエンコードエラー: {:?}", e))?;

    Ok((webp_data.to_vec(), "image/webp".to_string()))
}

pub fn mime_to_extension(mime: &str) -> &str {
    match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "bin",
    }
}

pub fn extension_to_mime(ext: &str) -> &str {
    match ext.to_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::{extension_to_mime, mime_to_extension, resize_if_needed};
    use crate::epub::intermediate::ImageCompressOptions;
    use image::DynamicImage;

    #[test]
    fn known_image_types_round_trip_to_their_epub_extensions() {
        for (extension, mime) in [
            ("jpeg", "image/jpeg"),
            ("PNG", "image/png"),
            ("webp", "image/webp"),
            ("gif", "image/gif"),
        ] {
            let expected_extension = if extension == "jpeg" {
                "jpg".to_string()
            } else {
                extension.to_lowercase()
            };
            assert_eq!(
                mime_to_extension(extension_to_mime(extension)),
                expected_extension
            );
            assert_eq!(extension_to_mime(mime_to_extension(mime)), mime);
        }
        assert_eq!(extension_to_mime("svg"), "application/octet-stream");
    }

    #[test]
    fn resize_never_enlarges_a_small_image() {
        let image = DynamicImage::new_rgb8(20, 10);
        let options = ImageCompressOptions {
            max_width: Some(100),
            max_height: Some(100),
            ..ImageCompressOptions::default()
        };
        let unchanged = resize_if_needed(image, &options);
        assert_eq!((unchanged.width(), unchanged.height()), (20, 10));
    }
}
