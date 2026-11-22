//! データベース用の構造体定義。

use serde::{Deserialize, Serialize};

pub use piep_reader_render::pages::{ReaderOutlineEntry, ReaderSearchHit};
pub use piep_reader_render::{AssetEntry, WorkBlock};

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
    /// Saved revision that matched when the current revision did not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_version: Option<i64>,
    /// Older saved revisions that also matched this query.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub historical_match_count: i64,
}

/// 保存済み作品から参照されているが、取得元での完全確認が終わっていない
/// 作者・シリーズ。`last_fetched_at` が埋まるまで何度でも検出できるため、
/// アプリ終了や通信失敗を跨いだ修復の再開位置にもなる。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncompleteEntityProfile {
    pub entity_type: String,
    pub source: String,
    pub source_key: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityProfileRepairStatus {
    pub person_count: i64,
    pub series_count: i64,
    pub total_count: i64,
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

/// Input shape for saving editor blocks from the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkBlockInput {
    pub block_type: String,
    pub text: Option<String>,
    pub asset_id: Option<i64>,
    pub attrs_json: Option<String>,
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

/// 反映済みの編集を、取得元と同じ書式へ組み直したもの。
///
/// 書き出しは保存した JSON を組み替えて本にする。編集版をそこへ差し込むには、
/// 取得元が書いていたのと同じ形 ―― pixiv なら本文記法、FANBOX ならブロックの
/// 並び ―― で渡す必要がある。平文だけを渡していたころは、挿絵も改ページも
/// 見出しも、その一手前で落ちていた。
#[derive(Debug, Clone)]
pub struct EditedSourceForm {
    pub pixiv_text: String,
    pub fanbox_blocks: Vec<serde_json::Value>,
    /// 取得元が分からないときの取りこぼし用。
    pub plain_text: String,
    /// 編集画面で足した絵。版の置き場の外（`editor-assets/`）にあるので、
    /// 本文が指していても書き出す側からは見えない。書き出す側が本へ加える。
    pub added_images: Vec<AssetEntry>,
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

/// 取得元をまたいでも衝突しない、保存状態に依存しない作品識別子。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct WorkKey {
    pub source: String,
    pub source_id: String,
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
    /// 提案の計算の作業用の表での作品の番号。画面へは出さず、棚の作品（`work`）へ置き換える。
    #[serde(skip)]
    pub corpus_key: Option<i64>,
    /// 棚の作品 ID（UUID）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<String>,
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
    /// 名前の案。一つに決めず、利用者に選ばせる。先頭が `proposed_name`。
    pub name_options: Vec<CollectionNameCandidate>,
    pub collection_kind: String,
    /// `sequence`（読む順のある続き物）か `theme`（味が同じ）か。
    pub track: String,
    /// `seed`（1作から広げた）か `sweep`（棚全体の走査）か。
    pub origin: String,
    /// 「なぜこれが束なのか」の一行。確度%の代わりに画面へ出す。
    pub evidence_summary: String,
    pub score: f64,
    pub rule_version: String,
    pub state: String,
    pub members: Vec<CollectionSuggestionMember>,
    pub created_at: String,
    pub updated_at: String,
}

/// 棚の走査の結果。
///
/// 束にならなかったものも返す。「催眠」759作は**まとまりではなく絞り込みの
/// 結果**なので、束としては出さない。ただし利用者にとっては確かに一つの
/// 見方なので、保存した検索として提案する。黙って捨てない。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionSweepResult {
    pub bundles: Vec<CollectionSuggestion>,
    pub saved_search_suggestions: Vec<SavedSearchSuggestion>,
    /// 題材の束を探すところまで届いたか。
    ///
    /// 意味索引が読めないと、走査は続き物しか出せない。それを黙って
    /// 「テーマの束は見つかりませんでした」と見せると、**壊れていることと
    /// 何も無いことが見分けられない**。
    pub semantic_used: bool,
    /// 届かなかったときの理由。使えたなら `None`。
    pub note: Option<String>,
}

/// すでにあるコレクションに、あとから入れるとよさそうな一作。
///
/// 束は作った時点で閉じない。新作は毎日届くし、あとから保存した旧作もある。
/// 「作ったときの顔ぶれ」に縛られる理由が無いのは、名前と同じである。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionAdditionCandidate {
    pub source: String,
    pub source_id: String,
    /// 提案の計算の作業用の表での作品の番号。画面へは出さず、棚の作品（`work`）へ置き換える。
    #[serde(skip)]
    pub corpus_key: i64,
    /// 棚の作品 ID（UUID）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<String>,
    pub title: String,
    pub author_name: String,
    pub cover_path: Option<String>,
    pub text_length: i64,
    pub published_at: String,
    /// 0.0〜1.0。走査の束と同じ尺度で測る。
    pub confidence: f64,
    /// なぜこの一作なのかを一行で。
    pub reason: String,
    pub evidence: Vec<CollectionSuggestionEvidence>,
}

