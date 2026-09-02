//! 実データに対して、モデルへ頼む仕事をひととおり試す。
//!
//!     cargo run --example assist_probe -- <piep.db の写し> <storage_dir>
//!
//! 本文を送る仕事も含むので、写しを渡すこと。DB には書き込まない。

use std::path::Path;

use piep_lib::assist::{self, AssistEngine};
use piep_lib::database::Database;

#[tokio::main]
async fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: assist_probe <piep.db> <storage_dir>".into());
    }
    let db = Database::open(Path::new(&args[1]), Path::new(&args[2]))?;

    let found = assist::discover_engines().await;
    let Some(discovered) = found.into_iter().find(|value| !value.models.is_empty()) else {
        println!("推論サーバーが見つかりません");
        return Ok(());
    };
    let engine = AssistEngine {
        base_url: discovered.base_url.clone(),
        model: discovered.models[0].clone(),
        remote_consent_url: None,
        allow_body: true,
        feature_profile: None,
    };
    println!("使うモデル: {} @ {}\n", engine.model, engine.base_url);

    let vocabulary = db.tag_vocabulary()?;
    println!("タグの語彙: {}語", vocabulary.len());

    // 1. タグの補完
    let ids = db.sample_bundle_like_ids_public()?;
    let target = ids.first().copied().ok_or("試す作品がありません")?;
    let work = db.work_facts(target)?;
    println!("\n--- タグの補完\n  対象: {}", trim(&work.title, 50));
    println!("  いまのタグ: {:?}", work.tags);
    match assist::suggest_tags(&engine, &work, &vocabulary).await {
        Ok(found) => {
            for value in found {
                println!("    + {} — {}", value.tag, value.reason);
            }
        }
        Err(error) => println!("    失敗: {error}"),
    }

    // 2. 言葉で探す
    println!("\n--- 言葉で探す");
    let phrase = "旅先で出会った二人が、少しずつ仲良くなる話";
    println!("  入力: {phrase}");
    match assist::interpret_search(&engine, phrase, &vocabulary).await {
        Ok(intent) => {
            println!("    読み: {}", intent.reading);
            println!("    含める: {:?}", intent.include_tags);
            println!("    除く: {:?}", intent.exclude_tags);
            println!("    本文から: {}", intent.query);
        }
        Err(error) => println!("    失敗: {error}"),
    }

    // 3. 作風のメモ
    println!("\n--- 作風のメモ（作者: {}）", work.author_name);
    match db.author_facts_by_name(&work.author_name) {
        Ok((author, works)) => match assist::describe_author(&engine, &author, &works).await {
            Ok(note) => println!("    {}", note.text),
            Err(error) => println!("    失敗: {error}"),
        },
        Err(error) => println!("    材料を作れません: {error}"),
    }

    // 4. 束の分割
    println!("\n--- 束の分割");
    let facts = ids
        .iter()
        .filter_map(|id| db.work_facts(*id).ok())
        .collect::<Vec<_>>();
    match assist::propose_splits(&engine, &facts).await {
        Ok(splits) if splits.is_empty() => println!("    分ける必要は無い、との答え"),
        Ok(splits) => {
            for split in splits {
                println!(
                    "    {} {:?} — {}",
                    split.name, split.positions, split.reason
                );
            }
        }
        Err(error) => println!("    失敗: {error}"),
    }

    // 5. あらすじ（本文を送る）
    println!("\n--- あらすじ");
    match db.work_facts_with_body(target) {
        Ok((facts, body)) => {
            println!("    本文 {}字", body.chars().count());
            match assist::summarize_work(&engine, &facts, &body).await {
                Ok(note) => println!("    {}", note.text),
                Err(error) => println!("    失敗: {error}"),
            }
        }
        Err(error) => println!("    本文を読めません: {error}"),
    }

    // 6. 前回のあらすじ（本文を送る）
    if let Some(previous) = ids.get(1).copied() {
        println!("\n--- 前回のあらすじ");
        match db.work_facts_with_body(previous) {
            Ok((facts, body)) => {
                println!("    前の話: {}", trim(&facts.title, 46));
                match assist::recap_previous(&engine, &facts.title, &body).await {
                    Ok(note) => println!("    {}", note.text),
                    Err(error) => println!("    失敗: {error}"),
                }
            }
            Err(error) => println!("    本文を読めません: {error}"),
        }
    }

    // 本文の許可が無いときは、頼む前に断ること。
    let no_body = AssistEngine {
        allow_body: false,
        ..engine
    };
    println!(
        "\n--- 本文の許可が無いとき: {:?}",
        assist::ensure_body_allowed(&no_body).err()
    );
    Ok(())
}

fn trim(value: &str, count: usize) -> String {
    value.chars().take(count).collect()
}
