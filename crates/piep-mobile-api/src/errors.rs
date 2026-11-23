use serde::{Deserialize, Serialize};

/// 読み手の API が返す、HTTP の状態と組にして扱うコード。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReaderErrorCode {
    Unauthorized,
    InvalidRequest,
    ResourceUnavailable,
    StorageError,
    InvalidRange,
    OriginForbidden,
    UriTooLong,
    Busy,
    ArchiveForbidden,
    StaleCursor,
    SearchIndexing,
    AssetTooLarge,
    ReadingCursorExpired,
    FavoriteConflict,
    ReadingConflict,
}

#[derive(Serialize, Deserialize)]
pub struct ReaderError {
    pub code: ReaderErrorCode,
}

/// 書き込みの世代が競合したときに返す、棚の現在値。
#[derive(Serialize, Deserialize)]
pub struct MutationConflict<T> {
    pub code: ReaderErrorCode,
    pub current: T,
}
