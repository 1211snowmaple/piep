//! 棚のファイルの種類を中身から決める。棚のファイルは種類を持たない（中身で指す）ので、
//! デスクトップの表示・EPUB・携帯の保存がみな同じここで見分ける。

/// 画像の media-type を先頭のバイト列から決める。分からなければ `None`
/// （呼び手は推測させずに断るか、自分の決め方に戻る）。
pub fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', b'7' | b'9', b'a', ..] => Some("image/gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        [_, _, _, _, b'f', b't', b'y', b'p', b'a', b'v', b'i', b'f' | b's', ..] => {
            Some("image/avif")
        }
        [b'B', b'M', ..] => Some("image/bmp"),
        _ => None,
    }
}

/// 見分けに要る先頭の長さ。ファイルから読むときはここまで読めば足りる。
pub const SNIFF_BYTES: usize = 12;

#[cfg(test)]
mod tests {
    use super::sniff_image;

    #[test]
    fn images_are_named_by_their_first_bytes() {
        assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\n\0\0"), Some("image/png"));
        assert_eq!(sniff_image(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(sniff_image(b"GIF89a"), Some("image/gif"));
        assert_eq!(sniff_image(b"GIF87a"), Some("image/gif"));
        assert_eq!(sniff_image(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_image(b"\0\0\0\x1cftypavif"), Some("image/avif"));
        assert_eq!(sniff_image(b"BM\0\0"), Some("image/bmp"));
    }

    #[test]
    fn other_bytes_are_left_unnamed() {
        for bytes in [&b"<html>"[..], b"", b"\x89PN", b"GIF8", b"RIFF\0\0\0\0WAVE"] {
            assert_eq!(sniff_image(bytes), None);
        }
    }
}
