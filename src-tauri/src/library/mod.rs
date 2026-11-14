//! 取得元の改稿、取得の記録、保存状態、元ファイルを分けて扱うライブラリ。
//!
//! 旧形式への読み取りは移行器だけに閉じ込める。本番の保存先を自動変換しない。

mod acquisitions;
mod annotations;
mod archive;
mod body;
mod catalogue;
mod editing;
mod identity;
mod integrity;
mod job_history;
mod legacy;
mod maintenance;
mod operations;
mod reader;
mod reader_references;
mod reading;
mod renditions;
mod repository;
mod resources;
mod saved_state;
mod schema;
mod searches;
mod source_state;
mod store;
mod suggestions;
mod transfer;
mod work_list;

pub use piep_library_protocol as protocol;

pub use acquisitions::{
    AcquisitionInfo, AcquisitionPayload, AcquisitionReaderOutput, AcquisitionState,
    AcquisitionTicket,
};
pub use annotations::{TagProvenance, TagUpdate, UserTag, UserTags};
pub use body::{BodyFormat, SourceBody};
pub use catalogue::{CollectionInfo, CollectionMember, CollectionSave};
pub use editing::{EditBlock, EditContent, EditInfo, EditMedia, EditState};
pub use identity::{cover_identity, image_identity, Fingerprint, LibraryId};
pub use job_history::{JobHistory, JobHistoryItem, JobHistoryLog};
pub use legacy::{LegacyImportReport, LegacyImporter};
pub use maintenance::{AssetAuditMarker, MaintenanceHistory, RepairAttemptMarker};
pub use operations::{EditRequest, SnapshotInfo, UserState};
pub use reader::{
    ReaderBinding, ReaderBindingSet, ReaderBody, ReaderDocument, ResolvedReaderBinding,
};
pub use reader_references::BodyAssetReference;
pub use reading::{ReadingAnchor, ReadingLocation, ReadingPosition, ReadingUpdate};
pub use repository::{
    Capture, CaptureResult, CaptureSource, CapturedAttachment, CapturedMedia, EntityKind, Library,
    WorkSummary,
};
pub use resources::{
    ResourceFile, ResourceFormat, ResourceInfo, ResourceManifest, ResourceRange, ResourceReference,
    ResourceScope,
};
pub use searches::{SavedSearch, SavedSearchDefinition, SavedSearchUpdate};
pub use source_state::SourceEntityState;
pub use store::{ContentFile, FileChunk, ObjectStore, StorageStats};
pub use suggestions::{CollectionSuggestion, SuggestedMember, SuggestionDecision, SuggestionJson};
pub use work_list::{SortValue, WorkCard, WorkCursor, WorkListPage, WorkListQuery, WorkSort};

use std::path::PathBuf;

/// 保管形式・完全性・所有関係の失敗を、呼び手が区別できるエラー。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("ファイルの処理に失敗しました: {0}")]
    Io(#[from] std::io::Error),
    #[error("データベースの処理に失敗しました: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("保存データを解釈できません: {0}")]
    Json(#[from] serde_json::Error),
    #[error("保存データの完全性を確認できません: {0}")]
    Integrity(String),
    #[error("新しい取得や保存が先に進んだため、この取得結果は現在版に適用しませんでした")]
    Superseded,
    #[error("指定された保存状態のファイルを利用できません")]
    ResourceUnavailable,
    #[error("読取範囲が元ファイルを超えています")]
    InvalidRange,
    #[error("対応していないライブラリ形式です: {0}")]
    Unsupported(String),
    #[error("保存先が安全な管理領域ではありません: {}", .0.display())]
    UnsafePath(PathBuf),
    #[error("このライブラリは別の処理で使用中です")]
    Busy,
    #[error("処理を中止しました")]
    Canceled,
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<piep_storage::Error> for Error {
    fn from(error: piep_storage::Error) -> Self {
        match error {
            piep_storage::Error::Io(error) => Self::Io(error),
            piep_storage::Error::Integrity(message) => Self::Integrity(message),
            piep_storage::Error::UnsafePath(path) => Self::UnsafePath(path),
        }
    }
}
