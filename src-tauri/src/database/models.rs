//! データベース用の構造体定義。

use serde::{Deserialize, Serialize};

/// ダウンロードエントリ（DB行に対応）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadEntry {
    pub id: i64,
    pub source: String,
    pub source_id: String,
    pub title: String,
    pub author_name: String,
    pub author_id: String,
    pub content_type: String,
    pub tags: Option<String>,
    pub excerpt: Option<String>,
    pub cover_path: Option<String>,
    pub json_path: String,
    pub original_json_path: Option<String>,
    pub asset_count: i64,
    pub file_size_bytes: i64,
    pub downloaded_at: String,
    pub source_created_at: Option<String>,
    pub content_hash: Option<String>,
    pub text_length: i64,
    pub source_updated_at: Option<String>,
    pub watch_updates: bool,
    pub current_version: i64,
    pub favorite: bool,
    pub person_id: Option<String>,
    pub person_name: Option<String>,
    pub series_id: Option<String>,
    pub series_title: Option<String>,
    pub search_score: Option<f64>,
    pub match_snippet: Option<String>,
    pub match_fields: Vec<String>,
}

/// 作者/クリエイター統合エンティティ
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonEntry {
    pub id: i64,
    pub source: String,
    pub source_key: String,
    pub display_name: String,
    pub icon_path: Option<String>,
    pub cover_path: Option<String>,
    pub description: Option<String>,
    pub links_json: Option<String>,
    pub content_hash: Option<String>,
    pub current_version: i64,
    pub last_checked_at: Option<String>,
    pub last_fetched_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub work_count: Option<i64>,
}

/// シリーズエンティティ
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeriesEntry {
    pub id: i64,
    pub source: String,
    pub source_key: String,
    pub title: String,
    pub description: Option<String>,
    pub cover_path: Option<String>,
    pub content_hash: Option<String>,
    pub current_version: i64,
    pub last_checked_at: Option<String>,
    pub last_fetched_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub work_count: Option<i64>,
}

/// 人物/シリーズの履歴
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityVersion {
    pub id: i64,
    pub entity_type: String,
    pub source: String,
    pub source_key: String,
    pub version: i64,
    pub content_hash: Option<String>,
    pub json_path: String,
    pub asset_count: i64,
    pub file_size_bytes: i64,
    pub created_at: String,
    pub change_summary: Option<String>,
}

/// アセットエントリ（DB行に対応）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetEntry {
    pub id: i64,
    pub download_id: i64,
    pub asset_type: String,
    pub filename: String,
    pub local_path: String,
    pub original_url: Option<String>,
    pub mime_type: Option<String>,
    pub file_size_bytes: i64,
}

/// データベースの統計情報
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbStats {
    pub total_downloads: i64,
    pub pixiv_count: i64,
    pub fanbox_count: i64,
    pub total_assets: i64,
    pub total_size_bytes: i64,
}

/// フィルター候補（名前と件数）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FacetCount {
    pub name: String,
    pub count: i64,
}

/// ライブラリの絞り込みUIで使う候補一覧
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterFacets {
    pub tags: Vec<FacetCount>,
    pub authors: Vec<FacetCount>,
    pub content_types: Vec<FacetCount>,
    pub asset_types: Vec<FacetCount>,
}

/// 更新チェック対象（作品・著者・シリーズ）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTarget {
    pub id: i64,
    pub target_type: String,
    pub source: String,
    pub source_key: String,
    pub display_name: String,
    pub enabled: bool,
    pub last_checked_at: Option<String>,
    pub last_seen_source_id: Option<String>,
    pub last_seen_source_updated_at: Option<String>,
    pub metadata_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// 更新チェック対象の作成・更新入力
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateTargetInput {
    pub target_type: String,
    pub source: String,
    pub source_key: String,
    pub display_name: String,
    pub enabled: bool,
    pub metadata_json: Option<String>,
}

/// 保存作品と著者・シリーズの関係
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRelation {
    pub download_id: i64,
    pub relation_type: String,
    pub source: String,
    pub relation_id: String,
    pub relation_name: String,
    pub work_count: Option<i64>,
}

/// 保存作品と人物の関係
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadPerson {
    pub download_id: i64,
    pub person_source: String,
    pub person_key: String,
    pub role: String,
    pub display_name: String,
}

/// 保存作品とシリーズの関係
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSeries {
    pub download_id: i64,
    pub series_source: String,
    pub series_key: String,
    pub title: String,
    pub content_order: Option<i64>,
}

/// 検索パラメータ
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchParams {
    pub query: Option<String>,
    pub source: Option<String>,
    pub content_type: Option<String>,
    pub sort_by: Option<String>,
    pub sort_order: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub favorite: Option<bool>,
    pub tags_include: Option<Vec<String>>,
    pub tags_exclude: Option<Vec<String>>,
    pub tag_filter_mode: Option<String>,
    pub authors_include: Option<Vec<String>>,
    pub authors_exclude: Option<Vec<String>>,
    pub min_char_count: Option<i64>,
    pub max_char_count: Option<i64>,
    pub asset_filter: Option<String>,
    pub watch_filter: Option<String>,
    pub person_source: Option<String>,
    pub person_key: Option<String>,
    pub series_source: Option<String>,
    pub series_key: Option<String>,
    pub search_mode: Option<String>,
}

/// Smart Search インデックスの構築状況
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchIndexStatus {
    pub total_downloads: i64,
    pub indexed_downloads: i64,
    pub pending_downloads: i64,
    pub is_complete: bool,
}

/// インポート情報（新規ダウンロード挿入用）
#[derive(Debug, Clone)]
pub struct NewDownload {
    pub source: String,
    pub source_id: String,
    pub title: String,
    pub author_name: String,
    pub author_id: String,
    pub content_type: String,
    pub tags: Option<String>,
    pub excerpt: Option<String>,
    pub cover_path: Option<String>,
    pub json_path: String,
    pub original_json_path: Option<String>,
    pub asset_count: i64,
    pub file_size_bytes: i64,
    pub downloaded_at: String,
    pub source_created_at: Option<String>,
    pub content_hash: Option<String>,
    pub text_length: i64,
    pub source_updated_at: Option<String>,
    pub watch_updates: bool,
    pub current_version: i64,
    pub favorite: bool,
}

/// 新規アセット挿入用
#[derive(Debug, Clone)]
pub struct NewAsset {
    pub download_id: i64,
    pub asset_type: String,
    pub filename: String,
    pub local_path: String,
    pub original_url: Option<String>,
    pub mime_type: Option<String>,
    pub file_size_bytes: i64,
}

/// バージョン履歴エントリ（DB行に対応）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadVersion {
    pub id: i64,
    pub download_id: i64,
    pub version: i64,
    pub content_hash: Option<String>,
    pub text_length: i64,
    pub json_path: String,
    pub original_json_path: Option<String>,
    pub asset_count: i64,
    pub file_size_bytes: i64,
    pub created_at: String,
    pub change_summary: Option<String>,
}

/// 新規バージョン履歴挿入用
#[derive(Debug, Clone)]
pub struct NewVersion {
    pub download_id: i64,
    pub version: i64,
    pub content_hash: Option<String>,
    pub text_length: i64,
    pub json_path: String,
    pub original_json_path: Option<String>,
    pub asset_count: i64,
    pub file_size_bytes: i64,
    pub created_at: String,
    pub change_summary: Option<String>,
}
