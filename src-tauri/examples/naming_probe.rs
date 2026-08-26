//! 命名エンジンの探索と試し書きを、実際の棚に対して確かめる。
//!
//!     cargo run --example naming_probe -- <piep.db の写し> <storage_dir>
//!
//! 書き込みはしないが、写しを渡すのが安全である。

use std::path::Path;

use piep_lib::database::Database;

#[tokio::main]
async fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: naming_probe <piep.db> <storage_dir>".into());
    }
    let db = Database::open(Path::new(&args[1]), Path::new(&args[2]))?;

    let found = piep_lib::assist::discover_engines().await;
    println!("見つかった推論サーバー: {}件", found.len());
    for engine in &found {
        println!(
            "  {} — {} (モデル {}件)",
            engine.label,
            engine.base_url,
            engine.models.len()
        );
        for model in engine.models.iter().take(4) {
            println!("      {model}");
        }
    }
    let Some(engine) = found.into_iter().find(|value| !value.models.is_empty()) else {
        println!("試し書きは省略します（使えるサーバーがありません）");
        return Ok(());
    };

    let works = db.sample_naming_works()?;
    println!("\n試し書きに渡す作品: {}件", works.len());
    for work in works.iter().take(4) {
        println!(
            "  {} / タグ {:?}",
            work.title.chars().take(46).collect::<String>(),
            work.tags.iter().take(4).collect::<Vec<_>>()
        );
    }

    let config = piep_lib::assist::AssistEngine {
        base_url: engine.base_url.clone(),
        model: engine.models[0].clone(),
        remote_consent_url: None,
        allow_body: false,
    };
    match piep_lib::assist::try_engine(&config, &works).await {
        Ok(result) => println!(
            "\n返ってきたもの（{}）\n  名前: {}\n  説明: {}\n  {}ミリ秒",
            config.model, result.name, result.subtitle, result.elapsed_ms
        ),
        Err(error) => println!("\n試し書きに失敗: {error}"),
    }
    Ok(())
}
