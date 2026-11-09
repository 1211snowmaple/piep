//! HTTP・認証・DB に依存しない転送契約。内部 manifest をそのまま公開しない。
//! UUID は標準形、ハッシュは小文字 hex、u64 は十進文字列で渡す。
mod scalars;
mod transfer;

pub use piep_storage::{Error, Result};
pub use scalars::{Decimal, Hash, Id};
pub use transfer::{
    Chunk, File, ResourceReference, ResourceScope, TransferManifest, TransferTarget,
    ValidatedTransfer, MAX_BODY_BYTES, MAX_FILE_BYTES, MAX_FILE_CHUNKS, MAX_MANIFEST_BYTES,
};
