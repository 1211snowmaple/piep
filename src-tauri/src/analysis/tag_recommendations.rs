//! 取得元タグの併用統計から、利用者が確認する候補を作る。
//!
//! 意味上その作品に当てはまるという判定ではない。作品本文や概要を読まず、
//! 同じ棚で取得元が付けたタグがどの程度併用されたかだけを返す。

use serde::{Deserialize, Serialize};

/// 取得元タグの併用統計。候補は意味的な正解を保証しない。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RelatedTagCandidate {
    pub tag: String,
    /// `共通タグを持つN作品中M作品で併用` の説明。
    pub reason: String,
    /// 対象の既存タグを一つ以上持つ、同じ作者を除いた取得元タグ付き作品数。
    pub matching_work_count: i64,
    /// 上記のうち候補タグも取得元から付与されている作品数。
    pub cooccurring_work_count: i64,
}
