//! 携帯端末とサーバーの間を流れる読取専用データの型。
//!
//! サーバー（`src-tauri/src/server`）がこの形で送り、携帯の Rust（`android/src-tauri`）が
//! 同じ型で受けて、そのまま画面へ渡す。画面側の TypeScript の型
//! （`shared/mobileApi.ts`）は、このファイルから `tools/api-types` が生成する。
//! 型を足すとき・項目を変えるときは、ここを直して生成し直す。
//!
//! ここには型と、両側で同じにしたい検査（一覧の条件の `validate`）だけを置く。
//! 棚の問い合わせへの変換はサーバー側（`server::reader`）にある。

use serde::{Deserialize, Serialize};

mod errors;
mod search;
pub use errors::{MutationConflict, ReaderError, ReaderErrorCode};
pub use search::*;

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
    pub cover_mode: String,
    pub cover_revision: i64,
    pub has_single_cover: bool,
    pub has_file_cover: bool,
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
pub struct MobilePage {
    pub page: usize,
    pub page_count: usize,
    pub html: String,
    pub total_plain_text_chars: usize,
    pub source_page_starts: Vec<usize>,
}

/// 棚の作品の一件（棚の UUID で指す）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileWork {
    /// 作品の UUID。
    pub id: String,
    /// 今の保存版の UUID。版が変わると変わるので、表紙を取り直す目印になる。
    pub snapshot_id: String,
    pub source: String,
    pub source_id: String,
    pub title: String,
    pub author_name: String,
    /// 取得元の作者の ID。分からなければ空。
    pub author_id: String,
    pub content_type: String,
    pub tags: Vec<String>,
    pub excerpt: Option<String>,
    pub text_length: i64,
    pub asset_count: i64,
    pub has_cover: bool,
    pub current_version: i64,
    /// 反映中の編集があるか。
    pub edited: bool,
    /// 反映中の編集の UUID。編集し直すと変わるので、読書位置と保存の印を分ける目印になる。
    pub edit_id: Option<String>,
    pub favorite: bool,
    /// この利用者のお気に入りの更新世代（十進の文字列）。付け外しの `expected` に渡す。
    pub favorite_generation: String,
    pub series_title: Option<String>,
    pub series_id: Option<String>,
    /// 公開日（ISO 8601）。
    pub source_created_at: Option<String>,
    /// 保存日（ISO 8601）。
    pub saved_at: String,
}

/// 棚の作品のお気に入りを付け外しする。`request_id` は端末が作る UUID で、送り直しても
/// 一度だけ効く。`expected` は手元の更新世代（十進の文字列）。違えば 409 で今の値が返る。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileFavoriteUpdate {
    pub request_id: String,
    pub expected: String,
    pub favorite: bool,
}

/// お気に入りの今の値と更新世代（十進の文字列）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileFavorite {
    pub favorite: bool,
    pub generation: String,
}

/// 棚の読書位置の場所。`view_kind` は `snapshot`（版の写し）か `edit`（編集）で、
/// `view_id` はその UUID。ページは本文の区切り（pixiv の `[newpage]`、編集の改ページ）で数えた
/// 0 からの番号、`ratio_ppm` はそのページの中のスクロールの割合（百万分率）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileReadingLocation {
    pub view_kind: String,
    pub view_id: String,
    pub page: u64,
    pub ratio_ppm: u32,
}

/// 棚の作品の読書位置（認証した利用者のもの）。`location` が空なのは、消した位置か、
/// ページで表せない位置。`mine` はこの端末が書いた位置か。`updated_at` は棚が受けた時刻
/// （ミリ秒）。`generation` は更新世代（十進の文字列。まだ位置が無ければ "0"）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileReading {
    pub work: String,
    pub location: Option<MobileReadingLocation>,
    pub finished: bool,
    pub mine: bool,
    pub updated_at: Option<i64>,
    pub generation: String,
}

/// 読書位置の一覧の一ページ。初めは `cursor` を空で頼み、返った `cursor` を次へ渡す。
/// `more` なら続きがある。続きが無くなったあとも、同じ `cursor` で変わった分だけを受け取れる。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileReadingPage {
    pub items: Vec<MobileReading>,
    pub cursor: String,
    pub more: bool,
}

/// 棚の作品の読書位置を書く。`request_id` は端末が作る UUID で、送り直しても一度だけ
/// 効く。`expected` は手元の更新世代（十進の文字列）。違えば 409 で今の位置が返る。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileReadingUpdate {
    pub request_id: String,
    pub expected: String,
    pub location: MobileReadingLocation,
    pub finished: bool,
}

/// 棚の一覧の一ページ。続きは `next_cursor` を変えずに次の要求へ渡す。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileWorkPage {
    pub items: Vec<MobileWork>,
    pub next_cursor: Option<String>,
    pub total_estimate: Option<i64>,
    pub index_complete: bool,
}

/// 棚の作品の保存した版の一つ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileVersion {
    /// その版の写しの ID。以前の版を読むときはこの写しの本を引く。
    pub snapshot_id: String,
    pub version: i64,
    pub text_length: Option<i64>,
    /// 保存日（ISO 8601）。
    pub saved_at: String,
}

/// 棚の作品の画面。`work.excerpt` は取得元の説明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileWorkDetail {
    pub work: MobileWork,
    pub people: Vec<MobilePerson>,
    pub series: Vec<MobileSeriesMembership>,
    /// 古い順。
    pub versions: Vec<MobileVersion>,
    /// 反映中の編集の基準の版。編集がなければ空。
    pub edit_base_version: Option<i64>,
}

/// 棚の検索欄の候補の一件。作者・シリーズは
/// `source`・`source_key` が取得元と ID、題は `value` が取得元の ID で `source_key` が棚の
/// 作品の ID、タグは `value` がタグ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileSearchSuggestion {
    pub kind: String,
    pub label: String,
    pub value: String,
    pub count: Option<i64>,
    pub exact_match: bool,
    pub source: Option<String>,
    pub source_key: Option<String>,
}

/// 棚の検索欄の候補の一覧。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileSearchSuggestions {
    pub items: Vec<MobileSearchSuggestion>,
}

/// 棚のコレクションの一回。棚に無い（未保存の）回は `work` が空で、入れた時の題と作者。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileCollectionMember {
    pub title: String,
    pub author_name: String,
    pub position: i64,
    pub work: Option<MobileWork>,
}

/// 棚のコレクションの画面。`id` は棚の UUID、表紙は `cover_mode` の枠ごとに
/// `/collections/{id}/cover/{single|file|tile-N}` から引く。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileCollectionDetail {
    #[serde(flatten)]
    pub summary: MobileCollection,
    pub members: Vec<MobileCollectionMember>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_collection_detail_is_one_flat_object_on_the_wire() {
        let json = r#"{"id":"c1","name":"棚","description":null,"collectionKind":"manual","memberCount":1,"availableCount":0,"coverMode":"tiles","coverRevision":0,"hasSingleCover":false,"hasFileCover":false,"coverTiles":[],"members":[{"title":"題","authorName":"作者","position":0,"work":null}]}"#;
        let detail: MobileCollectionDetail = serde_json::from_str(json).unwrap();
        assert_eq!(detail.summary.cover_mode, "tiles");
        assert_eq!(detail.members.len(), 1);
        let written = serde_json::to_value(&detail).unwrap();
        assert_eq!(written["name"], "棚");
        assert!(written.get("position").is_none());
        assert!(written.get("summary").is_none());
    }
}
