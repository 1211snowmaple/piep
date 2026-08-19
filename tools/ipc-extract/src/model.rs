//! 抽出結果の形。
//!
//! ここで定義した構造がそのまま JSON になり、Markdown の描画側と
//! ドリフト検査が読む。描画と検査を別々に書けるよう、抽出器は
//! 判断をせず事実だけを載せる。

use serde::Serialize;

/// 一度の抽出でソースから読み取ったすべての事実。
#[derive(Serialize, Default)]
pub struct Contract {
    /// `#[tauri::command]` が付いた関数。
    pub commands: Vec<Command>,
    /// `generate_handler!` に列挙された名前。ここに無いコマンドは呼べない。
    pub registered: Vec<String>,
    /// `.emit("...")` で送出しているイベント名と、その位置。
    pub events: Vec<EventEmit>,
    /// `schema.rs` の `CREATE TABLE` から読み取ったテーブル。
    pub tables: Vec<Table>,
}

/// フロントから `invoke("名前")` で呼べる関数ひとつ。
#[derive(Serialize)]
pub struct Command {
    /// `invoke` に渡す名前。Rust の関数名がそのまま使われる。
    pub name: String,
    /// `commands/` の下のどのモジュールか。
    pub module: String,
    /// リポジトリ相対のパス。
    pub file: String,
    /// 関数の開始行。
    pub line: usize,
    /// `///` の中身。空なら `None`。
    pub doc: Option<String>,
    pub is_async: bool,
    pub args: Vec<Arg>,
    pub returns: Ret,
}

/// コマンドの引数ひとつ。
#[derive(Serialize)]
pub struct Arg {
    /// Rust 側の名前 (snake_case)。
    pub rust_name: String,
    /// フロントが渡すときの名前。Tauri 2 が camelCase へ変換する。
    pub js_name: String,
    pub ty: String,
    /// Tauri が注入する引数か。`AppHandle` や `State` はフロントから渡さない。
    pub injected: bool,
}

/// 戻り値。`Result<T, E>` は分解して持つ。
#[derive(Serialize)]
pub struct Ret {
    /// 書かれたままの型。
    pub raw: String,
    /// `Result` の成功側。`Result` でなければ `None`。
    pub ok: Option<String>,
    /// `Result` の失敗側。
    pub err: Option<String>,
}

/// イベントの送出箇所。
#[derive(Serialize)]
pub struct EventEmit {
    pub name: String,
    pub file: String,
    pub line: usize,
}

/// SQLite のテーブルひとつ。
#[derive(Serialize)]
pub struct Table {
    pub name: String,
    pub file: String,
    pub line: usize,
    pub columns: Vec<Column>,
}

/// テーブルの列ひとつ。
#[derive(Serialize)]
pub struct Column {
    pub name: String,
    /// `INTEGER NOT NULL` のような、名前を除いた残り全部。
    pub definition: String,
}