/// 追加候補を探した結果。
///
/// 候補が空であることと、探せなかったことを混ぜない。意味索引が無い棚では
/// 規則だけで探すが、そのことは黙って隠さず `semantic_used` で返す。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionAdditionResult {
    pub collection_id: String,
    pub collection_name: String,
    pub candidates: Vec<CollectionAdditionCandidate>,
    /// 本文ベクトルまで見て測れたか。
    pub semantic_used: bool,
    /// 測れなかったときの理由。使えたなら `None`。
    pub note: Option<String>,
    /// 下限を越えた候補の総数。出したのはこのうち上位だけ。
    pub eligible_count: i64,
}

/// 束にするには大きすぎるタグ。保存した検索としてなら意味がある。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedSearchSuggestion {
    pub tag: String,
    pub work_count: i64,
    /// なぜ束にしなかったのかの一行。
    pub reason: String,
}

/// 束の名前の案。どこから来た案かが分かるようにしておく。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionNameCandidate {
    /// `title` / `series` / `tags` / `author` / `llm`
    pub source: String,
    pub name: String,
    /// 画面に出す短い説明。「題名の共通部分」「共有タグ」など。
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionSuggestionRequest {
    #[serde(default)]
    pub seed_download_ids: Vec<i64>,
    /// 棚の種の作品（UUID）。
    #[serde(default)]
    pub seed_works: Vec<String>,
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
    /// 取り込んでいない改稿がある作品の数。
    pub revised: i64,
}

/// フィルター候補（名前と件数）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FacetCount {
    pub name: String,
    pub count: i64,
    /// 保存元ごとに同名があり得る作者候補で使う。タグなどでは空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
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
    /// 棚の監視対象の ID。登録していない対象を一度だけ確かめるときは空。
    pub id: Option<crate::library::LibraryId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_generation: Option<i64>,
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
    /// 入っている監視グループの名前。バックアップはこれで所属を持ち運ぶ
    /// （グループの ID はライブラリごとに違うので、名前で結び直す）。
    #[serde(default)]
    pub watch_groups: Vec<String>,
    /// 一覧に出す顔。作者はプロフィール画像、シリーズは表紙。保存していなければ空。
    #[serde(default)]
    pub icon_path: Option<String>,
}

/// 利用者が作った監視グループ。監視対象を束ねて、まとめて確認する。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchGroup {
    pub id: crate::library::LibraryId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_generation: Option<i64>,
    pub name: String,
    pub sort_order: i64,
    pub created_at: String,
    pub updated_at: String,
    /// 中の監視対象（`update_targets.id`）。
    pub target_ids: Vec<crate::library::LibraryId>,
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
    pub work_ids: Option<Vec<crate::library::LibraryId>>,
    pub target_ids: Option<Vec<crate::library::LibraryId>>,
    pub credentials: Option<UpdateCredentials>,
    /// このジョブが保存した作品を、そのまま更新監視に載せるか。
    /// 設定の「保存した作品を自動で監視する」から渡ってくる。
    #[serde(default)]
    pub watch_saved: Option<bool>,
    /// 作者とシリーズの新作だけを見る軽い確認にするか。
    ///
    /// 作品ごとの確認と改稿の確認、プロフィールの更新をしない。
    #[serde(default)]
    pub new_works_only: Option<bool>,
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
    #[serde(default)]
    pub new_works_only: bool,
    #[serde(default)]
    pub subject_label: Option<String>,
    #[serde(default)]
    pub subject_source: Option<String>,
    pub totals: i64,
    pub processed: i64,
    pub check_total: i64,
    pub check_processed: i64,
    pub save_total: i64,
    pub save_processed: i64,
    pub candidate_count: i64,
    pub saved_count: i64,
    pub error_count: i64,
    #[serde(default)]
    pub held_count: i64,
    /// 前の回から同じ理由で保留のままの件数。`held_count` には入らない。
    #[serde(default)]
    pub known_held_count: i64,
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
    #[serde(default, skip_serializing_if = "UpdateWorkMetadata::is_empty")]
    pub metadata: UpdateWorkMetadata,
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<crate::library::LibraryId>,
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
    #[serde(default)]
    pub new_works_only: bool,
    pub totals: i64,
    pub processed: i64,
    pub check_total: i64,
    pub check_processed: i64,
    pub save_total: i64,
    pub save_processed: i64,
    pub candidate_count: i64,
    pub saved_count: i64,
    pub error_count: i64,
    #[serde(default)]
    pub held_count: i64,
    /// 前の回から同じ理由で保留のままの件数。`held_count` には入らない。
    #[serde(default)]
    pub known_held_count: i64,
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
    pub item_type: String,
    pub source: Option<String>,
    pub source_id: Option<String>,
    pub target_type: Option<String>,
    pub title: String,
    pub payload_json: String,
}

