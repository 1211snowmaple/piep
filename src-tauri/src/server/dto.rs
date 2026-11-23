//! 携帯の一覧の条件（`MobileSearchRequest`）と、その検査。棚の問い合わせへは `reader::list` が直す。
//! 返す型は `crates/piep-mobile-api` にある（携帯の Rust と画面の型が同じ定義を使う）。

use chrono::NaiveDate;
use serde::Deserialize;

/// Keep numbered-page jumps within the same work budget as the desktop UI.
const MAX_NUMBERED_OFFSET: i64 = 5_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MobileSearchRequest {
    pub text: Option<String>,
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
    pub limit: Option<i64>,
    pub source: Option<String>,
    pub content_type: Option<String>,
    pub authors_include: Option<Vec<String>>,
    pub authors_exclude: Option<Vec<String>>,
    pub tags_include: Option<Vec<String>>,
    pub tags_exclude: Option<Vec<String>>,
    pub tag_filter_mode: Option<String>,
    pub min_char_count: Option<i64>,
    pub max_char_count: Option<i64>,
    pub min_asset_count: Option<i64>,
    pub max_asset_count: Option<i64>,
    pub revision_filter: Option<String>,
    pub edit_filter: Option<String>,
    pub cover_filter: Option<String>,
    pub date_field: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
    pub favorite: Option<bool>,
    pub sort_by: Option<String>,
    pub sort_order: Option<String>,
    pub person_source: Option<String>,
    pub person_key: Option<String>,
    pub series_source: Option<String>,
    pub series_key: Option<String>,
}

impl MobileSearchRequest {
    /// 受け取った条件の長さ・範囲・語彙を確かめる。
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
                .is_some_and(|value| value.chars().count() > 200)
            {
                return Err("作者・シリーズの識別子が長すぎます".into());
            }
        }
        for source in [&self.person_source, &self.series_source] {
            if source
                .as_ref()
                .is_some_and(|value| !matches!(value.as_str(), "pixiv" | "fanbox"))
            {
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
        if [self.min_char_count, self.max_char_count]
            .into_iter()
            .flatten()
            .any(|value| !(0..=1_000_000_000).contains(&value))
            || self
                .min_char_count
                .zip(self.max_char_count)
                .is_some_and(|(min, max)| min > max)
        {
            return Err("文字数の範囲が正しくありません".into());
        }
        if [self.min_asset_count, self.max_asset_count]
            .into_iter()
            .flatten()
            .any(|value| !(0..=1_000_000_000).contains(&value))
            || self
                .min_asset_count
                .zip(self.max_asset_count)
                .is_some_and(|(min, max)| min > max)
        {
            return Err("添付数の範囲が正しくありません".into());
        }
        fn allowed(value: Option<&str>, choices: &[&str]) -> bool {
            value.is_none_or(|value| choices.contains(&value))
        }
        if !allowed(self.source.as_deref(), &["pixiv", "fanbox"])
            || !allowed(
                self.content_type.as_deref(),
                &["novel", "article", "text", "image", "file"],
            )
            || !allowed(self.tag_filter_mode.as_deref(), &["and", "or"])
            || !allowed(
                self.revision_filter.as_deref(),
                &["revised", "first_version"],
            )
            || !allowed(self.edit_filter.as_deref(), &["edited", "unedited"])
            || !allowed(self.cover_filter.as_deref(), &["has_cover", "no_cover"])
            || !allowed(
                self.date_field.as_deref(),
                &["downloaded_at", "source_created_at", "source_updated_at"],
            )
            || !allowed(
                self.sort_by.as_deref(),
                &[
                    "date",
                    "published",
                    "title",
                    "author",
                    "length",
                    "assets",
                    "version",
                    "series_order",
                ],
            )
            || !allowed(self.sort_order.as_deref(), &["asc", "desc"])
        {
            return Err("検索条件の値が正しくありません".into());
        }
        let parse_date = |value: Option<&String>| -> Result<Option<NaiveDate>, String> {
            let Some(value) = value else {
                return Ok(None);
            };
            if value.len() != 10
                || value.starts_with("0000")
                || value.as_bytes()[4] != b'-'
                || value.as_bytes()[7] != b'-'
            {
                return Err("日付の形式が正しくありません".into());
            }
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .map(Some)
                .map_err(|_| "日付の形式が正しくありません".into())
        };
        let date_from = parse_date(self.date_from.as_ref())?;
        let date_to = parse_date(self.date_to.as_ref())?;
        if date_from.zip(date_to).is_some_and(|(from, to)| from > to) {
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
    fn known_filters_pass() {
        assert!(parse(serde_json::json!({
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
            "dateField": "source_created_at",
            "dateFrom": "2024-02-29",
            "dateTo": "2026-10-14",
            "sortBy": "published",
            "sortOrder": "asc"
        }))
        .validate()
        .is_ok());
    }

    #[test]
    fn search_rejects_invalid_non_ai_filters() {
        for request in [
            serde_json::json!({ "tagFilterMode": "xor" }),
            serde_json::json!({ "minAssetCount": -1 }),
            serde_json::json!({ "minAssetCount": 3, "maxAssetCount": 2 }),
            serde_json::json!({ "revisionFilter": "both" }),
            serde_json::json!({ "editFilter": "all" }),
            serde_json::json!({ "coverFilter": "maybe" }),
            serde_json::json!({ "dateField": "private_path" }),
            serde_json::json!({ "dateFrom": "2026-02-30" }),
            serde_json::json!({ "dateFrom": "0000-01-01" }),
            serde_json::json!({ "dateFrom": "2026-10-15", "dateTo": "2026-10-14" }),
            serde_json::json!({ "sortBy": "json_path" }),
            serde_json::json!({ "tagsInclude": [" "] }),
            serde_json::json!({ "minCharCount": 5001, "maxCharCount": 5000 }),
        ] {
            let parsed: MobileSearchRequest =
                serde_json::from_value(request.clone()).expect("request shape");
            assert!(parsed.validate().is_err(), "accepted {request}");
        }
    }
}
