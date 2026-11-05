//! 保存前の実デコード。受信バイトのハッシュだけでは元から壊れた画像を検出できない。
use super::ClientError;
use image::{AnimationDecoder, ImageDecoder, ImageFormat, ImageReader, Limits};
use std::io::Cursor;

const MAX_DIMENSION: u32 = 8192;
const MAX_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_DECODE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ANIMATION_PIXELS: u64 = 32 * 1024 * 1024;
const MAX_FRAMES: usize = 256;

fn invalid_image() -> ClientError {
    ClientError {
        code: "invalid_offline_image",
        message: "画像が壊れているか、端末で検証できる大きさを超えています。保存済みの版はそのまま保持しました",
    }
}

fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    limits
}

fn check_decoder(decoder: &mut impl ImageDecoder) -> Result<(), ClientError> {
    decoder.set_limits(limits()).map_err(|_| invalid_image())?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS
        || decoder.total_bytes() > MAX_DECODE_BYTES {
        return Err(invalid_image());
    }
    Ok(())
}

fn check_frames(frames: image::Frames<'_>) -> Result<(), ClientError> {
    let mut total_pixels = 0u64;
    let mut count = 0;
    for frame in frames {
        let frame = frame.map_err(|_| invalid_image())?;
        count += 1;
        total_pixels += u64::from(frame.buffer().width()) * u64::from(frame.buffer().height());
        if count > MAX_FRAMES || total_pixels > MAX_ANIMATION_PIXELS { return Err(invalid_image()); }
    }
    if count == 0 { return Err(invalid_image()); }
    Ok(())
}

/// 画面へ出す前の検査。保存と同じ上限を見るが、画像は WebView がどのみち展開するので
/// 一枚ごとに全画素を展開はしない。枚数の多い動く画像だけは中身を数える。
pub(super) fn validate_display_image(bytes: &[u8], mime: &str) -> Result<(), ClientError> {
    let expected = match mime {
        "image/jpeg" => ImageFormat::Jpeg,
        "image/png" => ImageFormat::Png,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::WebP,
        // 大きさを確かめる手段がない形式は、保存と同じく通さない。
        _ => return Err(invalid_image()),
    };
    if image::guess_format(bytes).map_err(|_| invalid_image())? != expected {
        return Err(invalid_image());
    }
    let cursor = Cursor::new(bytes);
    match expected {
        ImageFormat::Gif => {
            let mut decoder = image::codecs::gif::GifDecoder::new(cursor).map_err(|_| invalid_image())?;
            check_decoder(&mut decoder)?;
            check_frames(decoder.into_frames())
        }
        ImageFormat::WebP => {
            let mut decoder = image::codecs::webp::WebPDecoder::new(cursor).map_err(|_| invalid_image())?;
            check_decoder(&mut decoder)?;
            if decoder.has_animation() {
                return check_frames(decoder.into_frames());
            }
            Ok(())
        }
        ImageFormat::Png => {
            let mut decoder = image::codecs::png::PngDecoder::with_limits(cursor, limits()).map_err(|_| invalid_image())?;
            check_decoder(&mut decoder)
        }
        _ => {
            let mut decoder = image::codecs::jpeg::JpegDecoder::new(cursor).map_err(|_| invalid_image())?;
            check_decoder(&mut decoder)
        }
    }
}

