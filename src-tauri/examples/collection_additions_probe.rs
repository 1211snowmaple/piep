//! すでにある束へ、あとから合う作品を探す規則を、実データに当てて見る。
//!
//! 試験用の小さな棚では、規則が効いているかどうかまでは分からない。走査の
//! 相棒として、3,900作の棚に当てて中身を目で確かめるための道具である。
//!
//! **書き込む。** 必ず本番の DB ではなく写しを渡すこと。
//!
//!     cargo run --release --example collection_additions_probe -- <piep.db の写し> <storage_dir>
//!
//! `storage_dir` は `downloads` フォルダ（意味索引はその隣の `search/` を見る）。
//!
//! 束をひとつだけ見たいときは `PROBE_COLLECTION=<id>` を置く。

use std::path::Path;

use piep_lib::database::Database;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: collection_additions_probe <piep.db> <storage_dir>".into());
    }
    let db = Database::open(Path::new(&args[1]), Path::new(&args[2]))?;

    let only = std::env::var("PROBE_COLLECTION").unwrap_or_default();
    let collections = db.list_work_collections()?;
    if collections.is_empty() {
        println!("コレクションが一つも無いので、足す先がない。");
        return Ok(());
    }

    let started = std::time::Instant::now();
    let mut looked = 0usize;
    let mut offered = 0usize;
    for collection in &collections {
        if !only.is_empty() && collection.id != only {
            continue;
        }
        let result = db.suggest_collection_additions(&collection.id)?;
        looked += 1;
        offered += result.candidates.len();

        println!(
            "\n=== {} （{}作品）",
            result.collection_name, collection.member_count
        );
        if let Some(note) = &result.note {
            println!("    ※ {note}");
        }
        if result.candidates.is_empty() {
            println!("    足せそうな作品なし");
            continue;
        }
        println!(
            "    {}件を提示（下限を越えたのは{}件／意味索引 {}）",
            result.candidates.len(),
            result.eligible_count,
            if result.semantic_used {
                "あり"
            } else {
                "なし"
            }
        );
        for candidate in &result.candidates {
            println!(
                "      [{:.2}] {} — {}",
                candidate.confidence,
                truncate(&candidate.title, 46),
                candidate.reason
            );
        }
    }

    println!(
        "\n{looked}束を見て、合計{offered}件を提示／{:.1}秒",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// 題名は見当が付けば足りる。端末の一行を超えると、かえって読めない。
fn truncate(text: &str, limit: usize) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= limit {
        return text.to_string();
    }
    chars.into_iter().take(limit).collect::<String>() + "…"
}
