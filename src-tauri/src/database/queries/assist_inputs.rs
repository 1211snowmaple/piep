//! モデルに渡す材料を、棚から組み立てる。
//!
//! ここが**送るものと送らないものの境目**である。本文を含む材料を作る関数は
//! 名前に `with_body` と付けてあり、呼び出し側は利用者の許可を確かめてから
//! しか呼べない（許可の確認は `assist` 側でも二重にやる）。

/// 出どころの付いたタグ。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaggedName {
    pub name: String,
    /// `origin`（取得元）／`manual`（利用者）／`llm`（モデルの案を採った）
    pub source: String,
}

/// モデルに書いてもらった覚え書き。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiNote {
    pub text: String,
    #[serde(default)]
    pub source_tail: Option<String>,
    #[serde(default)]
    pub claims: Option<Vec<crate::assist::GroundedClaim>>,
    /// どのモデルが書いたか。モデルを替えたときに古い文が混ざるのを防ぐ。
    pub model_id: String,
    pub feature_id: String,
    pub prompt_version: String,
    pub input_fingerprint: String,
    pub config_fingerprint: String,
    pub created_at: String,
    /// 固定promptの版が現在と異なる。入力・設定のstale判定には各fingerprintを使う。
    pub prompt_stale: bool,
    /// Title, tags, body, or other feature input changed after generation.
    pub input_stale: bool,
    /// None when settings were not supplied; otherwise whether the model or
    /// feature-specific settings differ from those used for generation.
    pub config_stale: Option<bool>,
}
