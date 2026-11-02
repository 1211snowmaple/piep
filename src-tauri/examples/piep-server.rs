//! 単独サーバー。常時動いている Windows 機で本棚を携帯端末へ出す。
//!
//! example に置いているのは、`src/bin` の実行ファイルは Tauri がデスクトップ版のインストーラーへ
//! 同梱してしまうからである。β の間に全員のインストーラーを 2 倍にはしない。組むときは
//! `cargo build --release --example piep-server`。本体は `piep_lib::server::standalone`。

#[tokio::main]
async fn main() {
    let args = std::env::args().skip(1).collect();
    std::process::exit(piep_lib::server::standalone::main_from_args(args).await);
}
