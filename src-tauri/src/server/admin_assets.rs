//! 実行ファイルに埋め込む管理画面。`build.rs` が `admin-dist` から一覧を作る。
//!
//! 参照するのは `standalone` だけにする。デスクトップ版の実行ファイルに画面を載せないためである。

use super::http::AdminAsset;

include!(concat!(env!("OUT_DIR"), "/admin_assets.rs"));
