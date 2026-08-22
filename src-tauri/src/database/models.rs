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
    pub tags: Vec<String>,
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
    /// Creator avatar, so cards can show a face next to the author name.
    pub person_icon_path: Option<String>,
    pub series_id: Option<String>,
    pub series_title: Option<String>,
    pub search_score: Option<f64>,
    pub match_fields: Vec<String>,
    pub score_reasons: Vec<ScoreReason>,
    pub match_highlights: Vec<SearchHighlight>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_key: Option<String>,
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
    /// 完結しているか。None は「取得元にまだ聞いていない」。
    pub is_concluded: Option<bool>,
    /// 取得元で公開されている話数。
    pub published_content_count: Option<i64>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreReason {
    pub field: String,
    pub match_type: String,
    pub term: String,
    pub contribution: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHighlight {
    pub field: String,
    pub text: String,
    pub segments: Vec<SearchHighlightSegment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_chunk_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHighlightSegment {
    pub text: String,
    pub matched: bool,
}

/// Local edit revision for a saved work.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkEditRevision {
    pub id: i64,
    pub download_id: i64,
    pub base_version: i64,
    pub status: String,
    pub title: Option<String>,
    pub content_hash: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A block in the local editor document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkBlock {
    pub id: i64,
    pub edit_revision_id: i64,
    pub order: i64,
    pub block_type: String,
    pub text: Option<String>,
    pub asset_id: Option<i64>,
    pub attrs_json: Option<String>,
}

/// Input shape for saving editor blocks from the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkBlockInput {
    pub block_type: String,
    pub text: Option<String>,
    pub asset_id: Option<i64>,
    pub attrs_json: Option<String>,
}

/// Read-optimized document payload for the dedicated reader.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaderDocument {
    pub download: DownloadEntry,
    pub assets: Vec<AssetEntry>,
    pub versions: Vec<DownloadVersion>,
    pub html: String,
    pub plain_text: String,
    pub is_edited: bool,
    pub active_edit_revision: Option<WorkEditRevision>,
}

/// Edit-optimized document payload for the block editor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorDocument {
    pub download: DownloadEntry,
    pub assets: Vec<AssetEntry>,
    pub active_revision: Option<WorkEditRevision>,
    pub draft_revision: Option<WorkEditRevision>,
    pub base_version: i64,
    pub blocks: Vec<WorkBlock>,
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

