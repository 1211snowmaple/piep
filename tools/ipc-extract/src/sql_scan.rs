//! `schema.rs` の中の SQL からテーブル定義を読む。
//!
//! スキーマは Rust の文字列リテラルとして書かれているので、構文木を辿っても
//! 中身は一つの文字列にしかならない。`CREATE TABLE` を直接探すほうが素直である。
//!
//! ここで読むのは「今のコードが作るテーブル」であって、利用者の手元にある
//! データベースの実際の姿ではない。移行の履歴は追わない。

use crate::model::{Column, Table};
use std::path::Path;
use syn::spanned::Spanned;

/// `CREATE TABLE [IF NOT EXISTS] 名前 (` を探し、対応する `)` までを列として読む。
///
/// `#[cfg(test)]` の中は見ない。移行の試験は「旧番号のライブラリ」を再現するため
/// 意図的に古い形のテーブルを作っており、それを現行スキーマとして載せると嘘になる。
pub fn scan_tables(schema_rs: &Path, repo_root: &Path) -> anyhow::Result<Vec<Table>> {
    let text = std::fs::read_to_string(schema_rs)?;
    let rel = crate::rust_scan::relative(schema_rs, repo_root);
    let skip = test_line_ranges(&text);
    let re = regex::Regex::new(
        r"(?i)CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*\(",
    )?;

    let mut out = Vec::new();
    for m in re.captures_iter(&text) {
        let whole = m.get(0).unwrap();
        let line = text[..whole.start()].lines().count() + 1;
        if skip.iter().any(|(s, e)| line >= *s && line <= *e) {
            continue;
        }
        let name = m[1].to_string();
        let open = whole.end() - 1; // `(` の位置
        let Some(close) = matching_paren(&text, open) else {
            continue;
        };
        let body = &text[open + 1..close];
        out.push(Table {
            name,
            file: rel.clone(),
            line,
            columns: parse_columns(body),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// `#[cfg(test)]` が付いた項目が占める行の範囲。
fn test_line_ranges(text: &str) -> Vec<(usize, usize)> {
    let Ok(file) = syn::parse_file(text) else {
        return Vec::new();
    };
    file.items
        .iter()
        .filter(|item| {
            let attrs: &[syn::Attribute] = match item {
                syn::Item::Mod(m) => &m.attrs,
                syn::Item::Fn(f) => &f.attrs,
                _ => &[],
            };
            attrs.iter().any(is_cfg_test)
        })
        .map(|item| {
            let span = item.span();
            (span.start().line, span.end().line)
        })
        .collect()
}

/// `#[cfg(test)]` かどうか。`#[cfg(feature = "x")]` などは対象外。
fn is_cfg_test(attr: &syn::Attribute) -> bool {
    if !attr.path().is_ident("cfg") {
        return false;
    }
    let mut found = false;
    let _ = attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("test") {
            found = true;
        }
        Ok(())
    });
    found
}

/// `open` にある `(` に対応する `)` の位置。見つからなければ `None`。
fn matching_paren(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in text[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// 定義本体をカンマで割って列にする。
///
/// 括弧の内側のカンマ（`FOREIGN KEY (a, b)` や `CHECK (x IN (1, 2))`）では
/// 割らない。`PRIMARY KEY (...)` のような表制約は列ではないので落とす。
fn parse_columns(body: &str) -> Vec<Column> {
    let mut parts: Vec<String> = Vec::new();
    let mut depth = 0usize;
    let mut cur = String::new();
    for c in body.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                cur.push(c);
            }
            ',' if depth == 0 => {
                parts.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    parts.push(cur);

    const CONSTRAINTS: &[&str] = &[
        "PRIMARY",
        "FOREIGN",
        "UNIQUE",
        "CHECK",
        "CONSTRAINT",
        "INDEX",
    ];

    parts
        .iter()
        .map(|p| collapse(p))
        .filter(|p| !p.is_empty())
        .filter(|p| {
            let head = p.split_whitespace().next().unwrap_or("").to_uppercase();
            !CONSTRAINTS.contains(&head.as_str())
        })
        .filter_map(|p| {
            let mut it = p.splitn(2, char::is_whitespace);
            let name = it.next()?.to_string();
            let definition = it.next().unwrap_or("").trim().to_string();
            Some(Column { name, definition })
        })
        .collect()
}

/// 改行と連続する空白を一つの空白へ畳む。SQL は桁を揃えて書かれている。
fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