pub(super) fn validate_saved_image(bytes: &[u8], mime: &str) -> Result<(), ClientError> {
    if mime == "image/avif" {
        return Err(ClientError {
            code: "unsupported_offline_image",
            message: "AVIF は保存前の画像検証に対応していません。デスクトップ版で PNG・WebP 等に変換してください。保存済みの版はそのまま保持しました",
        });
    }
    let expected = match mime {
        "image/jpeg" => ImageFormat::Jpeg,
        "image/png" => ImageFormat::Png,
        "image/gif" => ImageFormat::Gif,
        "image/webp" => ImageFormat::WebP,
        _ => return Err(invalid_image()),
    };
    if image::guess_format(bytes).map_err(|_| invalid_image())? != expected { return Err(invalid_image()); }
    // デコーダが画像の最後のピクセルまで読んでも、末尾欠損を許容する形式を補う。
    let complete = match expected {
        ImageFormat::Png => bytes.ends_with(b"\0\0\0\0IEND\xaeB`\x82"),
        ImageFormat::Jpeg => bytes.ends_with(&[0xff, 0xd9]),
        ImageFormat::Gif => bytes.last() == Some(&0x3b),
        ImageFormat::WebP => bytes.get(4..8).and_then(|value| value.try_into().ok())
            .is_some_and(|value| u64::from(u32::from_le_bytes(value)) + 8 == bytes.len() as u64),
        _ => false,
    };
    if !complete { return Err(invalid_image()); }
    let cursor = Cursor::new(bytes);
    match expected {
        ImageFormat::Gif => {
            let mut decoder = image::codecs::gif::GifDecoder::new(cursor).map_err(|_| invalid_image())?;
            check_decoder(&mut decoder)?;
            check_frames(decoder.into_frames())
        }
        ImageFormat::WebP => {
            let mut decoder = image::codecs::webp::WebPDecoder::new(cursor).map_err(|_| invalid_image())?;
            check_decoder(&mut decoder)?;
            if decoder.has_animation() { return check_frames(decoder.into_frames()); }
            image::DynamicImage::from_decoder(decoder).map_err(|_| invalid_image())?;
            Ok(())
        }
        ImageFormat::Png => {
            let mut decoder = image::codecs::png::PngDecoder::with_limits(cursor, limits()).map_err(|_| invalid_image())?;
            check_decoder(&mut decoder)?;
            if decoder.is_apng().map_err(|_| invalid_image())? {
                return check_frames(decoder.apng().map_err(|_| invalid_image())?.into_frames());
            }
            image::DynamicImage::from_decoder(decoder).map_err(|_| invalid_image())?;
            Ok(())
        }
        _ => {
            let mut reader = ImageReader::with_format(cursor, expected);
            reader.limits(limits());
            let mut decoder = reader.into_decoder().map_err(|_| invalid_image())?;
            check_decoder(&mut decoder)?;
            image::DynamicImage::from_decoder(decoder).map_err(|_| invalid_image())?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(format: ImageFormat, width: u32, height: u32) -> Vec<u8> {
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(width, height, image::Rgb([20, 80, 160])));
        let mut cursor = Cursor::new(Vec::new());
        image.write_to(&mut cursor, format).unwrap();
        cursor.into_inner()
    }

    #[test]
    fn decodes_supported_formats_and_rejects_empty_truncated_and_mislabeled_images() {
        for (format, mime) in [(ImageFormat::Png, "image/png"), (ImageFormat::Jpeg, "image/jpeg"), (ImageFormat::Gif, "image/gif"), (ImageFormat::WebP, "image/webp")] {
            let bytes = encoded(format, 3, 4);
            validate_saved_image(&bytes, mime).unwrap();
            assert!(validate_saved_image(&[], mime).is_err());
            assert!(validate_saved_image(&bytes[..bytes.len() / 2], mime).is_err());
            assert!(validate_saved_image(&bytes[..bytes.len() - 1], mime).is_err());
        }
        let png = encoded(ImageFormat::Png, 3, 4);
        assert!(validate_saved_image(&png, "image/jpeg").is_err());
        assert_eq!(validate_saved_image(&png, "image/avif").unwrap_err().code, "unsupported_offline_image");
    }

    #[test]
    fn refuses_dimensions_and_animation_work_above_the_budget() {
        assert!(validate_saved_image(&encoded(ImageFormat::Png, MAX_DIMENSION + 1, 1), "image/png").is_err());
        let frame = image::Frame::new(image::RgbaImage::new(1, 1));
        let mut bytes = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut bytes).encode_frames(std::iter::repeat_n(frame, MAX_FRAMES + 1)).unwrap();
        assert!(validate_saved_image(&bytes, "image/gif").is_err());
    }

    #[test]
    fn rejects_the_probe_png_crc_error_but_accepts_its_repaired_bytes() {
        use base64::Engine as _;
        let mut bytes = base64::engine::general_purpose::STANDARD.decode(
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/lXcAAAAASUVORK5CYII="
        ).unwrap();
        assert!(validate_saved_image(&bytes, "image/png").is_err());
        // 元の IDAT CRC = efbf9577。type+data の正しい CRC は efa2a75b。
        bytes[52..56].copy_from_slice(&[0xef, 0xa2, 0xa7, 0x5b]);
        validate_saved_image(&bytes, "image/png").unwrap();
    }
}
