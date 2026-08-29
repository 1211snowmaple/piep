//! 意味検索の「問いの作り方」を、実際の棚で測る。
//!
//!     cargo run --release --example semantic_query_probe -- <piep.db の写し> <storage_dir>
//!
//! 書き込みはしないが、**写しを渡すこと**。索引を開くので、動いている piep が
//! あると排他で弾かれる。
//!
//! # 何を比べているか
//!
//! 索引側は本文をそのまま `passage: {本文}` として埋め込む。0.11.0 までは
//! 検索側だけが `query: {かな・ローマ字・同義語を空白で連ねたもの}` を
//! 埋め込んでいた。multilingual-e5 は両側に自然文を期待するモデルなので、
//! 片側だけ語彙の袋詰めにすると埋め込みが本来の位置からずれる。
//!
//! 正解を人手で用意せずに測るため、**作品自身を答えにする**:
//!
//! - 題名を問いにして、その作品が何位に返るか
//! - 本文の途中の一文を問いにして、その作品が何位に返るか
//!
//! どちらも「その作品が上位に来るほど良い」で、作り方どうしの**相対**を見る。
//!
//! # 測った結果（163,393 断片・188 作品）
//!
//! ```text
//! ■ 題名を問いにしたとき          1位     5位内   10位内   MRR
//!   変形形＋同義語（0.11.0 まで）  26.6%   44.1%   45.7%   0.340
//!   正規化した自然文               48.9%   67.6%   70.2%   0.570
//!   そのまま（いま）               69.7%   83.5%   85.6%   0.756
//!
//! ■ 本文の一文を問いにしたとき
//!   変形形＋同義語（0.11.0 まで）   6.9%   12.8%   14.9%   0.098
//!   正規化した自然文               50.5%   59.0%   63.8%   0.546
//!   そのまま（いま）               69.7%   78.2%   82.4%   0.739
//! ```
//!
//! 問いの作り方を変えるときは、ここを測り直してから変えること。

use std::collections::HashSet;
use std::path::Path;

use piep_lib::database::semantic_index;
use piep_lib::database::Database;

/// 測る問いの作り方。
enum Strategy {
    /// 0.11.0 までの作り方。変形形と同義語を空白で連ねる。
    Variants,
    /// 正規化しただけの自然文。
    Normalized,
    /// いまの作り方。手を入れずにそのまま渡す。
    Shipped,
}

impl Strategy {
    fn label(&self) -> &'static str {
        match self {
            Self::Variants => "変形形＋同義語（0.11.0 まで）",
            Self::Normalized => "正規化した自然文",
            Self::Shipped => "そのまま（いま）",
        }
    }

    fn prepare(&self, query: &str) -> String {
        use piep_lib::database::search_normalization::{
            normalize_for_search, query_variants, synonym_variants,
        };
        match self {
            Self::Variants => {
                let mut values = query_variants(query);
                values.extend(synonym_variants(query));
                if values.is_empty() {
                    normalize_for_search(query)
                } else {
                    values.join(" ")
                }
            }
            Self::Normalized => normalize_for_search(query),
            Self::Shipped => semantic_index::semantic_query_text(query),
        }
    }
}

#[derive(Default)]
struct Tally {
    asked: usize,
    hit_at_1: usize,
    hit_at_5: usize,
    hit_at_10: usize,
    reciprocal: f64,
}

impl Tally {
    fn record(&mut self, rank: Option<usize>) {
        self.asked += 1;
        let Some(rank) = rank else { return };
        if rank == 1 {
            self.hit_at_1 += 1;
        }
        if rank <= 5 {
            self.hit_at_5 += 1;
        }
        if rank <= 10 {
            self.hit_at_10 += 1;
        }
        self.reciprocal += 1.0 / rank as f64;
    }

    fn report(&self, label: &str) {
        if self.asked == 0 {
            println!("  {label}: 測れなかった");
            return;
        }
        let pct = |n: usize| n as f64 * 100.0 / self.asked as f64;
        println!(
            "  {label:<26}  1位 {:5.1}%   5位内 {:5.1}%   10位内 {:5.1}%   MRR {:.3}",
            pct(self.hit_at_1),
            pct(self.hit_at_5),
            pct(self.hit_at_10),
            self.reciprocal / self.asked as f64,
        );
    }
}

/// 与えた問いで検索し、その作品が何位に来たかを返す。同じ作品の断片が並ぶので、
/// **作品の単位で順位を数える**。
fn rank_of(storage: &Path, query_text: &str, want: i64, limit: usize) -> Option<usize> {
    let hits = semantic_index::search_with_query_text(storage, query_text, limit).ok()?;
    let mut seen = HashSet::new();
    let mut rank = 0usize;
    for hit in hits {
        if !seen.insert(hit.download_id) {
            continue;
        }
        rank += 1;
        if hit.download_id == want {
            return Some(rank);
        }
    }
    None
}

/// 本文の真ん中あたりから一文を取る。短すぎるものは問いにしない。
fn middle_sentence(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 200 {
        return None;
    }
    let start = chars.len() / 3;
    let slice: String = chars[start..(start + 90).min(chars.len())].iter().collect();
    let trimmed = slice.trim();
    (trimmed.chars().count() >= 30).then(|| trimmed.to_string())
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: semantic_query_probe <piep.db の写し> <storage_dir>".into());
    }
    let db = Database::open(Path::new(&args[1]), Path::new(&args[2]))?;
    let storage = Path::new(&args[2]);

    let status = semantic_index::status(storage);
    println!(
        "意味索引: {} 断片 / モデル {} / 提供 {}",
        status.indexed_chunks,
        semantic_index::model_id(),
        status.provider
    );
    if status.indexed_chunks == 0 {
        return Err("意味索引が空である。先に作ってから測ること".into());
    }

    // 索引に載っている作品から本文の断片を拾い、題名は棚から引く。
    let chunks = semantic_index::probe_samples(storage, 200)?;
    let mut samples = Vec::with_capacity(chunks.len());
    for chunk in chunks {
        let Ok(entry) = db.get_download(chunk.download_id) else {
            continue;
        };
        if entry.title.trim().is_empty() {
            continue;
        }
        samples.push((chunk.download_id, entry.title, chunk.text));
    }
    println!("測る作品: {} 件\n", samples.len());

    let strategies = [Strategy::Variants, Strategy::Normalized, Strategy::Shipped];
    let mut by_title: Vec<Tally> = strategies.iter().map(|_| Tally::default()).collect();
    let mut by_passage: Vec<Tally> = strategies.iter().map(|_| Tally::default()).collect();

    for (index, (download_id, title, text)) in samples.iter().enumerate() {
        if index % 25 == 0 {
            println!("  ... {index}/{}", samples.len());
        }
        let sentence = middle_sentence(text);
        for (slot, strategy) in strategies.iter().enumerate() {
            let title_query = strategy.prepare(title);
            by_title[slot].record(rank_of(storage, &title_query, *download_id, 20));

            if let Some(sentence) = &sentence {
                let passage_query = strategy.prepare(sentence);
                by_passage[slot].record(rank_of(storage, &passage_query, *download_id, 20));
            }
        }
    }

    println!("\n■ 題名を問いにしたとき");
    for (slot, strategy) in strategies.iter().enumerate() {
        by_title[slot].report(strategy.label());
    }
    println!("\n■ 本文の一文を問いにしたとき");
    for (slot, strategy) in strategies.iter().enumerate() {
        by_passage[slot].report(strategy.label());
    }
    println!("\n数字が高いほど、その作り方のほうが「探しているものを上に出す」。");
    Ok(())
}
