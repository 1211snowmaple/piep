//! 本棚の DB を機械の間で運ぶ形に直す。
//!
//! DB のパスは絶対パスで入っている。そのまま送ると相手の機械では開けず、
//! デスクトップ版の利用者名まで届く。送る前に `piep-library:/` の形へ直し、
//! 受け取った側で自分の置き場へ戻す。
//!
//! 実際の棚には `\\?\C:\...` の形や、`/` と `\` が混ざった形も入っている。
//! 文字列の前方一致では拾えないので、必ず [`to_portable`] を通す。

use rusqlite::Connection;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const PORTABLE_PREFIX: &str = "piep-library:/";

/// パスが入っている列。表を足したらここにも足す（テストで DB の定義と突き合わせる）。
pub const PATH_COLUMNS: &[(&str, &str)] = &[
    ("downloads", "cover_path"),
    ("downloads", "json_path"),
    ("downloads", "original_json_path"),
    ("assets", "local_path"),
    ("download_versions", "json_path"),
    ("download_versions", "original_json_path"),
    ("entity_versions", "json_path"),
    ("people", "icon_path"),
    ("people", "cover_path"),
    ("series", "cover_path"),
    ("work_collections", "cover_image_path"),
    ("saved_asset_audits", "json_path"),
    ("download_save_journal", "stage_path"),
    ("download_save_journal", "final_path"),
];

/// 読むのに要らない表。写しでは空にする。
///
/// 途中記録の表は、開くときの回復処理がそこに書かれたパスを触る。受け取る側でも
/// 必ず空にして、送られてきた DB の記録でサーバーのファイルを動かさせない。
pub const CLEARED_TABLES: &[&str] = &[
    "update_job_logs",
    "update_job_items",
    "update_jobs",
    "update_candidates",
    "watch_group_members",
    "watch_groups",
    "update_targets",
    "download_save_journal",
    "restore_journal",
    "restore_manifest_journal",
    "saved_asset_audits",
    "incomplete_save_repairs",
    "collection_suggestions",
    "collection_pair_feedback",
    "ai_notes",
    "saved_searches",
    "semantic_index_state",
    "search_index_state",
    "search_index_meta",
];

/// 送ってよい場所。全文索引や書き出しの置き場は含めない。
pub const FILE_ROOTS: &[&str] = &[
    "downloads/pixiv",
    "downloads/fanbox",
    "downloads/editor-assets",
    "profiles",
    "series",
    "collection-covers",
];

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PortableReport {
    pub converted: i64,
    /// アプリデータの外を指していて、消した値の数。
    pub outside: i64,
}

fn normalize(path: &str) -> String {
    let unified = path.replace('/', "\\");
    match unified.strip_prefix("\\\\?\\") {
        Some(rest) => rest.to_string(),
        None => unified,
    }
}

/// アプリデータの中のパスを `piep-library:/` の形に直す。外を指すなら `None`。
pub fn to_portable(path: &str, app_data: &str) -> Option<String> {
    let path = normalize(path);
    let root = normalize(app_data);
    let root = root.trim_end_matches('\\');
    let head = path.get(..root.len())?;
    if !head.eq_ignore_ascii_case(root) {
        return None;
    }
    let tail = path[root.len()..].strip_prefix('\\')?;
    if tail.is_empty() {
        return None;
    }
    Some(format!("{PORTABLE_PREFIX}{}", tail.replace('\\', "/")))
}

/// アプリデータの呼び方を全部挙げる。
///
/// 保存時のパスの多くは `canonicalize` を通っているので、ジャンクションや 8.3 形式の名前で
/// 渡されたアプリデータとは文字列が一致しない。実体の名前でも照らさないと、本棚の中のパスを
/// 外のものとして消してしまう。
fn library_roots(app_data: &Path) -> Vec<String> {
    let mut roots = vec![app_data.to_string_lossy().into_owned()];
    if let Ok(real) = app_data.canonicalize() {
        let real = real.to_string_lossy().into_owned();
        if !normalize(&real).eq_ignore_ascii_case(&normalize(&roots[0])) {
            roots.push(real);
        }
    }
    roots
}

/// 相対パスを検査する。`..`、ドライブ、送ってよい場所の外を断る。
pub fn validate_relative(relative: &str) -> Result<(), String> {
    if relative.is_empty()
        || relative.len() > 1024
        || relative.contains('\\')
        || relative.contains(':')
        || relative.contains('\0')
    {
        return Err("正しくないパスです".into());
    }
    if relative.split('/').any(|part| {
        part.is_empty() || part == "." || part == ".." || part.ends_with('.') || part.ends_with(' ')
    }) {
        return Err("正しくないパスです".into());
    }
    let allowed = FILE_ROOTS.iter().any(|root| {
        relative
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
    });
    if !allowed {
        return Err("送れない場所のパスです".into());
    }
    Ok(())
}

