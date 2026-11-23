//! HTTP・認証・DB に依存しない転送契約。内部 manifest をそのまま公開しない。
//! UUID は標準形、ハッシュは小文字 hex、u64 は十進文字列で渡す。
mod bundle;
mod identity;
mod pairing;
mod scalars;
mod transfer;

pub use bundle::{BundleFile, ReaderBundle, BUNDLE_FORMAT, MAX_BUNDLE_FILES};
pub use identity::{Capabilities, LibraryIdentity, MAX_IDENTITY_BYTES};
pub use pairing::{PairReply, PairReplyError, Role, API_VERSION};
pub use piep_storage::{Error, Result};
pub use scalars::{Decimal, Hash, Id};
pub use transfer::{
    Chunk, File, ResourceReference, ResourceScope, TransferManifest, TransferTarget,
    ValidatedTransfer, MAX_BODY_BYTES, MAX_FILE_BYTES, MAX_FILE_CHUNKS, MAX_MANIFEST_BYTES,
};

/// ある時点で棚に並んでいる作品と、読書に使う保存版・反映中の編集。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkReadingView {
    pub entity_id: Id,
    pub snapshot_id: Id,
    pub edit_id: Option<Id>,
}

/// 携帯の作品一覧に載せる、棚の作品と読書用の版。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReaderWork {
    pub view: WorkReadingView,
    pub provider: String,
    pub key: String,
    pub title: String,
    pub author_name: String,
    pub content_type: String,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReaderWorkPage {
    pub items: Vec<ReaderWork>,
    pub next: Option<String>,
}
