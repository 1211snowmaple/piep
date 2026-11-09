//! 保管の実装は端末と共有する crate に置く。保存形式の別実装を持たない。
pub(crate) use piep_storage::{regular_directory, regular_file, sync_directory};
pub use piep_storage::{ContentFile, FileChunk, ObjectStore, StorageStats};