/// Small, eagerly loaded part of a reader document.  Assets and body content
/// intentionally live behind separate commands so opening a work does not
/// serialize a multi-megabyte payload before the first frame can render.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaderMetadata {
    pub download: DownloadEntry,
    pub versions: Vec<DownloadVersion>,
    pub asset_count: i64,
    pub is_edited: bool,
    pub active_edit_revision: Option<WorkEditRevision>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaderContentPage {
    pub page: usize,
    pub page_count: usize,
    pub html: String,
    pub plain_text: String,
    pub total_plain_text_chars: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReaderSearchHit {
    /// One-based transport/source page number for direct navigation.
    pub page: usize,
    pub snippet: String,
    pub count: usize,
}

/// Runtime measurements used by the large-library diagnostics screen.  The
/// benchmark values are measured on demand against the user's real database;
/// they are not synthetic estimates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFileIssue {
    /// Stable machine-readable value: missing, unsafe, unreadable, empty,
    /// size_mismatch, or transient.
    pub issue_type: String,
    /// work_json, work_asset, profile, entity_json, or transient.
    pub category: String,
    pub path: String,
    pub label: Option<String>,
    pub expected_size_bytes: Option<u64>,
    pub actual_size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryDiagnostics {
    pub measured_at: String,
    pub total_downloads: i64,
    pub total_assets: i64,
    pub total_versions: i64,
    pub total_text_length: i64,
    pub database_size_bytes: u64,
    pub wal_size_bytes: u64,
    pub storage_size_bytes: u64,
    pub lexical_index_size_bytes: u64,
    pub lexical_index_file_count: u64,
    pub lexical_index_segment_count: u64,
    pub semantic_index_size_bytes: u64,
    pub sqlite_page_count: i64,
    pub sqlite_free_pages: i64,
    pub sqlite_cache_size_bytes: u64,
    pub live_database_bytes: u64,
    pub fragmentation_percent: f64,
    pub orphan_asset_rows: i64,
    pub orphan_asset_bytes: i64,
    pub orphan_asset_files: u64,
    pub orphan_asset_file_bytes: u64,
    pub checked_file_references: u64,
    pub missing_json_files: u64,
    pub missing_asset_files: u64,
    pub missing_profile_files: u64,
    pub unsafe_referenced_files: u64,
    pub unreadable_referenced_files: u64,
    pub empty_referenced_files: u64,
    pub mismatched_asset_files: u64,
    pub transient_files: u64,
    pub transient_file_bytes: u64,
    /// A bounded sample for an actionable UI. Totals above remain the source
    /// of truth; very large result sets never cross IPC in full.
    pub file_issue_samples: Vec<LibraryFileIssue>,
    pub process_memory_bytes: Option<u64>,
    pub list_first_page_ms: f64,
    pub list_p50_ms: f64,
    pub list_p95_ms: f64,
    pub lexical_search_ms: Option<f64>,
    pub lexical_search_p50_ms: Option<f64>,
    pub lexical_search_p95_ms: Option<f64>,
    pub exact_author_p50_ms: Option<f64>,
    pub exact_author_p95_ms: Option<f64>,
    pub benchmark_query: Option<String>,
    pub search_index: SearchIndexStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryMaintenanceResult {
    pub compacted: bool,
    pub before_bytes: u64,
    pub after_bytes: u64,
    pub reclaimed_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchIndexOptimizationResult {
    pub optimized: bool,
    pub before_segments: u64,
    pub after_segments: u64,
    pub before_bytes: u64,
    pub after_bytes: u64,
    pub reclaimed_bytes: u64,
    pub elapsed_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardTrendPoint {
    pub bucket: String,
    pub count: i64,
    pub pixiv_count: i64,
    pub fanbox_count: i64,
    pub total_size_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceBreakdown {
    pub source: String,
    pub count: i64,
    pub total_size_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardSummary {
    pub stats: DbStats,
    pub favorite_count: i64,
    pub watched_count: i64,
    pub update_target_count: i64,
    pub indexed_count: i64,
    pub pending_index_count: i64,
    pub top_tags: Vec<FacetCount>,
    pub top_authors: Vec<FacetCount>,
    pub recent_downloads: Vec<DownloadEntry>,
    pub source_breakdown: Vec<SourceBreakdown>,
    pub monthly_downloads: Vec<DashboardTrendPoint>,
}

/// A named set of search conditions the reader keeps.
///
/// These used to live in browser storage, which made them per-install and kept
/// them out of reach of anything but the library toolbar. They are part of the
/// library, so they belong in it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedSearch {
    pub id: i64,
    pub name: String,
    pub query: Option<String>,
    pub params_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedSearchInput {
    /// Absent for a new entry. Saving again under an existing name updates that
    /// entry rather than failing on the unique constraint.
    pub id: Option<i64>,
    pub name: String,
    pub query: Option<String>,
    pub params_json: String,
}

/// 利用者が作品を横断してまとめる永続コレクション。
///
/// 公式シリーズや保存検索とは独立しており、同じ作品は複数のコレクションへ
/// 所属できる。`collection_kind` は `ordered` または `unordered`。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkCollectionSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub collection_kind: String,
    pub cover_download_id: Option<i64>,
    pub cover_path: Option<String>,
    pub revision: i64,
    pub member_count: i64,
    pub available_count: i64,
    pub total_text_length: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkCollectionInput {
    /// 更新時だけ指定。新規作成では安定 ID をバックエンドが生成する。
    pub id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub collection_kind: String,
    pub cover_download_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkCollectionMember {
    pub collection_id: String,
    pub source: String,
    pub source_id: String,
    pub download_id: Option<i64>,
    pub title: String,
    pub author_name: String,
    pub cover_path: Option<String>,
    pub text_length: i64,
    pub position: i64,
    pub member_role: String,
    pub added_by: String,
    pub pinned: bool,
    pub note: Option<String>,
    pub missing: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkCollectionMemberInput {
    pub source: String,
    pub source_id: String,
    /// バックアップ等で現在未保存の作品名を保つための任意スナップショット。
    pub title_snapshot: Option<String>,
    pub author_snapshot: Option<String>,
    pub position: Option<i64>,
    pub member_role: Option<String>,
    pub added_by: Option<String>,
    pub pinned: Option<bool>,
    pub note: Option<String>,
}

/// 取得元をまたいでも衝突しない、保存状態に依存しない作品識別子。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct WorkKey {
    pub source: String,
    pub source_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkCollection {
    #[serde(flatten)]
    pub summary: WorkCollectionSummary,
    pub members: Vec<WorkCollectionMember>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkLink {
    pub id: i64,
    pub from_source: String,
    pub from_source_id: String,
    pub from_download_id: Option<i64>,
    pub to_source: String,
    pub to_source_id: String,
    pub to_download_id: Option<i64>,
    pub relation_type: String,
    pub evidence_type: String,
    pub anchor_text: Option<String>,
    pub context_text: Option<String>,
    pub confidence: f64,
    pub status: String,
    pub discovered_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionSuggestionEvidence {
    pub kind: String,
    pub label: String,
    pub contribution: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionSuggestionMember {
    pub source: String,
    pub source_id: String,
    pub download_id: Option<i64>,
    pub title: String,
    pub author_name: String,
    pub cover_path: Option<String>,
    pub text_length: i64,
    pub proposed_position: i64,
    pub score: f64,
    pub selected: bool,
    pub evidence: Vec<CollectionSuggestionEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionSuggestion {
    pub id: String,
    pub proposed_name: String,
    pub collection_kind: String,
    pub score: f64,
    pub rule_version: String,
    pub state: String,
    pub members: Vec<CollectionSuggestionMember>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionSuggestionRequest {
    pub seed_download_ids: Vec<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptCollectionSuggestionInput {
    pub suggestion_id: String,
    pub name: Option<String>,
    pub collection_kind: Option<String>,
    pub member_keys: Option<Vec<WorkKey>>,
}

/// The counts the library sidebar shows next to each shelf.
///
/// Kept separate from the dashboard summary, which also computes tag, author
/// and trend data: the sidebar is on screen the whole time and must not pay for
/// any of that.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryShelfCounts {
    pub total: i64,
    pub favorite: i64,
    pub watched: i64,
    /// How many of the supplied works still exist. Reading positions are kept
    /// per device and outlive the works they point at.
    pub reading: i64,
}

/// フィルター候補（名前と件数）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FacetCount {
    pub name: String,
    pub count: i64,
}

/// 作者・シリーズカードで使うエンティティ候補
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityFacet {
    pub source: String,
    pub source_key: String,
    pub display_name: String,
    pub count: i64,
    pub cover_path: Option<String>,
    pub description: Option<String>,
    pub updated_at: Option<String>,
    pub latest_downloaded_at: Option<String>,
    /// 配下の作品のうち、取得元でいちばん新しいものの時刻。並べ替え用。
    pub latest_source_updated_at: Option<String>,
    pub sample_title: Option<String>,
    pub icon_path: Option<String>,
    pub banner_path: Option<String>,
    /// シリーズだけが持つ。作者は常に None。
    pub is_concluded: Option<bool>,
}

/// A stable keyset page of series connected to one person/author.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntitySeriesPage {
    pub items: Vec<EntityFacet>,
    pub next_cursor: Option<String>,
    /// Exact number of series matching the person and optional query.
    pub total: i64,
}

/// ライブラリの絞り込みUIで使う候補一覧
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterFacets {
    pub tags: Vec<FacetCount>,
    pub authors: Vec<FacetCount>,
    pub author_entities: Vec<EntityFacet>,
    pub series: Vec<EntityFacet>,
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
    /// 最後に「新しいもの」を見つけた時刻。ずっと空なら休眠している対象。
    #[serde(default)]
    pub last_hit_at: Option<String>,
    /// 連続で失敗している回数。0 に戻るのは成功したとき。
    #[serde(default)]
    pub consecutive_errors: i64,
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

/// まだ取り込んでいない改稿。
///
/// 取得元が直した日そのものは持たない。**手元が持っているのは「更新確認が
/// いつそれを見つけたか」だけ**で、作品行に入っている `source_updated_at` は
/// 取り直すまで古い版のままである（取り直して初めて追いついたと言えるため）。
/// 持っていない日付を、それらしく見せない。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRevision {
    /// `{source}:{sourceId}`。画面が手元の作品と突き合わせるための鍵。
    pub key: String,
    /// 更新確認がこの改稿を見つけた時刻。
    pub found_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCredentials {
    pub pixiv_refresh_token: Option<String>,
    /// pixiv の web セッション。無くても更新確認は動く（従来の経路になるだけ）。
    ///
    /// 既定を持たせてあるのは、この二つを知らない依頼が届いても
    /// 更新確認そのものは動くべきだから。**新しい鍵が無いことは、故障ではない。**
    #[serde(default)]
    pub pixiv_cookie: Option<String>,
    /// その Cookie を受け取ったときの UA。`pixiv_cookie` と対でだけ意味を持つ。
    #[serde(default)]
    pub pixiv_user_agent: Option<String>,
    pub fanbox_cookie: Option<String>,
    pub fanbox_user_agent: Option<String>,
}

/// 更新ジョブを始めるときに画面から届く依頼。
///
/// 受け取るだけの型で、DBには入らない。走り出したあとも要る `watch_saved` は
/// `update_jobs` の列へ写す。依頼をまるごと漬けていたころは、使われなくなった
/// 項目が永続データとして残り続けた。`Serialize` を持たせていないのは、
/// うっかりまた保存しないため。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartUpdateJobRequest {
    pub scope: String,
    pub mode: String,
    pub work_ids: Option<Vec<i64>>,
    pub target_ids: Option<Vec<i64>>,
    pub credentials: Option<UpdateCredentials>,
    /// このジョブが保存した作品を、そのまま更新監視に載せるか。
    /// 設定の「保存した作品を自動で監視する」から渡ってくる。
    #[serde(default)]
    pub watch_saved: Option<bool>,
    /// 監視対象に登録せず、この一回だけ確認する作者・シリーズ。
    ///
    /// 作品ページや作者ページの「新作を確認」から渡ってくる。登録済みの対象と
    /// 同じものを指したときは、その対象の前回位置を引き継いで無駄に遡らない。
    #[serde(default)]
    pub adhoc_targets: Option<Vec<AdhocUpdateTarget>>,
}

/// 一回きりの確認先。登録された監視対象とは別物。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdhocUpdateTarget {
    pub target_type: String,
    pub source: String,
    pub source_key: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateJobSummary {
    pub job_id: String,
    pub status: String,
    pub scope: String,
    pub mode: String,
    pub totals: i64,
    pub processed: i64,
    pub candidate_count: i64,
    pub saved_count: i64,
    pub error_count: i64,
    pub active_label: Option<String>,
    pub started_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateJobLog {
    pub id: i64,
    pub log_type: String,
    pub message: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateJobCandidate {
    pub id: i64,
    pub key: String,
    pub source: String,
    pub source_id: String,
    pub title: String,
    pub subtitle: String,
    pub target_label: String,
    pub target_type: String,
    pub selected: bool,
    pub status: String,
    /// "new" | "sequel" | "revision"。画面で分類して選べるようにするための印。
    #[serde(default)]
    pub kind: String,
    /// 失敗したときの理由。分類の札（`[取得制限]` など）を含む。
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateJobSnapshot {
    pub job_id: String,
    pub status: String,
    pub scope: String,
    pub mode: String,
    pub totals: i64,
    pub processed: i64,
    pub candidate_count: i64,
    pub saved_count: i64,
    pub error_count: i64,
    pub active_label: Option<String>,
    pub logs: Vec<UpdateJobLog>,
    pub candidates: Vec<UpdateJobCandidate>,
    pub next_candidate_cursor: Option<i64>,
    pub previous_log_cursor: Option<i64>,
    pub started_at: String,
    pub updated_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UpdateJobItem {
    pub id: i64,
    pub job_id: String,
    pub item_type: String,
    pub source: Option<String>,
    pub source_id: Option<String>,
    pub target_type: Option<String>,
    pub title: String,
    pub payload_json: String,
    pub status: String,
    pub error: Option<String>,
    pub result_download_id: Option<i64>,
}

/// 見つけたが、まだ保存も拒否もしていない作品。ジョブより長生きする。
#[derive(Debug, Clone)]
pub struct UpdateCandidateInput {
    pub source: String,
    pub source_id: String,
    pub kind: String,
    pub title: String,
    pub payload_json: String,
    pub target_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCandidateRow {
    pub source: String,
    pub source_id: String,
    pub kind: String,
    pub title: String,
    pub payload_json: String,
    pub target_type: Option<String>,
    pub status: String,
    pub first_seen_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct UpdateJobItemInput {
    pub item_type: String,
    pub source: Option<String>,
    pub source_id: Option<String>,
    pub target_type: Option<String>,
    pub title: String,
    pub payload_json: String,
    pub status: String,
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

/// 作者・シリーズの一覧そのものにかける条件。
///
/// 配下の作品にかける [`SearchV2Params`] とは層が違う。追いかけているか、
/// 何作品以上あるか、完結しているか - どれも束ね自身の性質で、作品の側には
/// 無い。混ぜると「監視中」が作品の話か作者の話か読めなくなる。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityFacetScope {
    /// "watched" | "paused" | "unwatched"。それ以外は無視する。
    pub watch: Option<String>,
    /// これ以上の作品を持つものだけ。1以下は条件なしと同じ。
    pub min_work_count: Option<i64>,
    /// 完結しているか。シリーズだけに効く。
    pub concluded: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchV2Params {
    pub text: Option<String>,
    pub query: Option<String>,
    pub source: Option<String>,
    pub content_type: Option<String>,
    pub sort_by: Option<String>,
    pub sort_order: Option<String>,
    pub limit: Option<i64>,
    pub cursor: Option<String>,
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
    /// Jumps straight to a page rather than walking there with a cursor.
    ///
    /// Only meaningful for an explicit column ordering: relevance paging walks
    /// the index with a score cursor, which has no notion of an nth page.
    pub offset: Option<i64>,
    /// Restricts the result to these works. Backs shelves whose membership is
    /// known to the client rather than the database - reading positions are
    /// kept per device, so only the client knows which works are part-read.
    pub ids_include: Option<Vec<i64>>,
    pub view_mode: Option<String>,
    pub projection: Option<String>,
    pub search_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchMeta {
    pub engine: String,
    pub query: Option<String>,
    pub total_estimate: Option<i64>,
    pub index_complete: bool,
    /// Human-readable, stable facts the UI can use to explain how the query
    /// was interpreted. These are intentionally not tied to Tantivy internals.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub explanations: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exact_entity: Option<SearchEntityIntent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_index_complete: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_model_ready: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchEntityIntent {
    pub kind: String,
    pub label: String,
    pub source: Option<String>,
    pub source_key: Option<String>,
    pub strict: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchV2Result {
    pub items: Vec<DownloadEntry>,
    pub next_cursor: Option<String>,
    pub total_estimate: Option<i64>,
    pub search_meta: SearchMeta,
    pub facets_version: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSuggestParams {
    pub text: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSuggestion {
    pub kind: String,
    pub label: String,
    pub value: String,
    pub count: Option<i64>,
    #[serde(default)]
    pub exact_match: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSuggestResult {
    pub items: Vec<SearchSuggestion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkMutationResult {
    pub matched_count: i64,
    pub changed_count: i64,
}

/// Smart Search インデックスの構築状況
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchIndexStatus {
    pub total_downloads: i64,
    pub indexed_downloads: i64,
    pub pending_downloads: i64,
    pub is_complete: bool,
    pub phase: String,
    pub indexed_chunks: i64,
    pub semantic_indexed_chunks: i64,
    pub semantic_indexed_downloads: i64,
    pub semantic_pending_downloads: i64,
    pub semantic_model_ready: bool,
    pub embedding_provider: String,
    pub gpu_enabled: bool,
    pub throughput_per_sec: Option<f64>,
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
    pub tags: Vec<String>,
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

#[cfg(test)]
mod start_request_shape_tests {
    use super::StartUpdateJobRequest;

    /// 画面が送らない項目は、JSON からそのまま消える（JS の undefined）。
    /// 省略が欠損として扱われると、確認そのものが始まらなくなる。
    #[test]
    fn omitted_optional_fields_deserialize_as_none() {
        let request: StartUpdateJobRequest =
            serde_json::from_str(r#"{"scope":"all","mode":"check_only"}"#).unwrap();
        assert_eq!(request.scope, "all");
        assert!(request.work_ids.is_none());
        assert!(request.target_ids.is_none());
        assert!(request.credentials.is_none());
        assert!(request.watch_saved.is_none());
        assert!(request.adhoc_targets.is_none());
    }
}
