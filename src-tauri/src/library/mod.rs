//! 取得元の改稿、取得の記録、保存状態、元ファイルを分けて扱うライブラリ。

mod acquisitions;
mod annotations;
mod archive;
pub(crate) mod asset_repairs;
mod body;
mod catalogue;
mod discovery;
mod editing;
mod execution;
pub use execution::ExecutionMetadata;
mod execution_payload;
mod identity;
mod integrity;
mod operations;
mod partial;
pub use partial::{PartTransfer, PART_MARKER};
mod pixiv_edit_gaps;
mod profile_capture;
pub use profile_capture::{ProfileSave, PROFILE_REFRESH_ROUTE};
mod publish;
mod reader;
mod reader_catalogue;
mod reader_loans;
mod reader_references;
mod reading;
mod reclaim;
mod renditions;
pub(crate) mod repair_attempts;
mod repository;
mod resources;
mod revision_folds;
mod save_repairs;
pub use save_repairs::MAX_INCOMPLETE_SAVE_REPAIRS;
mod saved_state;
pub(crate) mod schema;
mod searches;
mod shipment;
pub use shipment::adopted_request;
pub(crate) mod source_bindings;
mod source_state;
mod store;
mod suggestions;
mod transfer;
mod watching;
pub use discovery::{Discovery, DiscoveryInput};
mod work_capture;
mod work_list;
pub(crate) mod work_profiles;

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
pub(crate) use identity::{is_pixiv_placeholder_image, masked_pixiv_metadata};
pub use operations::{EditPointerChange, EditRequest, SnapshotInfo, UserState, UserStateChange};
pub use reader::{
    ReaderBinding, ReaderBindingSet, ReaderBody, ReaderDocument, ResolvedReaderBinding,
};
pub use reader_references::BodyAssetReference;
pub use reading::{
    server_device, ReadingAnchor, ReadingLocation, ReadingPosition, ReadingUpdate, PAGE_RATIO_SCALE,
};
pub use reclaim::{ReclaimOptions, ReclaimReport};
pub use repository::{
    Capture, CaptureResult, CaptureSource, CapturedAttachment, CapturedMedia, EntityKind, Library,
    WorkSummary,
};
pub use resources::{
    ResourceFile, ResourceFormat, ResourceInfo, ResourceManifest, ResourceRange, ResourceReference,
    ResourceScope,
};
pub use searches::{SavedSearch, SavedSearchDefinition, SavedSearchUpdate};
pub use source_state::{SourceEntityState, WorkSourceState};
pub use store::{ContentFile, FileChunk, ObjectStore, StorageStats};
pub use suggestions::{CollectionSuggestion, SuggestedMember, SuggestionDecision, SuggestionJson};
pub use transfer::reader_may_read;
pub use watching::{WatchEdit, WatchGroup, WatchResult, WatchTarget};
pub use work_capture::{asset_reference, StagedAsset, WorkSave};
pub use work_list::{
    AcquisitionFilter, CountRange, DateField, DateRange, MediaFilter, NeighborKind, ReimportState,
    RevisionFilter, SortValue, TextMatches, WorkCard, WorkCursor, WorkFilter, WorkListPage,
    WorkListQuery, WorkNeighbors, WorkSort, REIMPORT_INPUT,
};

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
    #[error("読書位置が指定した本文の範囲にありません")]
    InvalidReadingLocation,
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
