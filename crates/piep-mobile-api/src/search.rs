//! 携帯の一覧の条件と、その検査。
//!
//! 携帯が組み、サーバーが受ける。どちらも同じ `validate` を通すので、送る前に断られる条件と
//! サーバーで断られる条件が食い違わない。並べ方・日付の欄の名前は、デスクトップの棚の
//! 並べ方（`WorkSort`・`DateField`）と同じ。

use serde::{Deserialize, Serialize};

/// 番号のページで飛べる先の上限。デスクトップの画面と同じ。
pub const MAX_NUMBERED_OFFSET: i64 = 5_000;

/// 作品の並べ方。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileWorkSort {
    Saved,
    Published,
    Updated,
    Title,
    Author,
    Length,
    Size,
    Media,
    Version,
    SeriesOrder,
}

impl MobileWorkSort {
    /// 向きを指さないときの向き（題・作者名・シリーズの順だけ昇順）。
    pub fn ascending_by_default(self) -> bool {
        matches!(self, Self::Title | Self::Author | Self::SeriesOrder)
    }
}

/// 作者・シリーズの並べ方。中の作品から決まる鍵は、作品の並べ方と同じ名前。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileEntitySort {
    WorkCount,
    Saved,
    Published,
    Updated,
    Name,
    Length,
    Size,
    Media,
    Version,
}

/// 日付の絞り込みに使う欄。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileDateField {
    #[default]
    Saved,
    Published,
    Updated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileSortOrder {
    Asc,
    Desc,
}

/// 一覧の一ページの条件。空の条件は送らない。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MobileSearchRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authors_include: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authors_exclude: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags_include: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags_exclude: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_filter_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_char_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_char_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_asset_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_asset_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_filter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edit_filter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_filter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_field: Option<MobileDateField>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favorite: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_by: Option<MobileWorkSort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort_order: Option<MobileSortOrder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_key: Option<String>,
}

/// 取得元の名前。
pub fn valid_source(value: &str) -> bool {
    matches!(value, "pixiv" | "fanbox")
}

/// `YYYY-MM-DD` の実在する日付か。
pub fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit())
    {
        return false;
    }
    let number = |range: std::ops::Range<usize>| value[range].parse::<u32>().unwrap_or(0);
    let (year, month, day) = (number(0..4), number(5..7), number(8..10));
    if year == 0 {
        return false;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let last = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=last).contains(&day)
}

