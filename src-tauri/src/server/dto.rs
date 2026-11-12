//! 携帯端末に渡す読取専用データの境界。
//!
//! SQLite のモデルをそのまま JSON 化すると Windows の絶対パスを公開する。
//! 読み手の API は必ずここで公開専用の型へ変換する。
//!
//! 型そのものは `crates/piep-mobile-api` にある（携帯の Rust と画面の型が同じ定義を使う）。
//! ここには DB の行からの変換を置く。

use crate::database::{
    AssetEntry, DownloadEntry, DownloadPerson, DownloadSeries, EntityFacet, ReaderContentPage,
    ReaderMetadata, SearchV2Params, SearchV2Result, WorkCollection, WorkCollectionSummary,
};
use chrono::NaiveDate;
pub use piep_mobile_api::{
    MobileCollection, MobileCollectionCoverTile, MobileCollectionDetail, MobileCollectionMember,
    MobileEntity, MobileEntityDetail, MobileEntityPage, MobileEntitySeriesPage, MobileFacet,
    MobileMetadata, MobilePage, MobilePerson, MobileRevision, MobileSearchPage,
    MobileSeriesMembership, MobileVersion, MobileWork,
};
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::LazyLock;

/// Keep numbered-page jumps within the same work budget as the desktop UI.
const MAX_NUMBERED_OFFSET: i64 = 5_000;

impl From<DownloadEntry> for MobileWork {
    fn from(work: DownloadEntry) -> Self {
        Self {
            id: work.id,
            source: work.source,
            source_id: work.source_id,
            title: work.title,
            author_name: work.author_name,
            author_id: work.author_id,
            content_type: work.content_type,
            tags: work.tags,
            excerpt: work.excerpt,
            text_length: work.text_length,
            asset_count: work.asset_count,
            has_cover: work.cover_path.is_some(),
            current_version: work.current_version,
            content_hash: work.content_hash,
            favorite: work.favorite,
            series_title: work.series_title,
            series_id: work.series_id,
            source_created_at: work.source_created_at,
            downloaded_at: work.downloaded_at,
        }
    }
}

impl From<SearchV2Result> for MobileSearchPage {
    fn from(result: SearchV2Result) -> Self {
        Self {
            items: result.items.into_iter().map(MobileWork::from).collect(),
            next_cursor: result.next_cursor,
            total_estimate: result.total_estimate,
            index_complete: result.search_meta.index_complete,
        }
    }
}

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

impl From<EntityFacet> for MobileEntity {
    fn from(entity: EntityFacet) -> Self {
        Self {
            source: entity.source,
            source_key: entity.source_key,
            display_name: entity.display_name,
            count: entity.count,
            description: entity.description,
            sample_title: entity.sample_title,
            has_cover: entity.cover_path.is_some(),
            is_concluded: entity.is_concluded,
        }
    }
}

impl From<WorkCollectionSummary> for MobileCollection {
    fn from(collection: WorkCollectionSummary) -> Self {
        Self {
            id: collection.id,
            name: collection.name,
            description: collection.description,
            collection_kind: collection.collection_kind,
            member_count: collection.member_count,
            available_count: collection.available_count,
            cover_work_id: collection.cover_download_id,
            cover_mode: collection.cover_mode,
            cover_revision: collection.revision,
            has_single_cover: collection.cover_path.is_some(),
            has_file_cover: collection.cover_image_path.is_some(),
            cover_tiles: collection
                .cover_tiles
                .into_iter()
                .map(|tile| MobileCollectionCoverTile {
                    source: tile.source,
                    source_id: tile.source_id,
                    title: tile.title,
                    author_name: tile.author_name,
                    has_cover: tile.cover_path.is_some(),
                })
                .collect(),
            position: None,
        }
    }
}

impl From<WorkCollection> for MobileCollectionDetail {
    fn from(collection: WorkCollection) -> Self {
        Self {
            summary: MobileCollection::from(collection.summary),
            members: collection
                .members
                .into_iter()
                .map(|member| MobileCollectionMember {
                    title: member.title,
                    author_name: member.author_name,
                    position: member.position,
                    work: member.work.map(MobileWork::from),
                })
                .collect(),
        }
    }
}

impl From<DownloadPerson> for MobilePerson {
    fn from(person: DownloadPerson) -> Self {
        Self {
            source: person.person_source,
            key: person.person_key,
            role: person.role,
            display_name: person.display_name,
        }
    }
}

impl From<DownloadSeries> for MobileSeriesMembership {
    fn from(series: DownloadSeries) -> Self {
        Self {
            source: series.series_source,
            key: series.series_key,
            title: series.title,
            content_order: series.content_order,
        }
    }
}

