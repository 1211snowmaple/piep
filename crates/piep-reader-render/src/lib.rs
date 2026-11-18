//! 保存された作品のデータから、読む画面の HTML とページを組み立てる。
//! デスクトップと携帯が同じものを使い、どちらで読んでもページの切れ目が変わらないようにする。

use serde::{Deserialize, Serialize};

pub mod blocks;
pub mod document;
pub mod fanbox_post;
pub mod mobile;
pub mod pages;
pub mod parser;

/// 添付ファイルの本文の前に置く、注意書きのブロックの種類。
pub const NOTICE_BLOCK_TYPE: &str = "piep_attachment_notice";

/// アセットエントリ（DB行に対応）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetEntry {
    pub id: i64,
    pub download_id: i64,
    pub asset_type: String,
    pub filename: String,
    pub local_path: String,
    pub original_url: Option<String>,
    pub mime_type: Option<String>,
    pub file_size_bytes: i64,
}

/// A block in the local editor document.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkBlock {
    pub id: i64,
    pub edit_revision_id: i64,
    pub order: i64,
    pub block_type: String,
    pub text: Option<String>,
    pub asset_id: Option<i64>,
    pub attrs_json: Option<String>,
}

/// ファイル名サニタイザー
/// OS依存の禁止文字 (< > : " / \ | ? *) や、ディレクトリトラバーサルを示す ".." を安全な文字列に置換する
pub fn sanitize_filename(name: &str) -> String {
    let mut cleaned = name.replace("..", "__");

    // Windowsの不正文字をアンダースコアに置換
    let invalid_chars = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    for &c in &invalid_chars {
        cleaned = cleaned.replace(c, "_");
    }

    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "asset_file".to_string()
    } else {
        trimmed.to_string()
    }
}
