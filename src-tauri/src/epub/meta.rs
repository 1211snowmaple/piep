//! メタデータの正規化。
//!
//! EPUB のメタデータは書式が厳格で、`dcterms:modified` は秒までの UTC 表記
//! （`2018-08-21T12:52:01Z`）でなければならず、pixiv がそのまま返す
//! `2018-08-21T21:52:01+09:00` は検証に落ちる。識別子も同様に、空だったり
//! 作品間で重複したりすると取り込み側が同じ本とみなす。

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use sha2::{Digest, Sha256};

/// `dcterms:modified` が要求する秒精度の UTC 表記に直す。
pub fn to_utc_timestamp(value: &str) -> Option<String> {
    parse_datetime(value).map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// `dc:date` 向けに日付だけを取り出す。
pub fn to_date(value: &str) -> Option<String> {
    parse_datetime(value).map(|dt| dt.format("%Y-%m-%d").to_string())
}

/// 情報ページの表示用。解釈できなければ元の文字列をそのまま返す。
pub fn format_date_japanese(value: &str) -> String {
    match parse_datetime(value) {
        Some(dt) => dt.format("%Y年%-m月%-d日").to_string(),
        None => value.to_string(),
    }
}

fn parse_datetime(value: &str) -> Option<DateTime<Utc>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(trimmed) {
        return Some(parsed.with_timezone(&Utc));
    }
    // pixiv も FANBOX も RFC 3339 で返すが、取り込んだ書庫には素朴な表記も混ざる。
    for format in [
        "%Y-%m-%d %H:%M:%S",
        "%Y/%m/%d %H:%M:%S",
        "%Y-%m-%dT%H:%M:%S",
    ] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, format) {
            return Some(naive.and_utc());
        }
    }
    for format in ["%Y-%m-%d", "%Y/%m/%d"] {
        if let Ok(date) = NaiveDate::parse_from_str(trimmed, format) {
            return Some(date.and_hms_opt(0, 0, 0)?.and_utc());
        }
    }
    None
}

/// 作品を一意に指す URN。取得元と ID から作るので、同じ作品なら常に同じ値になる。
pub fn source_urn(source: &str, kind: &str, id: &str) -> String {
    if id.trim().is_empty() {
        // ID を持たない取り込みでも、本ごとに違う識別子は必要になる。
        return uuid_urn(&format!("{source}:{kind}"));
    }
    format!("urn:{source}:{kind}:{}", id.trim())
}

/// 与えた種から決まる UUID 形式の URN。
///
/// 同じ種からは常に同じ値が出るので、書き出し直しても本が別物にならない。
pub fn uuid_urn(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 の版とバリアントのビットを立て、UUID として妥当な形にする。
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// BCP 47 の言語タグとして通る形に整える。通らなければ既定へ落とす。
pub fn normalize_language(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    let shaped = trimmed.split('-').all(|part| {
        !part.is_empty() && part.len() <= 8 && part.chars().all(|c| c.is_ascii_alphanumeric())
    });
    if trimmed.is_empty() || !shaped {
        return fallback.to_string();
    }
    trimmed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixiv_timestamps_become_the_utc_form_the_spec_requires() {
        // ローカル時刻のまま書くと dcterms:modified が検証に落ちる。
        assert_eq!(
            to_utc_timestamp("2018-08-21T21:52:01+09:00").as_deref(),
            Some("2018-08-21T12:52:01Z")
        );
        assert_eq!(
            to_utc_timestamp("2024-01-02T03:04:05Z").as_deref(),
            Some("2024-01-02T03:04:05Z")
        );
        assert_eq!(
            to_date("2018-08-21T21:52:01+09:00").as_deref(),
            Some("2018-08-21")
        );
        assert_eq!(to_utc_timestamp(""), None);
        assert_eq!(to_utc_timestamp("いつか"), None);
    }

    #[test]
    fn bare_dates_and_loose_formats_still_parse() {
        assert_eq!(
            to_utc_timestamp("2020-05-06").as_deref(),
            Some("2020-05-06T00:00:00Z")
        );
        assert_eq!(
            to_utc_timestamp("2020-05-06 07:08:09").as_deref(),
            Some("2020-05-06T07:08:09Z")
        );
        assert_eq!(
            format_date_japanese("2018-08-21T21:52:01+09:00"),
            "2018年8月21日"
        );
        assert_eq!(format_date_japanese("不明"), "不明");
    }

    #[test]
    fn identifiers_are_stable_and_unique_per_work() {
        assert_eq!(source_urn("pixiv", "novel", "123"), "urn:pixiv:novel:123");
        assert_eq!(uuid_urn("seed"), uuid_urn("seed"));
        assert_ne!(uuid_urn("a"), uuid_urn("b"));
        let generated = uuid_urn("seed");
        assert!(generated.starts_with("urn:uuid:"));
        assert_eq!(generated.len(), "urn:uuid:".len() + 36);
        // 版は 4、バリアントは 8/9/a/b でなければ UUID として妥当にならない。
        assert_eq!(generated.as_bytes()[9 + 14] as char, '4');
        assert!(matches!(
            generated.as_bytes()[9 + 19] as char,
            '8' | '9' | 'a' | 'b'
        ));
    }

    #[test]
    fn languages_fall_back_when_unusable() {
        assert_eq!(normalize_language("ja", "ja"), "ja");
        assert_eq!(normalize_language("zh-Hant", "ja"), "zh-Hant");
        assert_eq!(normalize_language("日本語", "ja"), "ja");
        assert_eq!(normalize_language("", "ja"), "ja");
    }
}
