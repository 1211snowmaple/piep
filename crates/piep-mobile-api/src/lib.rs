//! 携帯端末とサーバーの間を流れる読取専用データの型。
//!
//! サーバー（`src-tauri/src/server`）がこの形で送り、携帯の Rust（`android/src-tauri`）が
//! 同じ型で受けて、そのまま画面へ渡す。画面側の TypeScript の型
//! （`shared/mobileApi.ts`）は、このファイルから `tools/api-types` が生成する。
//! 型を足すとき・項目を変えるときは、ここを直して生成し直す。
//!
//! ここには型だけを置く。DB の行からの変換はサーバー側の `dto.rs` にある。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileWork {
    pub id: i64,
    pub source: String,
    pub source_id: String,
    pub title: String,
    pub author_name: String,
    #[serde(default)]
    pub author_id: String,
    pub content_type: String,
    pub tags: Vec<String>,
    pub excerpt: Option<String>,
    pub text_length: i64,
    pub asset_count: i64,
    pub has_cover: bool,
    pub current_version: i64,
    pub content_hash: Option<String>,
    pub favorite: bool,
    pub series_title: Option<String>,
    pub series_id: Option<String>,
    #[serde(default)]
    pub source_created_at: Option<String>,
    #[serde(default)]
    pub downloaded_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileSearchPage {
    pub items: Vec<MobileWork>,
    pub next_cursor: Option<String>,
    pub total_estimate: Option<i64>,
    pub index_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileEntity {
    pub source: String,
    pub source_key: String,
    pub display_name: String,
    pub count: i64,
    pub description: Option<String>,
    pub sample_title: Option<String>,
    pub has_cover: bool,
    pub is_concluded: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileEntityPage {
    pub items: Vec<MobileEntity>,
    pub next_offset: Option<i64>,
    pub total: i64,
}

/// 作者の中のシリーズ一覧。続きは不透明なカーソルで辿る。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileEntitySeriesPage {
    pub items: Vec<MobileEntity>,
    pub next_cursor: Option<String>,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileFacet {
    pub name: String,
    pub count: i64,
}

/// 作者・シリーズの画面を一度で描くための情報。保存先のパスは含めない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileEntityDetail {
    pub source: String,
    pub source_key: String,
    pub display_name: String,
    pub description: Option<String>,
    pub work_count: i64,
    pub has_cover: bool,
    pub has_banner: bool,
    pub is_concluded: Option<bool>,
    pub published_count: Option<i64>,
    pub links: Vec<String>,
    pub tags: Vec<MobileFacet>,
    pub series_count: Option<i64>,
    pub collections: Vec<MobileCollection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileCollection {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub collection_kind: String,
    pub member_count: i64,
    pub available_count: i64,
    pub cover_work_id: Option<i64>,
    #[serde(default)]
    pub cover_mode: String,
    #[serde(default)]
    pub cover_revision: i64,
    #[serde(default)]
    pub has_single_cover: bool,
    #[serde(default)]
    pub has_file_cover: bool,
    #[serde(default)]
    pub cover_tiles: Vec<MobileCollectionCoverTile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileCollectionCoverTile {
    pub source: String,
    pub source_id: String,
    pub title: String,
    pub author_name: String,
    pub has_cover: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileCollectionMember {
    pub title: String,
    pub author_name: String,
    pub position: i64,
    pub work: Option<MobileWork>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileCollectionDetail {
    #[serde(flatten)]
    pub summary: MobileCollection,
    pub members: Vec<MobileCollectionMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileVersion {
    pub version: i64,
    pub content_hash: Option<String>,
    pub text_length: i64,
    pub asset_count: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobilePerson {
    pub source: String,
    pub key: String,
    pub role: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileSeriesMembership {
    pub source: String,
    pub key: String,
    pub title: String,
    pub content_order: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileMetadata {
    pub work: MobileWork,
    #[serde(default)]
    pub people: Vec<MobilePerson>,
    #[serde(default)]
    pub series: Vec<MobileSeriesMembership>,
    pub versions: Vec<MobileVersion>,
    pub is_edited: bool,
    pub active_edit_id: Option<i64>,
    pub active_edit_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobilePage {
    pub page: usize,
    pub page_count: usize,
    pub html: String,
    pub total_plain_text_chars: usize,
    pub source_page_starts: Vec<usize>,
    /// この本文がどの版のものか。端末が本文の前後でメタデータを取り直さずに済む。
    /// サーバーは必ず入れる。端末に残っている古い保存分には無い。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<MobileRevision>,
}

/// 本文の版を見分ける三つ組み。`MobileMetadata` の同じ値と一致する。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileRevision {
    pub current_version: i64,
    pub content_hash: Option<String>,
    pub active_edit_hash: Option<String>,
}

impl From<&MobileMetadata> for MobileRevision {
    fn from(metadata: &MobileMetadata) -> Self {
        Self {
            current_version: metadata.work.current_version,
            content_hash: metadata.work.content_hash.clone(),
            active_edit_hash: metadata.active_edit_hash.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_saved_before_revisions_still_reads_and_writes_back_without_the_key() {
        let old = r#"{"page":0,"pageCount":1,"html":"<p>本文</p>","totalPlainTextChars":2,"sourcePageStarts":[0]}"#;
        let page: MobilePage = serde_json::from_str(old).unwrap();
        assert_eq!(page.revision, None);
        assert_eq!(serde_json::to_string(&page).unwrap(), old);
    }

    #[test]
    fn a_collection_detail_is_one_flat_object_on_the_wire() {
        let json = r#"{"id":"c1","name":"棚","description":null,"collectionKind":"manual","memberCount":1,"availableCount":0,"coverWorkId":null,"members":[{"title":"題","authorName":"作者","position":0,"work":null}]}"#;
        let detail: MobileCollectionDetail = serde_json::from_str(json).unwrap();
        assert_eq!(detail.summary.cover_mode, "");
        assert_eq!(detail.members.len(), 1);
        let written = serde_json::to_value(&detail).unwrap();
        assert_eq!(written["name"], "棚");
        assert!(written.get("position").is_none());
        assert!(written.get("summary").is_none());
    }
}
