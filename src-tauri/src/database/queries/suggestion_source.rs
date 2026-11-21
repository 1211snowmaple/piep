//! コレクションの提案が読む材料の出どころ。
//!
//! 提案の計算（走査・種からの提案・追加の候補・名前の案）は、作品・シリーズ・タグ・本文リンク・
//! 採否の記録を SQL で引く。棚から同じ形の表をメモリ上に組んで読む
//! （`crate::library_suggestion_corpus`）。表の外にあるもの（意味索引・本文）は、ここで棚から引く。
//!
//! 作品の番号（`downloads.id`）は、作業用の表の中だけの番号になる。

use super::*;
use std::ops::Deref;

/// 作業用の表への接続（メモリ上の表）。
pub(crate) enum Corpus<'a> {
    Borrowed(&'a Connection),
}

impl Deref for Corpus<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        match self {
            Corpus::Borrowed(conn) => conn,
        }
    }
}

pub(crate) trait SuggestionSource {
    /// 旧棚の形の表（downloads・download_series・tags・download_tags・work_links・
    /// collection_pair_feedback・work_collections・work_collection_members）。
    fn corpus(&self) -> Result<Corpus<'_>, String>;
    /// 作品ごとの本文ベクトル。意味索引が読めないときは Err。
    fn centroids(&self, wanted: &HashSet<i64>) -> Result<HashMap<i64, Vec<f32>>, String>;
    /// 意味索引に作品が一つでもあるか。
    fn semantic_ready(&self) -> bool;
    /// 種の作品に近い作品（作品のベクトル同士を比べる。モデルは読み込まない）。
    fn similar_works(&self, seeds: &[i64], limit: usize) -> Result<Vec<(i64, f64)>, String>;
    /// 本文（版や合本の見分けに使う）。読めなければ None。
    fn plain_text(&self, id: i64) -> Option<String>;
}