impl MobileSearchRequest {
    /// 条件の長さ・範囲・語彙を確かめる。
    pub fn validate(&self) -> Result<(), String> {
        if self
            .text
            .as_ref()
            .is_some_and(|text| text.chars().count() > 200)
        {
            return Err("検索語は 200 文字以下にしてください".into());
        }
        if self
            .cursor
            .as_ref()
            .is_some_and(|cursor| cursor.len() > 4096)
        {
            return Err("検索カーソルが長すぎます".into());
        }
        if self.cursor.is_some() && self.offset.is_some() {
            return Err("検索カーソルとページ番号は同時に指定できません".into());
        }
        if self
            .offset
            .is_some_and(|offset| !(0..=MAX_NUMBERED_OFFSET).contains(&offset))
        {
            return Err("ページ番号の範囲が正しくありません".into());
        }
        for key in [&self.person_key, &self.series_key] {
            if key
                .as_ref()
                .is_some_and(|value| value.is_empty() || value.chars().count() > 200)
            {
                return Err("作者・シリーズの識別子が正しくありません".into());
            }
        }
        for source in [&self.person_source, &self.series_source, &self.source] {
            if source.as_ref().is_some_and(|value| !valid_source(value)) {
                return Err("取得元が正しくありません".into());
            }
        }
        for terms in [
            &self.authors_include,
            &self.authors_exclude,
            &self.tags_include,
            &self.tags_exclude,
        ] {
            if terms.as_ref().is_some_and(|items| {
                items.len() > 10
                    || items.iter().any(|item| {
                        item.trim().is_empty() || item != item.trim() || item.chars().count() > 100
                    })
            }) {
                return Err("作者・タグの条件が長すぎます".into());
            }
        }
        let bad_range = |min: Option<i64>, max: Option<i64>| {
            [min, max]
                .into_iter()
                .flatten()
                .any(|value| !(0..=1_000_000_000).contains(&value))
                || min.zip(max).is_some_and(|(min, max)| min > max)
        };
        if bad_range(self.min_char_count, self.max_char_count) {
            return Err("文字数の範囲が正しくありません".into());
        }
        if bad_range(self.min_asset_count, self.max_asset_count) {
            return Err("添付数の範囲が正しくありません".into());
        }
        fn allowed(value: Option<&str>, choices: &[&str]) -> bool {
            value.is_none_or(|value| choices.contains(&value))
        }
        if !allowed(
            self.content_type.as_deref(),
            &["novel", "article", "text", "image", "file"],
        ) || !allowed(self.tag_filter_mode.as_deref(), &["and", "or"])
            || !allowed(
                self.revision_filter.as_deref(),
                &["revised", "first_version"],
            )
            || !allowed(self.edit_filter.as_deref(), &["edited", "unedited"])
            || !allowed(self.cover_filter.as_deref(), &["has_cover", "no_cover"])
        {
            return Err("検索条件の値が正しくありません".into());
        }
        for date in [&self.date_from, &self.date_to] {
            if date.as_ref().is_some_and(|value| !valid_date(value)) {
                return Err("日付の形式が正しくありません".into());
            }
        }
        // 同じ形の日付は、文字列の大小と日付の前後が一致する。
        if self
            .date_from
            .as_ref()
            .zip(self.date_to.as_ref())
            .is_some_and(|(from, to)| from > to)
        {
            return Err("日付の範囲が正しくありません".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(value: serde_json::Value) -> MobileSearchRequest {
        serde_json::from_value(value).expect("request")
    }

    #[test]
    fn oversized_query_and_out_of_range_pages_are_rejected() {
        assert!(parse(serde_json::json!({ "text": "海", "limit": 500 }))
            .validate()
            .is_ok());
        assert!(parse(serde_json::json!({ "text": "海".repeat(201) }))
            .validate()
            .is_err());
        for request in [
            serde_json::json!({ "offset": -1 }),
            serde_json::json!({ "offset": MAX_NUMBERED_OFFSET + 1 }),
            serde_json::json!({ "cursor": "after", "offset": 20 }),
        ] {
            assert!(parse(request).validate().is_err());
        }
        assert!(parse(serde_json::json!({ "text": "海", "offset": 40 }))
            .validate()
            .is_ok());
    }

    #[test]
    fn known_filters_pass_and_round_trip() {
        let request = parse(serde_json::json!({
            "authorsInclude": ["作者A", "作者B"],
            "authorsExclude": ["除外する作者"],
            "tagsInclude": ["猫", "海"],
            "tagsExclude": ["除外するタグ"],
            "tagFilterMode": "or",
            "minCharCount": 1000,
            "maxCharCount": 5000,
            "minAssetCount": 2,
            "maxAssetCount": 12,
            "revisionFilter": "revised",
            "editFilter": "edited",
            "coverFilter": "has_cover",
            "dateField": "published",
            "dateFrom": "2024-02-29",
            "dateTo": "2026-10-14",
            "sortBy": "series_order",
            "sortOrder": "asc"
        }));
        assert!(request.validate().is_ok());
        assert_eq!(request.sort_by, Some(MobileWorkSort::SeriesOrder));
        assert_eq!(request.date_field, Some(MobileDateField::Published));
        let again: MobileSearchRequest =
            serde_json::from_value(serde_json::to_value(&request).unwrap()).unwrap();
        assert_eq!(again, request);
        // 空の条件は送らない。
        assert_eq!(
            serde_json::to_value(MobileSearchRequest::default()).unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn invalid_filters_are_rejected() {
        for request in [
            serde_json::json!({ "tagFilterMode": "xor" }),
            serde_json::json!({ "minAssetCount": -1 }),
            serde_json::json!({ "minAssetCount": 3, "maxAssetCount": 2 }),
            serde_json::json!({ "revisionFilter": "both" }),
            serde_json::json!({ "editFilter": "all" }),
            serde_json::json!({ "coverFilter": "maybe" }),
            serde_json::json!({ "source": "other" }),
            serde_json::json!({ "dateFrom": "2026-02-30" }),
            serde_json::json!({ "dateFrom": "0000-01-01" }),
            serde_json::json!({ "dateFrom": "2026-10-15", "dateTo": "2026-10-14" }),
            serde_json::json!({ "tagsInclude": [" "] }),
            serde_json::json!({ "minCharCount": 5001, "maxCharCount": 5000 }),
            serde_json::json!({ "personKey": "" }),
        ] {
            let parsed: MobileSearchRequest =
                serde_json::from_value(request.clone()).expect("request shape");
            assert!(parsed.validate().is_err(), "accepted {request}");
        }
        // 語彙の外の並べ方・日付の欄は、形の段階で断る。
        for request in [
            serde_json::json!({ "sortBy": "json_path" }),
            serde_json::json!({ "sortBy": "downloaded_at" }),
            serde_json::json!({ "dateField": "private_path" }),
        ] {
            assert!(serde_json::from_value::<MobileSearchRequest>(request).is_err());
        }
        assert!(valid_date("2024-02-29") && !valid_date("2023-02-29") && !valid_date("2026-13-01"));
    }
}
