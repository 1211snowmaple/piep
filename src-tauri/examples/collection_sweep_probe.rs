//! 実データに対して束の走査を走らせ、何が出るかを見る。
//!
//! 試験用の小さな棚では、規則が効いているかどうかまでは分からない。
//! 3,900作の棚に当てて、束の数と中身を目で確かめるための道具である。
//!
//! **書き込む。** 必ず本番の DB ではなく写しを渡すこと。
//!
//!     cargo run --example collection_sweep_probe -- <piep.db の写し> <storage_dir>
//!
//! `storage_dir` は `downloads` フォルダ（意味索引はその隣の `search/` を見る）。

use std::path::Path;

use piep_lib::database::Database;

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: collection_sweep_probe <piep.db> <storage_dir>".into());
    }
    let db = Database::open(Path::new(&args[1]), Path::new(&args[2]))?;

    let started = std::time::Instant::now();
    let swept = db.sweep_collection_candidates()?;
    let bundles = swept.bundles;
    let elapsed = started.elapsed();

    let sequences = bundles.iter().filter(|b| b.track == "sequence").count();
    let themes = bundles.iter().filter(|b| b.track == "theme").count();
    let works: usize = bundles.iter().map(|b| b.members.len()).sum();
    println!(
        "束 {} 件（続き物 {sequences} / テーマ {themes}）／作品 {works} 件／{:.1}秒",
        bundles.len(),
        elapsed.as_secs_f64()
    );
    if !swept.saved_search_suggestions.is_empty() {
        println!(
            "
束にしなかったタグ（保存した検索向き）:"
        );
        for idea in &swept.saved_search_suggestions {
            println!("  {} ({}作)", idea.tag, idea.work_count);
        }
    }

    // PROBE_TRACK=theme のように絞れると、片方の系統だけを見られる。
    let track_filter = std::env::var("PROBE_TRACK").unwrap_or_default();
    for bundle in bundles
        .iter()
        .filter(|b| track_filter.is_empty() || b.track == track_filter)
        .take(20)
    {
        let authors = bundle
            .members
            .iter()
            .map(|member| member.author_name.as_str())
            .collect::<std::collections::HashSet<_>>();
        println!(
            "\n--- [{}] {} 作 / 作者 {} 人\n    名前: {}\n    根拠: {}",
            bundle.track,
            bundle.members.len(),
            authors.len(),
            bundle.proposed_name,
            bundle.evidence_summary
        );
        for option in &bundle.name_options {
            println!("      案({}) {}", option.source, option.name);
        }
        for member in bundle.members.iter().take(4) {
            let title: String = member.title.chars().take(54).collect();
            println!("      - {title}");
        }
    }
    Ok(())
}