impl From<ReaderMetadata> for MobileMetadata {
    fn from(metadata: ReaderMetadata) -> Self {
        let active_edit_id = metadata.active_edit_revision.as_ref().map(|edit| edit.id);
        let active_edit_hash = metadata
            .active_edit_revision
            .as_ref()
            .and_then(|edit| edit.content_hash.clone());
        Self {
            work: MobileWork::from(metadata.download),
            people: Vec::new(),
            series: Vec::new(),
            versions: metadata
                .versions
                .into_iter()
                .map(|version| MobileVersion {
                    version: version.version,
                    content_hash: version.content_hash,
                    text_length: version.text_length,
                    asset_count: version.asset_count,
                    created_at: version.created_at,
                })
                .collect(),
            is_edited: metadata.is_edited,
            active_edit_id,
            active_edit_hash,
        }
    }
}

static HTML_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<!--.*?-->|<[a-z][a-z0-9:-]*\b(?:[^"'<>]|"[^"]*"|'[^']*')*>"#)
        .expect("valid tag regex")
});

static HTML_ATTRIBUTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)(\s+)([^\s/="'<>`]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?"#)
        .expect("valid attribute regex")
});

static ATTACHMENT_ORIGINAL_BUTTON: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)\s*<button\b[^>]*\battachment-original\b[^>]*>.*?</button>"#)
        .expect("valid button regex")
});

/// PC 版が付けたファイルパスを、当該作品の画像 ID に置き換える。
/// 未知の参照はそのまま返さない。添付の「原本を見る」は携帯では開けないので外す。
/// 端末へ渡す本文を作る。`dimensions` は挿絵の縦横を返す。
///
/// 縦横が分かれば、画像が届く前から枠の高さが決まる。本文を先に出しても位置がずれない。
pub fn mobile_page(
    page: ReaderContentPage,
    assets: &[AssetEntry],
    revision: MobileRevision,
    dimensions: impl Fn(&AssetEntry) -> Option<(u32, u32)>,
) -> Result<MobilePage, String> {
    let asset_ids: HashMap<String, &AssetEntry> = assets
        .iter()
        .map(|asset| (escape_html_attribute(&asset.local_path), asset))
        .collect();
    let mut missing = false;
    let readable = ATTACHMENT_ORIGINAL_BUTTON.replace_all(&page.html, "");
    let html = HTML_TAG
        .replace_all(&readable, |captures: &regex::Captures<'_>| {
            let tag = &captures[0];
            if tag.starts_with("<!--") {
                return tag.to_string();
            }
            // 属性を値ごとに読む。本文の単語や、alt/title の値に書かれた
            // data-local-path は画像の参照として扱わない。
            HTML_ATTRIBUTE
                .replace_all(tag, |attribute: &regex::Captures<'_>| {
                    if !attribute[2].eq_ignore_ascii_case("data-local-path") {
                        return attribute[0].to_string();
                    }
                    let path = attribute
                        .get(3)
                        .or_else(|| attribute.get(4))
                        .or_else(|| attribute.get(5));
                    let Some(asset) = path.and_then(|path| asset_ids.get(path.as_str())) else {
                        missing = true;
                        return String::new();
                    };
                    let reference = match dimensions(asset) {
                        Some((width, height)) => format!(
                            r#"data-asset-id="{}" width="{width}" height="{height}""#,
                            asset.id
                        ),
                        None => format!(r#"data-asset-id="{}""#, asset.id),
                    };
                    format!("{}{reference}", &attribute[1])
                })
                .into_owned()
        })
        .into_owned();
    if missing {
        return Err("本文の画像を解決できません".into());
    }
    Ok(MobilePage {
        page: page.page,
        page_count: page.page_count,
        html,
        total_plain_text_chars: page.total_plain_text_chars,
        source_page_starts: page.source_page_starts,
        revision: Some(revision),
    })
}

