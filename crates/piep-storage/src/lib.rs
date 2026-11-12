//! デスクトップ・Android 共通の可逆保管。UI・認証・SQLite の状態は扱わない。
//! 保存形式 v1 と既存の元ハッシュは変えない。

mod fingerprint;
mod store;

pub use fingerprint::Fingerprint;
pub use store::{
    decode_chunk, regular_directory, regular_file, sync_directory, ChunkEncoding, ChunkState,
    ContentFile, FileChunk, ObjectStore, StorageStats, StoredFile, VerifiedSession,
    MAX_CHUNK_ORIGINAL_BYTES, MAX_CHUNK_STORED_BYTES, MAX_ENCODED_CHUNK_BYTES, OBJECT_HEADER_BYTES,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("ファイルの処理に失敗しました: {0}")]
    Io(#[from] std::io::Error),
    #[error("保存データの完全性を確認できません: {0}")]
    Integrity(String),
    #[error("保存先が安全な管理領域ではありません: {}", .0.display())]
    UnsafePath(std::path::PathBuf),
}

pub type Result<T> = std::result::Result<T, Error>;
