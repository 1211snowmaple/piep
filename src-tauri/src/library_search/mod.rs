//! 新しい棚の本文の検索。
//!
//! 文書の材料（本文の取り出し、FANBOX の添付からの取り込み）と索引（全文索引の重み・
//! 字句解析・問い合わせの解釈）は旧棚の実装をそのまま使う。棚の中核（`crate::library`）は
//! それらに依存せず単体で組み立てられる必要があるので、ここに分けて置く。棚の側には、
//! 索引の文書と作品の対応の表（`search_documents`）だけがある。

mod document;
mod index;
mod query;
mod results;
mod semantic;

/// 意味検索のモデルの置き場（`<置き場>/models`）。棚の道具からも同じ場所を使う。
pub use crate::database::semantic_index::set_model_cache_dir;
pub use document::{source_url, SearchDocument};
pub use index::{HistoryIndexReport, SearchHit, SearchIndexReport};
pub use query::{parse_query, work_from_url, QueryParts, SeriesRef, WorkRef};
pub use results::{
    EntityIntent, SearchDecoration, SearchMatch, SearchMode, SearchPage, TextSearch, VersionScope,
};
pub use semantic::{SemanticEvidence, SemanticIndexReport, SemanticIndexState};
