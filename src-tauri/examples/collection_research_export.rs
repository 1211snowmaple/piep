//! 現行のコレクション題名ルールを、読み取り専用で調査に渡す。
//!
//! cargo run --example collection_research_export -- <piep.db>
//! echo '["架空作品 第1話", "架空作品 第2話"]' | cargo run --example collection_research_export -- --titles
//!
//! SQLiteは読み取り専用で開き、アプリのDB初期化処理は呼ばない。

use std::io::{self, Read, Write};
use std::path::Path;

use piep_lib::database::collection_rules;
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;

#[derive(Serialize)]
struct RuleSignals {
    display_title_stem: String,
    family_match_key: String,
    episode_order: Option<i64>,
    has_ordinal_marker: bool,
    edition_match_key: String,
    is_administrative_post: bool,
}

impl RuleSignals {
    fn for_title(title: &str, content_type: &str, text_length: i64) -> Self {
        Self {
            display_title_stem: collection_rules::display_title_stem(title),
            family_match_key: collection_rules::family_match_key(title),
            episode_order: collection_rules::episode_order(title),
            has_ordinal_marker: collection_rules::has_ordinal_marker(title),
            edition_match_key: collection_rules::edition_match_key(title),
            is_administrative_post: collection_rules::is_administrative_post(
                title,
                content_type,
                text_length,
            ),
        }
    }
}

#[derive(Serialize)]
struct TitleExport {
    title: String,
    #[serde(flatten)]
    rules: RuleSignals,
}

#[derive(Serialize)]
struct WorkExport {
    id: i64,
    source: String,
    source_id: String,
    title: String,
    author_name: String,
    author_id: String,
    normalized_author_key: String,
    content_type: String,
    text_length: i64,
    published_at: String,
    #[serde(flatten)]
    rules: RuleSignals,
}

fn emit_json(value: &impl Serialize) -> Result<(), Box<dyn std::error::Error>> {
    let stdout = io::stdout();
    let mut output = io::BufWriter::new(stdout.lock());
    serde_json::to_writer(&mut output, value)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 1 {
        return Err("usage: collection_research_export <piep.db> | --titles".into());
    }
    if args[0] == "--titles" {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input)?;
        // 本文を持たない合成例も、小説として題名ルールを評価する。
        let titles: Vec<String> = serde_json::from_str(&input)?;
        let result = titles
            .into_iter()
            .map(|title| TitleExport {
                rules: RuleSignals::for_title(&title, "novel", 0),
                title,
            })
            .collect::<Vec<_>>();
        return emit_json(&result);
    }

    let conn = Connection::open_with_flags(Path::new(&args[0]), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut query = conn.prepare(
        "SELECT id, source, source_id, title, author_name, COALESCE(author_id, ''),
                content_type, text_length, COALESCE(source_created_at, downloaded_at, '')
         FROM downloads ORDER BY id",
    )?;
    let rows = query.query_map([], |row| {
        let title: String = row.get(3)?;
        let content_type: String = row.get(6)?;
        let text_length: i64 = row.get(7)?;
        let author_name: String = row.get(4)?;
        let author_id: String = row.get(5)?;
        let source: String = row.get(1)?;
        let normalized_author_key = if author_name.trim().is_empty() {
            format!("{source}:{author_id}")
        } else {
            piep_lib::database::search::normalize_search_text(author_name.trim())
        };
        let rules = RuleSignals::for_title(&title, &content_type, text_length);
        Ok(WorkExport {
            id: row.get(0)?,
            source,
            source_id: row.get(2)?,
            title,
            author_name,
            author_id,
            normalized_author_key,
            content_type,
            text_length,
            published_at: row.get(8)?,
            rules,
        })
    })?;
    let result = rows.collect::<Result<Vec<_>, _>>()?;
    emit_json(&result)
}