/// `piep-library:/` の形を、この機械の置き場のパスへ戻す。
pub fn from_portable(value: &str, library_root: &Path) -> Result<PathBuf, String> {
    let relative = value
        .strip_prefix(PORTABLE_PREFIX)
        .ok_or("相対の形ではないパスです")?;
    validate_relative(relative)?;
    let mut path = library_root.to_path_buf();
    path.extend(relative.split('/'));
    Ok(path)
}

fn sql_error(error: rusqlite::Error) -> String {
    format!("写しの DB を直せません: {error}")
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count > 0)
    .map_err(sql_error)
}

fn clear_tables(conn: &Connection) -> Result<(), String> {
    for table in CLEARED_TABLES {
        if table_exists(conn, table)? {
            conn.execute(&format!("DELETE FROM \"{table}\""), [])
                .map_err(sql_error)?;
        }
    }
    Ok(())
}

fn column_is_required(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .map_err(sql_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(3)?))
        })
        .map_err(sql_error)?;
    for row in rows {
        let (name, not_null) = row.map_err(sql_error)?;
        if name == column {
            return Ok(not_null != 0);
        }
    }
    Err(format!("{table}.{column} がありません"))
}

/// 列の値を書き換える。`convert` が `None` を返した値は空にする。
fn rewrite_column<F>(
    conn: &Connection,
    table: &str,
    column: &str,
    mut convert: F,
) -> Result<(i64, i64), String>
where
    F: FnMut(&str) -> Result<Option<String>, String>,
{
    let required = column_is_required(conn, table, column)?;
    let rows: Vec<(i64, String)> = {
        let mut stmt = conn
            .prepare(&format!(
                "SELECT rowid, \"{column}\" FROM \"{table}\" WHERE \"{column}\" IS NOT NULL AND \"{column}\" <> ''"
            ))
            .map_err(sql_error)?;
        let mapped = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(sql_error)?;
        mapped.collect::<Result<_, _>>().map_err(sql_error)?
    };
    let mut update = conn
        .prepare(&format!(
            "UPDATE \"{table}\" SET \"{column}\" = ?1 WHERE rowid = ?2"
        ))
        .map_err(sql_error)?;
    let (mut converted, mut dropped) = (0, 0);
    for (rowid, value) in rows {
        match convert(&value)? {
            Some(next) => {
                update
                    .execute(rusqlite::params![next, rowid])
                    .map_err(sql_error)?;
                converted += 1;
            }
            None => {
                let blank: Option<&str> = required.then_some("");
                update
                    .execute(rusqlite::params![blank, rowid])
                    .map_err(sql_error)?;
                dropped += 1;
            }
        }
    }
    Ok((converted, dropped))
}

/// デスクトップ版の側で、`VACUUM INTO` した写しを送る形に直す。
pub fn prepare_for_send(copy: &Path, app_data: &Path) -> Result<PortableReport, String> {
    let roots = library_roots(app_data);
    let mut conn = Connection::open(copy).map_err(sql_error)?;
    conn.pragma_update(None, "foreign_keys", true)
        .map_err(sql_error)?;
    let tx = conn.transaction().map_err(sql_error)?;
    clear_tables(&tx)?;
    if table_exists(&tx, "search_feature_settings")? {
        tx.execute(
            "UPDATE search_feature_settings SET semantic_enabled = 0",
            [],
        )
        .map_err(sql_error)?;
    }
    let mut report = PortableReport::default();
    for (table, column) in PATH_COLUMNS {
        if !table_exists(&tx, table)? {
            continue;
        }
        let (converted, outside) = rewrite_column(&tx, table, column, |value| {
            Ok(roots.iter().find_map(|root| to_portable(value, root)))
        })?;
        report.converted += converted;
        report.outside += outside;
    }
    tx.commit().map_err(sql_error)?;
    // 消した行の中身をファイルに残さない。
    conn.execute_batch("VACUUM;").map_err(sql_error)?;
    Ok(report)
}

fn table_columns(conn: &Connection, schema: &str, table: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA \"{schema}\".table_info(\"{table}\")"))
        .map_err(sql_error)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(sql_error)?;
    rows.collect::<Result<_, _>>().map_err(sql_error)
}

