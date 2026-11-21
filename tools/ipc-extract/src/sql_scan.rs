//! 棚の台帳（`library.db`）のテーブル定義を読む。
//!
//! 台帳のスキーマは `src-tauri/src/library/schema.rs` が `include_str!` で並べた SQL で、最初の
//! `schema.sql` のあとに移行が続く。移行には `ALTER TABLE` や作り直し（`DROP TABLE` のあと同じ名前で
//! `CREATE TABLE`）があるので、`CREATE TABLE` の文を拾うだけでは今の姿にならない。そこで、並んだ順に
//! 空のメモリ上のデータベースへ当て、出来上がった `sqlite_master` から定義を読む。SQLite は
//! `ALTER TABLE` のたびに保存した `CREATE TABLE` の文を書き換えるので、それが今の姿である。
//!
//! ここで読むのは「今のコードが新しく作る台帳」であって、利用者の手元の台帳の実際の姿ではない。

use crate::model::{Column, Table};
use std::path::{Path, PathBuf};

/// `schema_rs` が並べた SQL を順に当て、出来上がったテーブルを名前の順に返す。
///
/// 各テーブルの場所は、そのテーブルを最後に `CREATE TABLE` した SQL ファイルの行にする。
pub fn scan_tables(schema_rs: &Path, repo_root: &Path) -> anyhow::Result<Vec<Table>> {
    let files = migration_files(schema_rs)?;
    anyhow::ensure!(
        !files.is_empty(),
        "{} に include_str! の SQL が無い",
        schema_rs.display()
    );
    let mut sources = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file)
            .map_err(|e| anyhow::anyhow!("{} を読めない: {e}", file.display()))?;
        sources.push((file.clone(), text));
    }

    let conn = rusqlite::Connection::open_in_memory()?;
    for (file, text) in &sources {
        conn.execute_batch(text)
            .map_err(|e| anyhow::anyhow!("{} を当てられない: {e}", file.display()))?;
    }

    let mut stmt = conn.prepare(
        "SELECT name, sql FROM sqlite_master \
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND sql IS NOT NULL ORDER BY name",
    )?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;

    let mut out = Vec::new();
    for (name, sql) in rows {
        let sql = strip_comments(&sql);
        let Some(open) = sql.find('(') else { continue };
        let Some(close) = matching_paren(&sql, open) else {
            continue;
        };
        let (file, line) = created_at(&name, &sources)
            .map(|(file, line)| (crate::rust_scan::relative(file, repo_root), line))
            .unwrap_or_else(|| (crate::rust_scan::relative(schema_rs, repo_root), 1));
        out.push(Table {
            name,
            file,
            line,
            columns: parse_columns(&sql[open + 1..close]),
        });
    }
    Ok(out)
}

/// `schema_rs` の `include_str!("…")` を、書かれた順に `schema_rs` からの位置で返す。
fn migration_files(schema_rs: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let text = std::fs::read_to_string(schema_rs)?;
    let dir = schema_rs.parent().unwrap_or(Path::new("."));
    let re = regex::Regex::new(r#"include_str!\(\s*"([^"]+\.sql)"\s*\)"#)?;
    Ok(re.captures_iter(&text).map(|m| dir.join(&m[1])).collect())
}

/// テーブルを最後に `CREATE TABLE` したファイルと行。
fn created_at<'a>(name: &str, sources: &'a [(PathBuf, String)]) -> Option<(&'a Path, usize)> {
    let re = regex::Regex::new(&format!(
        r"(?i)CREATE\s+TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?{}\s*\(",
        regex::escape(name)
    ))
    .ok()?;
    sources.iter().rev().find_map(|(file, text)| {
        let found = re.find_iter(text).last()?;
        Some((file.as_path(), text[..found.start()].lines().count() + 1))
    })
}

/// `--` から行末までを落とす。保存された定義には、書いたときの注釈がそのまま残る。
/// 文字列の中の `--` は台帳のスキーマに無いので、区別しない。
fn strip_comments(sql: &str) -> String {
    sql.lines()
        .map(|line| match line.find("--") {
            Some(at) => &line[..at],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
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
            // `UNIQUE(a,b)` のように括弧を詰めて書いた表制約もあるので、先頭の語だけを見る。
            let head: String = p
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect::<String>()
                .to_uppercase();
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
