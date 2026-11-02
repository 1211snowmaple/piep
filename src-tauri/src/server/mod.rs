//! サーバーモード。本棚を携帯端末へ出す HTTP 層と、機械の間で本棚を写す仕組み。
//!
//! 仕様は `docs/plan/server-mode.md`。デスクトップ版の内蔵サーバーと単独の
//! `piep-server` は、ここの同じ部品で動く。違うのは「本棚をどこから借りるか」と
//! 「状態をどこに置くか」だけである。

pub mod desktop;
pub mod dto;
pub mod http;
pub mod library;
pub mod snapshot;
pub mod standalone;
pub mod store;
pub mod sync_receive;
pub mod sync_send;
#[cfg(test)]
mod tests;
pub mod tls;

/// 携帯端末との契約の版。壊す変更のときだけ上げる。
pub const API_VERSION: u32 = 2;
/// 送受信のかたまりの大きさ。リバースプロキシの上限に引っかからない値にする。
pub const CHUNK_BYTES: usize = 8 * 1024 * 1024;
/// 差分の問い合わせ 1 回あたりのファイル数。
pub const MANIFEST_BATCH: usize = 5_000;
/// 待ち受けの既定のポート。
pub const DEFAULT_PORT: u16 = 38777;

/// 登録文字列（版 2）を作る。鍵を含むので、記録には残さない。
pub fn pairing_payload(
    role: store::Role,
    url: &str,
    fingerprint: Option<&str>,
    secret: &str,
) -> String {
    serde_json::json!({
        "version": 2,
        "role": role.as_str(),
        "url": url,
        "secret": secret,
        "fingerprint": fingerprint,
    })
    .to_string()
}