/// 直前の世代の索引の記録を引き継ぐ。
///
/// 記録は作品ごとに「版＋本文ハッシュ」なので、引き継げば変わった作品だけが
/// 「未索引」になる。ID が同じでも別の作品になっていれば引き継がない。
fn carry_index_state(conn: &Connection, previous: &Path) -> Result<(), String> {
    conn.execute(
        "ATTACH DATABASE ?1 AS previous",
        [previous.to_string_lossy().as_ref()],
    )
    .map_err(sql_error)?;
    let result = (|| -> Result<(), String> {
        for (table, join) in [
            (
                "search_index_state",
                " JOIN previous.downloads pd ON pd.id = p.download_id
                  JOIN main.downloads nd ON nd.id = p.download_id
                   AND nd.source = pd.source AND nd.source_id = pd.source_id",
            ),
            ("search_index_meta", ""),
        ] {
            let current = table_columns(conn, "main", table)?;
            let earlier: HashSet<String> = table_columns(conn, "previous", table)?
                .into_iter()
                .collect();
            let shared: Vec<&String> = current
                .iter()
                .filter(|name| earlier.contains(*name))
                .collect();
            if shared.is_empty() {
                continue;
            }
            let target = shared
                .iter()
                .map(|name| format!("\"{name}\""))
                .collect::<Vec<_>>()
                .join(", ");
            let select = shared
                .iter()
                .map(|name| format!("p.\"{name}\""))
                .collect::<Vec<_>>()
                .join(", ");
            conn.execute(
                &format!("INSERT OR REPLACE INTO main.\"{table}\" ({target}) SELECT {select} FROM previous.\"{table}\" p{join}"),
                [],
            )
            .map_err(sql_error)?;
        }
        Ok(())
    })();
    conn.execute_batch("DETACH DATABASE previous;")
        .map_err(sql_error)?;
    result
}