/// ジョブの項目ひとつの、いまの状態。画面が行に印を付けるために使う。
///
/// `payload_json` は返さない。何百件もの項目を画面へ渡すのに、使わない本文を
/// 一緒に運ぶ理由が無い。**画面へ渡すものは、画面が使うものだけにする。**
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateJobItemState {
    #[serde(default, skip_serializing_if = "UpdateWorkMetadata::is_empty")]
    pub metadata: UpdateWorkMetadata,
    pub source: Option<String>,
    pub source_id: Option<String>,
    pub status: String,
    pub error: Option<String>,
    /// Present for explicit item lists; progress deltas stay small.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<crate::library::LibraryId>,
}

/// A small progress notification for a running update/save job.
///
/// Full snapshots carry candidate and log pages and are intentionally kept for
/// explicit screen loads. Per-item worker events use this delta so an 800-item
/// save does not serialize the same hundreds of rows after every item.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateJobProgressDelta {
    pub summary: UpdateJobSummary,
    pub changed_item: Option<UpdateJobItemState>,
    pub latest_log: Option<UpdateJobLog>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_id: Option<crate::library::LibraryId>,
    #[serde(default, skip_serializing_if = "UpdateWorkMetadata::is_empty")]
    pub metadata: UpdateWorkMetadata,
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

/// Display metadata comes from the saved row or the already recorded provider
/// payload. Listing candidates must not open thousands of saved JSON files.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateWorkMetadata {
    pub author_name: Option<String>,
    pub published_at: Option<String>,
    pub updated_at: Option<String>,
    pub saved_at: Option<String>,
}

impl UpdateWorkMetadata {
    /// 実行キューの表示用の写し。本文を含めず、移行と通常の作成で同じ形にする。
    pub(crate) fn execution_display(payload: &str) -> serde_json::Result<String> {
        crate::library::ExecutionMetadata::execution_display(payload)
    }
    fn is_empty(&self) -> bool {
        self.author_name.is_none()
            && self.published_at.is_none()
            && self.updated_at.is_none()
            && self.saved_at.is_none()
    }
    pub(crate) fn from_payload(payload: &serde_json::Value) -> Self {
        let m = crate::library::ExecutionMetadata::from_payload(payload);
        Self {
            author_name: m.author_name,
            published_at: m.published_at,
            updated_at: m.updated_at,
            saved_at: m.saved_at,
        }
    }
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

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchParams {
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
    pub min_asset_count: Option<i64>,
    pub max_asset_count: Option<i64>,
    pub min_file_size_bytes: Option<i64>,
    pub max_file_size_bytes: Option<i64>,
    pub asset_filter: Option<String>,
    pub series_filter: Option<String>,
    /// "revised" | "first_version".
    pub revision_filter: Option<String>,
    /// "attention" | "incomplete". Only saved works can appear in the library.
    pub acquisition_filter: Option<String>,
    /// "edited" | "unedited". Refers to an active local edit revision.
    pub edit_filter: Option<String>,
    /// "has_cover" | "no_cover".
    pub cover_filter: Option<String>,
    pub date_field: Option<String>,
    pub date_from: Option<String>,
    pub date_to: Option<String>,
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
    /// "current" (default) | "all". Historical revisions remain grouped by work.
    pub version_scope: Option<String>,
}

fn is_zero(value: &i64) -> bool {
    *value == 0
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

/// Smart Search インデックスの構築状況
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchIndexStatus {
    pub total_downloads: i64,
    pub indexed_downloads: i64,
    pub pending_downloads: i64,
    /// 全文索引だけの話。意味索引の遅れはここに入らないので、画面が
    /// 「意味検索も最新」と言うために使ってはいけない。
    pub is_complete: bool,
    pub phase: String,
    pub semantic_indexed_chunks: i64,
    pub semantic_indexed_downloads: i64,
    pub semantic_pending_downloads: i64,
    pub semantic_enabled: bool,
    pub semantic_model_ready: bool,
    pub embedding_provider: String,
    pub gpu_enabled: bool,
    pub throughput_per_sec: Option<f64>,
}

/// 新規アセット挿入用
#[derive(Debug, Clone)]
pub struct NewAsset {
    pub asset_type: String,
    pub filename: String,
    pub local_path: String,
    pub mime_type: Option<String>,
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
        assert!(request.new_works_only.is_none());
        assert!(request.adhoc_targets.is_none());
    }
}