fn escape_html_attribute(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_replaces_asset_path_and_rejects_unknown_reference() {
        let path = r#"C:\Users\reader\AppData\piep\cover&one.jpg"#;
        let assets = vec![AssetEntry {
            id: 42,
            download_id: 7,
            asset_type: "image".into(),
            filename: "cover.jpg".into(),
            local_path: path.into(),
            original_url: None,
            mime_type: Some("image/jpeg".into()),
            file_size_bytes: 100,
        }];
        let page = ReaderContentPage {
            page: 0,
            page_count: 1,
            html: format!(
                r#"<p>本文</p><img data-local-path="{}" alt="表紙">"#,
                escape_html_attribute(path)
            ),
            plain_text: "本文".into(),
            total_plain_text_chars: 2,
            source_page_starts: vec![0],
        };
        let revision = MobileRevision {
            current_version: 3,
            content_hash: Some("abc".into()),
            active_edit_hash: None,
        };
        let safe =
            mobile_page(page.clone(), &assets, revision.clone(), |_| None).expect("known asset");
        assert!(safe.html.contains("data-asset-id=\"42\""));
        assert!(!safe.html.contains("width="));
        assert!(!safe.html.contains("C:\\Users"));
        assert!(!safe.html.contains("data-local-path"));
        assert_eq!(safe.revision.as_ref(), Some(&revision));
        let sized = mobile_page(page.clone(), &assets, revision.clone(), |_| {
            Some((1200, 1697))
        })
        .expect("known asset");
        assert!(sized
            .html
            .contains(r#"data-asset-id="42" width="1200" height="1697""#));
        assert!(mobile_page(page, &[], revision, |_| None).is_err());
    }

    #[test]
    fn page_drops_the_attachment_original_button() {
        let page = ReaderContentPage {
            page: 0,
            page_count: 1,
            html: r#"<div class="attachment-notice">添付ファイル「a.pdf」から取り込んだ本文 <button type="button" class="attachment-original" data-local-path="C:\library\a.pdf" data-file-name="a.pdf">原本を見る</button></div><p>本文</p>"#.into(),
            plain_text: "本文".into(),
            total_plain_text_chars: 2,
            source_page_starts: vec![0],
        };
        let revision = MobileRevision {
            current_version: 1,
            content_hash: None,
            active_edit_hash: None,
        };
        let safe = mobile_page(page, &[], revision, |_| None).expect("button removed");
        assert_eq!(
            safe.html,
            r#"<div class="attachment-notice">添付ファイル「a.pdf」から取り込んだ本文</div><p>本文</p>"#
        );
    }

    #[test]
    fn local_path_words_in_text_and_attribute_values_are_normal_reader_content() {
        let html = r#"<p>data-local-path と DATA-LOCAL-PATH="説明"</p><p title='data-local-path="例"'>説明</p><code>&lt;img data-local-path="例"&gt;</code>"#;
        let page = ReaderContentPage {
            page: 0,
            page_count: 1,
            html: html.into(),
            plain_text: String::new(),
            total_plain_text_chars: 0,
            source_page_starts: vec![0],
        };
        let revision = MobileRevision {
            current_version: 1,
            content_hash: None,
            active_edit_hash: None,
        };
        assert_eq!(
            mobile_page(page.clone(), &[], revision.clone(), |_| None)
                .unwrap()
                .html,
            html
        );
        for attribute in [
            r#"data-local-path="C:\private.png""#,
            "DATA-LOCAL-PATH='C:/private.png'",
            "data-local-path=C:/private.png",
            "data-local-path",
        ] {
            let unsafe_page = ReaderContentPage {
                html: format!("<img {attribute}>"),
                ..page.clone()
            };
            assert!(
                mobile_page(unsafe_page, &[], revision.clone(), |_| None).is_err(),
                "{attribute}"
            );
        }
    }

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

    #[test]
    fn work_summary_does_not_serialize_private_paths() {
        let work = DownloadEntry {
            id: 7,
            source: "pixiv".into(),
            source_id: "123".into(),
            title: "作品".into(),
            author_name: "作者".into(),
            author_id: "456".into(),
            content_type: "novel".into(),
            tags: vec!["短編".into()],
            excerpt: None,
            cover_path: Some(r"C:\private\cover.jpg".into()),
            json_path: r"C:\private\work.json".into(),
            original_json_path: Some(r"C:\private\original.json".into()),
            asset_count: 1,
            file_size_bytes: 100,
            downloaded_at: "2026-10-10".into(),
            source_created_at: None,
            content_hash: None,
            text_length: 20,
            source_updated_at: None,
            watch_updates: false,
            current_version: 1,
            favorite: false,
            person_id: None,
            person_name: None,
            person_icon_path: Some(r"C:\private\avatar.jpg".into()),
            series_id: None,
            series_title: None,
            search_score: None,
            match_fields: vec![],
            score_reasons: vec![],
            match_highlights: vec![],
            sort_key: None,
            matched_version: None,
            historical_match_count: 0,
        };
        let json = serde_json::to_string(&MobileWork::from(work)).expect("summary JSON");
        assert!(json.contains("作品"));
        assert!(json.contains("hasCover"));
        assert!(!json.contains("C:\\private"));
        assert!(!json.contains("Path"));
    }
}