/// サーバーの側で、受け取った写しを自分の置き場に合わせて直す。
///
/// 管理者の鍵が漏れた場合も考え、送られてきた DB を信用しない。パスの形が正しく
/// なければ全体を断る。途中記録の表も空にする。
pub fn restore_on_server(
    incoming: &Path,
    library_root: &Path,
    previous: Option<&Path>,
) -> Result<(), String> {
    let mut conn = Connection::open(incoming).map_err(sql_error)?;
    conn.pragma_update(None, "trusted_schema", false)
        .map_err(sql_error)?;
    conn.pragma_update(None, "foreign_keys", true)
        .map_err(sql_error)?;
    let check: String = conn
        .query_row("PRAGMA quick_check(1)", [], |row| row.get(0))
        .map_err(sql_error)?;
    if check != "ok" {
        return Err("受け取った DB が壊れています".into());
    }
    if !table_exists(&conn, "downloads")?
        || !crate::database::schema::is_known_library(&conn).map_err(sql_error)?
    {
        return Err("受け取ったファイルは piep の本棚ではありません".into());
    }
    {
        // 送られてきた DB が持つトリガーをサーバーの中で動かさない。必要なものは
        // 下の schema::initialize が作り直す。
        let triggers: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type = 'trigger'")
                .map_err(sql_error)?;
            let rows = stmt.query_map([], |row| row.get(0)).map_err(sql_error)?;
            rows.collect::<Result<_, _>>().map_err(sql_error)?
        };
        for trigger in triggers {
            conn.execute(
                &format!("DROP TRIGGER \"{}\"", trigger.replace('"', "\"\"")),
                [],
            )
            .map_err(sql_error)?;
        }
        // 実際に本棚として開けるかも、公開する前に確かめる。SQLite の検査と
        // downloads の存在だけでは、不適合な表や旧形式を通してしまう。
        // 識別済みの本棚だけを初期化するので、foreign DB のリセットは起きない。
        crate::database::schema::initialize(&conn).map_err(sql_error)?;
        crate::database::schema::validate_columns(&conn).map_err(sql_error)?;
        let tx = conn.transaction().map_err(sql_error)?;
        clear_tables(&tx)?;
        for (table, column) in PATH_COLUMNS {
            if !table_exists(&tx, table)? {
                continue;
            }
            rewrite_column(&tx, table, column, |value| {
                from_portable(value, library_root)
                    .map(|path| Some(path.to_string_lossy().into_owned()))
                    .map_err(|error| format!("{table}.{column}: {error}"))
            })?;
        }
        tx.commit().map_err(sql_error)?;
    }
    if let Some(previous) = previous.filter(|path| path.exists()) {
        carry_index_state(&conn, previous)?;
    }
    // 別の名前に動かすので、WAL を残さない形で閉じる。
    conn.pragma_update(None, "journal_mode", "DELETE")
        .map_err(sql_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = r"C:\Users\reader\AppData\Roaming\piep";

    #[test]
    fn received_databases_must_be_readable_without_resetting_the_library() {
        for damage in [
            "ALTER TABLE downloads ADD COLUMN tags TEXT",
            "DROP TABLE work_edit_blocks; CREATE TABLE work_edit_blocks (wrong_column TEXT)",
        ] {
            let dir = crate::server::tests::TestRoot::created("snapshot_schema");
            let incoming = dir.0.join("incoming.db");
            let conn = Connection::open(&incoming).unwrap();
            crate::database::schema::initialize(&conn).unwrap();
            conn.execute_batch(damage).unwrap();
            drop(conn);
            assert!(
                restore_on_server(&incoming, &dir.0.join("library"), None).is_err(),
                "本棚として開けない写しは確定前に断る: {damage}"
            );
        }
    }

    #[test]
    fn every_shape_found_in_a_real_library_is_converted() {
        for (input, expected) in [
            (
                format!(r"{APP}\downloads\pixiv\1\v1\original.json"),
                "downloads/pixiv/1/v1/original.json",
            ),
            (
                format!(r"\\?\{APP}\downloads\pixiv\1\v1\a.jpg"),
                "downloads/pixiv/1/v1/a.jpg",
            ),
            (
                r"C:/Users/reader/AppData/Roaming/piep\series\pixiv\9\cover.jpg".to_string(),
                "series/pixiv/9/cover.jpg",
            ),
            (
                r"c:\users\READER\AppData\Roaming\piep\profiles\x.png".to_string(),
                "profiles/x.png",
            ),
        ] {
            assert_eq!(
                to_portable(&input, APP).as_deref(),
                Some(format!("{PORTABLE_PREFIX}{expected}").as_str()),
                "{input}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_library_reached_through_a_junction_keeps_its_real_paths() {
        let base = std::env::temp_dir().join(format!(
            "piep_roots_{}",
            super::super::store::random_token(9)
        ));
        let real = base.join("real");
        let junction = base.join("junction");
        std::fs::create_dir_all(&real).unwrap();
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&real)
            .output()
            .unwrap();
        assert!(made.status.success(), "ジャンクションを作れません");

        let roots = library_roots(&junction);
        assert_eq!(roots.len(), 2, "実体の名前も挙げる");
        let stored = real
            .canonicalize()
            .unwrap()
            .join("downloads")
            .join("cover.png");
        let portable = roots
            .iter()
            .find_map(|root| to_portable(&stored.to_string_lossy(), root));
        assert_eq!(
            portable.as_deref(),
            Some("piep-library:/downloads/cover.png")
        );

        std::fs::remove_dir(&junction).unwrap();
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn paths_outside_the_library_are_not_portable() {
        for input in [
            r"D:\other\a.jpg",
            r"C:\Users\reader\AppData\Roaming\piep-other\a.jpg",
            APP,
        ] {
            assert_eq!(to_portable(input, APP), None, "{input}");
        }
    }

    #[test]
    fn hostile_relative_paths_are_refused() {
        for input in [
            "downloads/pixiv/../../secret",
            "../secret",
            "downloads/search-index/x",
            "piep.db",
            "downloads/pixiv",
            r"downloads\pixiv\a",
            "downloads/pixiv/C:/a",
            "downloads/pixiv//a",
            "downloads/pixiv/a.",
            "",
        ] {
            assert!(validate_relative(input).is_err(), "{input}");
            assert!(
                from_portable(&format!("{PORTABLE_PREFIX}{input}"), Path::new(r"D:\lib")).is_err(),
                "{input}"
            );
        }
        assert!(from_portable(r"C:\anything", Path::new(r"D:\lib")).is_err());
        assert!(validate_relative("downloads/pixiv/1/v1/original.json").is_ok());
        assert!(validate_relative("collection-covers/a.png").is_ok());
    }

    /// 列の一覧と DB の定義を突き合わせる。パスの列を足したのにここを忘れると、
    /// 利用者名が相手の機械へ届くので、形で見つけて失敗させる。
    #[test]
    fn path_column_list_matches_the_schema() {
        let dir = std::env::temp_dir().join(format!(
            "piep_snapshot_{}",
            super::super::store::random_token(8)
        ));
        std::fs::create_dir_all(dir.join("downloads")).unwrap();
        let db_path = dir.join("piep.db");
        let db = crate::database::Database::open(&db_path, &dir.join("downloads")).unwrap();
        drop(db);
        let conn = Connection::open(&db_path).unwrap();
        let tables: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let mut found = Vec::new();
        for table in &tables {
            for column in table_columns(&conn, "main", table).unwrap() {
                if column.ends_with("_path") {
                    found.push((table.clone(), column));
                }
            }
        }
        for pair in &found {
            assert!(
                PATH_COLUMNS
                    .iter()
                    .any(|(table, column)| table == &pair.0 && column == &pair.1),
                "{pair:?} が PATH_COLUMNS にありません"
            );
        }
        // 復元の記録は、復元を始めるときに作られる。新しい DB にはまだ無い。
        const MADE_ON_DEMAND: &[&str] = &["restore_journal", "restore_manifest_journal"];
        for table in CLEARED_TABLES
            .iter()
            .filter(|table| !MADE_ON_DEMAND.contains(table))
        {
            assert!(
                tables.iter().any(|name| name == table),
                "{table} は DB にありません"
            );
        }
        drop(conn);
        crate::database::tantivy_index::release_runtime(&dir.join("downloads"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
