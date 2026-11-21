//! 携帯の一覧の条件（`MobileSearchRequest`）。受け取った条件を検査し、検索の条件へ直す。
//! 返す型は `crates/piep-mobile-api` にある（携帯の Rust と画面の型が同じ定義を使う）。

use crate::database::SearchV2Params;
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
    /// モバイル API で受け取る条件を絞り、PC 版の唯一の検索入口に渡す。
    pub fn into_db_params(self) -> Result<SearchV2Params, String> {
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
        let mut params = serde_json::json!({
            "text": self.text,
            "cursor": self.cursor,
            "limit": self.limit.unwrap_or(if self.offset.is_some() { 20 } else { 30 }).clamp(1, 50),
            "offset": self.offset,
            "source": self.source,
            "contentType": self.content_type,
            "authorsInclude": self.authors_include,
            "authorsExclude": self.authors_exclude,
            "tagsInclude": self.tags_include,
            "tagsExclude": self.tags_exclude,
            "tagFilterMode": self.tag_filter_mode,
            "minCharCount": self.min_char_count,
            "maxCharCount": self.max_char_count,
            "minAssetCount": self.min_asset_count,
            "maxAssetCount": self.max_asset_count,
            "revisionFilter": self.revision_filter,
            "editFilter": self.edit_filter,
            "coverFilter": self.cover_filter,
            "dateField": self.date_field,
            "dateFrom": self.date_from,
            "dateTo": self.date_to,
            "favorite": self.favorite,
            "sortBy": self.sort_by,
            "sortOrder": self.sort_order,
            "personSource": self.person_source,
            "personKey": self.person_key,
            "seriesSource": self.series_source,
            "seriesKey": self.series_key,
            "projection": "libraryCompact",
        });
        // Older clients omit offset; keep that wire shape unchanged.
        if self.offset.is_none() {
            params.as_object_mut().unwrap().remove("offset");
        }
        serde_json::from_value(params).map_err(|_| "検索条件を読み取れません".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_input_caps_page_size_and_rejects_oversized_query() {
        let request: MobileSearchRequest = serde_json::from_value(serde_json::json!({
            "text": "海", "limit": 500
        }))
        .expect("request");
        assert_eq!(request.into_db_params().expect("params").limit, Some(50));
        let oversized: MobileSearchRequest = serde_json::from_value(serde_json::json!({
            "text": "海".repeat(201)
        }))
        .expect("request");
        assert!(oversized.into_db_params().is_err());
    }

    #[test]
    fn numbered_search_pages_default_to_twenty_and_accept_legacy_requests() {
        let legacy: MobileSearchRequest = serde_json::from_value(serde_json::json!({
            "text": "海"
        }))
        .expect("legacy request without offset");
        let legacy_params = legacy.into_db_params().expect("legacy params");
        assert_eq!(legacy_params.offset, None);
        assert_eq!(legacy_params.limit, Some(30));

        let numbered: MobileSearchRequest = serde_json::from_value(serde_json::json!({
            "text": "海", "offset": 40
        }))
        .expect("numbered request");
        let numbered_params = numbered.into_db_params().expect("numbered params");
        assert_eq!(numbered_params.offset, Some(40));
        assert_eq!(numbered_params.limit, Some(20));
    }

    #[test]
    fn numbered_search_rejects_out_of_range_offsets_and_cursor_combinations() {
        for request in [
            serde_json::json!({ "offset": -1 }),
            serde_json::json!({ "offset": MAX_NUMBERED_OFFSET + 1 }),
            serde_json::json!({ "cursor": "after", "offset": 20 }),
        ] {
            let parsed: MobileSearchRequest = serde_json::from_value(request).expect("request");
            assert!(parsed.into_db_params().is_err());
        }
    }

    #[test]
    fn search_forwards_exclusions_and_character_range() {
        let request: MobileSearchRequest = serde_json::from_value(serde_json::json!({
            "authorsExclude": ["除外する作者"],
            "tagsExclude": ["除外するタグ"],
            "minCharCount": 1000,
            "maxCharCount": 5000
        }))
        .expect("request");
        let params = request.into_db_params().expect("params");
        assert_eq!(params.authors_exclude, Some(vec!["除外する作者".into()]));
        assert_eq!(params.tags_exclude, Some(vec!["除外するタグ".into()]));
        assert_eq!(params.min_char_count, Some(1000));
        assert_eq!(params.max_char_count, Some(5000));

        let invalid: MobileSearchRequest = serde_json::from_value(serde_json::json!({
            "minCharCount": 5001,
            "maxCharCount": 5000
        }))
        .expect("request");
        assert!(invalid.into_db_params().is_err());
    }

    #[test]
    fn search_forwards_non_ai_filters_to_existing_search() {
        let request: MobileSearchRequest = serde_json::from_value(serde_json::json!({
            "authorsInclude": ["作者A", "作者B"],
            "tagsInclude": ["猫", "海"],
            "tagFilterMode": "or",
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
        .expect("request");
        let params = request.into_db_params().expect("params");
        assert_eq!(
            params.authors_include,
            Some(vec!["作者A".into(), "作者B".into()])
        );
        assert_eq!(params.tags_include, Some(vec!["猫".into(), "海".into()]));
        assert_eq!(params.tag_filter_mode.as_deref(), Some("or"));
        assert_eq!(params.min_asset_count, Some(2));
        assert_eq!(params.max_asset_count, Some(12));
        assert_eq!(params.revision_filter.as_deref(), Some("revised"));
        assert_eq!(params.edit_filter.as_deref(), Some("edited"));
        assert_eq!(params.cover_filter.as_deref(), Some("has_cover"));
        assert_eq!(params.date_field.as_deref(), Some("source_created_at"));
        assert_eq!(params.date_from.as_deref(), Some("2024-02-29"));
        assert_eq!(params.date_to.as_deref(), Some("2026-10-14"));
        assert_eq!(params.sort_by.as_deref(), Some("published"));
        assert_eq!(params.sort_order.as_deref(), Some("asc"));
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
        ] {
            let parsed: MobileSearchRequest =
                serde_json::from_value(request.clone()).expect("request shape");
            assert!(parsed.into_db_params().is_err(), "accepted {request}");
        }
    }
}
