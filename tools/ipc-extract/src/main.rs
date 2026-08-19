//! piep のフロント・バック境界をソースから抽出する。
//!
//! 出力は JSON ひとつ。Markdown への整形とドリフト検査は docs-tools 側が行う。
//! ここで判断をしないのは、抽出と判断を分けておけば「事実は合っているのに
//! 検査が厳しすぎる」といった調整を、抽出器に触らずにできるからである。
//!
//! 使い方:
//! ```text
//! cargo run --manifest-path tools/ipc-extract/Cargo.toml -- --repo . --out contract.json
//! ```

mod model;
mod rust_scan;
mod sql_scan;

use model::Contract;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let (repo, out) = parse_args()?;
    let repo = repo.canonicalize()?;

    let commands_dir = repo.join("src-tauri/src/commands");
    let lib_rs = repo.join("src-tauri/src/lib.rs");
    let src_dir = repo.join("src-tauri/src");
    let schema_rs = repo.join("src-tauri/src/database/schema.rs");

    for p in [&commands_dir, &lib_rs, &src_dir, &schema_rs] {
        anyhow::ensure!(p.exists(), "{} が見つからない", p.display());
    }

    let contract = Contract {
        commands: rust_scan::scan_commands(&commands_dir, &repo)?,
        registered: rust_scan::scan_registered(&lib_rs)?,
        events: rust_scan::scan_events(&src_dir, &repo)?,
        tables: sql_scan::scan_tables(&schema_rs, &repo)?,
    };

    eprintln!(
        "コマンド {} / 登録 {} / イベント送出 {} 箇所 / テーブル {}",
        contract.commands.len(),
        contract.registered.len(),
        contract.events.len(),
        contract.tables.len(),
    );

    let json = serde_json::to_string_pretty(&contract)?;
    match out {
        Some(path) => {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, json + "\n")?;
            eprintln!("書き出した: {}", path.display());
        }
        None => println!("{json}"),
    }
    Ok(())
}

/// `--repo <path>` と `--out <path>`。`--out` を省くと標準出力へ流す。
fn parse_args() -> anyhow::Result<(PathBuf, Option<PathBuf>)> {
    let mut repo = PathBuf::from(".");
    let mut out = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--repo" => repo = args.next().map(PathBuf::from).unwrap_or(repo),
            "--out" => out = args.next().map(PathBuf::from),
            other => anyhow::bail!("知らない引数: {other}"),
        }
    }
    Ok((repo, out))
}
