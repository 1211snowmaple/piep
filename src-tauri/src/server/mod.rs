//! サーバーモード。棚を携帯端末へ出す HTTP 層と、デスクトップ版から単独サーバーへ棚を送る仕組み。
//!
//! 仕様は `docs/plan/server-mode.md`。デスクトップ版の内蔵サーバーと単独の
//! `piep-server` は、ここの同じ部品で動く。違うのは「棚をどこから借りるか」と
//! 「状態をどこに置くか」だけである。

pub mod admin_assets;
pub mod admin_store;
pub mod desktop;
pub mod http;
pub mod install;
#[cfg(windows)]
mod install_acl;
pub mod reader;
mod shelf_send;
pub mod standalone;
pub mod store;
pub mod sync_send;
#[cfg(test)]
mod tests;
pub mod tls;
pub mod update;
pub mod watchdog;

/// 携帯端末との契約の版。壊す変更のときだけ上げる。
pub const API_VERSION: u32 = 2;
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
