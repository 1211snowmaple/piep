pub use piep_storage::Fingerprint;

/// 棚のコピーや端末間の送受信でも変わらない識別子。
pub type LibraryId = uuid::Uuid;

/// pixiv の既知の表紙・挿絵形式だけ、品質によらない識別子を作る。
/// 未知の URL は厳密比較へ戻し、他の画像を同じものと推測しない。
pub fn cover_identity(provider: &str, url: &str) -> String {
    image_identity(provider, url)
}

pub fn image_identity(provider: &str, url: &str) -> String {
    if provider == "pixiv" {
        if let Ok(parsed) = url::Url::parse(url) {
            if parsed.scheme() == "https"
                && parsed.host_str() == Some("i.pximg.net")
                && parsed.port().is_none()
                && parsed.username().is_empty()
                && parsed.password().is_none()
                && parsed.query().is_none()
                && parsed.fragment().is_none()
            {
                let path = parsed.path();
                let rest = path.strip_prefix("/c/").and_then(|p| p.split_once('/'));
                let path = rest.map_or(path, |(_, path)| path);
                let path = path.trim_start_matches('/');
                let (kind, original, master) = if path.starts_with("novel-cover-") {
                    (
                        "novel-cover",
                        path.strip_prefix("novel-cover-original/img/"),
                        path.strip_prefix("novel-cover-master/img/"),
                    )
                } else {
                    (
                        "illustration",
                        path.strip_prefix("img-original/img/"),
                        path.strip_prefix("img-master/img/"),
                    )
                };
                let stem = original.or(master).and_then(|p| {
                    let (stem, ext) = p.rsplit_once('.')?;
                    matches!(ext, "jpg" | "jpeg" | "png" | "gif").then_some(stem)
                });
                if let Some(stem) = stem {
                    let stem = if master.is_some() {
                        stem.strip_suffix("_master1200")
                    } else {
                        Some(stem)
                    };
                    if let Some(stem) = stem {
                        // 日時と固有名を含む既知の形に限る。形式の拡張は試験と一緒に行う。
                        let parts: Vec<_> = stem.split('/').collect();
                        if parts.len() == 7
                            && parts[..6]
                                .iter()
                                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                            && !parts[6].is_empty()
                            && parts[6]
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                        {
                            if kind == "illustration"
                                && !parts[6].split_once("_p").is_some_and(|(id, page)| {
                                    !id.is_empty()
                                        && !page.is_empty()
                                        && id.bytes().all(|b| b.is_ascii_digit())
                                        && page.bytes().all(|b| b.is_ascii_digit())
                                })
                            {
                                return format!("url:v1:{provider}:{url}");
                            }
                            return format!("pixiv:{kind}:v1:{stem}");
                        }
                    }
                }
            }
        }
    }
    format!("url:v1:{provider}:{url}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_quality_is_independent_of_author_changes() {
        let base = "2024/11/15/19/39/13/ci23413112_bbb0c381";
        let thumb = format!(
            "https://i.pximg.net/c/240x480_80/novel-cover-master/img/{base}_master1200.jpg"
        );
        let original = format!("https://i.pximg.net/novel-cover-original/img/{base}.png");
        assert_eq!(
            cover_identity("pixiv", &thumb),
            cover_identity("pixiv", &original)
        );
        assert_ne!(
            cover_identity("pixiv", &original),
            cover_identity("pixiv", &original.replace("bbb0", "cccc"))
        );
        for unknown in [
            original.replace("i.pximg.net", "elsewhere.invalid"),
            format!("{original}?unknown=1"),
            "https://i.pximg.net/novel-cover-original/img/custom.png".into(),
        ] {
            assert!(cover_identity("pixiv", &unknown).starts_with("url:v1:"));
        }
    }

    #[test]
    fn file_ids_cannot_be_paths() {
        let id = Fingerprint::of(b"original\0bytes");
        assert_eq!(Fingerprint::parse(&id.to_string()).unwrap(), id);
        for invalid in ["../test", "", "こんにちは", &"g".repeat(64)] {
            assert!(Fingerprint::parse(invalid).is_err());
        }
    }

    #[test]
    fn illustration_originals_and_masters_share_identity_but_pages_and_replacements_do_not() {
        let thumb="https://i.pximg.net/c/1200x1200/img-master/img/2026/01/02/03/04/05/123_p0_master1200.jpg";
        let original = "https://i.pximg.net/img-original/img/2026/01/02/03/04/05/123_p0.png";
        assert_eq!(
            image_identity("pixiv", thumb),
            image_identity("pixiv", original)
        );
        assert_ne!(
            image_identity("pixiv", original),
            image_identity("pixiv", &original.replace("_p0", "_p1"))
        );
        assert_ne!(
            image_identity("pixiv", original),
            image_identity("pixiv", &original.replace("03/04/05", "03/04/06"))
        );
        for unknown in [
            original.replace("123_p0", "unknown"),
            format!("{original}?unknown=1"),
        ] {
            assert!(image_identity("pixiv", &unknown).starts_with("url:v1:"));
        }
    }
}
