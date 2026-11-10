//! 端末内の読書位置と「続きを読む」。
//!
//! 以前は WebView の localStorage に置いていた。localStorage は書き込みが遅れ、
//! ホームへ戻ってすぐアプリを止めると最後の位置が消えた。ここでは呼び出しが
//! 返った時点で SQLite に確定している。
//!
//! 作品は取得元と取得元の ID で見分ける。サーバーの作品番号は接続先ごとに振られるので、
//! 棚から開くための接続先と番号は、最後に読んだときのものとして別に持つ。

use super::device_db::{DeviceDb, records_error};
use super::*;
use rusqlite::OptionalExtension;

const MAX_WORK_JSON: usize = 64 * 1024;
const MAX_POSITION_JSON: usize = 4 * 1024;
const MAX_SHELF: i64 = 200;
const MAX_IMPORT: usize = 5_000;

/// 1 作品の、端末での読書の記録。位置は `version`・`content_hash`・`edit_hash` の本文のもの。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalReading {
    source: String,
    source_id: String,
    version: i64,
    content_hash: Option<String>,
    edit_hash: Option<String>,
    page: i64,
    /// 画面の形を含む位置。中身は画面側が決める。
    position: Option<serde_json::Value>,
    progress: f64,
    finished: bool,
    /// 「続きを読む」に並べるか。開いただけの作品は並べない。
    shelved: bool,
    read_at: i64,
    server_url: Option<String>,
    work_id: Option<i64>,
    /// 棚に出すための作品情報。
    work: Option<serde_json::Value>,
}

fn key(source: &str, source_id: &str) -> String {
    format!("{source}:{source_id}")
}

fn valid_source(source: &str, source_id: &str) -> bool {
    matches!(source, "pixiv" | "fanbox")
        && !source_id.is_empty()
        && source_id.chars().count() <= 200
}

fn validate(reading: &LocalReading) -> Result<(), ClientError> {
    let json_len = |value: &Option<serde_json::Value>| {
        value.as_ref().map_or(0, |value| value.to_string().len())
    };
    if !valid_source(&reading.source, &reading.source_id)
        || !(0..=1_000_000).contains(&reading.page)
        || !(0.0..=1.0).contains(&reading.progress)
        || reading.read_at < 0
        || reading
            .content_hash
            .as_ref()
            .is_some_and(|hash| hash.len() > 128)
        || reading
            .edit_hash
            .as_ref()
            .is_some_and(|hash| hash.len() > 128)
        || reading
            .server_url
            .as_ref()
            .is_some_and(|url| url.len() > 2048)
        || reading
            .position
            .as_ref()
            .is_some_and(|value| !value.is_object())
        || json_len(&reading.position) > MAX_POSITION_JSON
        || reading
            .work
            .as_ref()
            .is_some_and(|value| !value.is_object())
        || json_len(&reading.work) > MAX_WORK_JSON
    {
        return Err(ClientError::remote());
    }
    Ok(())
}

fn row_to_reading(row: &rusqlite::Row<'_>) -> rusqlite::Result<LocalReading> {
    let json = |text: Option<String>| text.and_then(|text| serde_json::from_str(&text).ok());
    Ok(LocalReading {
        source: row.get("source")?,
        source_id: row.get("source_id")?,
        version: row.get("version")?,
        content_hash: row.get("content_hash")?,
        edit_hash: row.get("edit_hash")?,
        page: row.get("page")?,
        position: json(row.get("position")?),
        progress: row.get("progress")?,
        finished: row.get("finished")?,
        shelved: row.get("shelved")?,
        read_at: row.get("read_at")?,
        server_url: row.get("server_url")?,
        work_id: row.get("work_id")?,
        work: json(row.get("work")?),
    })
}

fn get(
    tx: &rusqlite::Transaction<'_>,
    source: &str,
    source_id: &str,
) -> Result<Option<LocalReading>, ClientError> {
    tx.query_row(
        "SELECT * FROM readings WHERE key = ?1",
        [key(source, source_id)],
        row_to_reading,
    )
    .optional()
    .map_err(records_error)
}

fn put(tx: &rusqlite::Transaction<'_>, reading: &LocalReading) -> Result<(), ClientError> {
    let text = |value: &Option<serde_json::Value>| value.as_ref().map(serde_json::Value::to_string);
    tx.execute(
        "INSERT INTO readings(key, source, source_id, version, content_hash, edit_hash, page, position,
             progress, finished, shelved, read_at, server_url, work_id, work)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
         ON CONFLICT(key) DO UPDATE SET version=excluded.version, content_hash=excluded.content_hash,
             edit_hash=excluded.edit_hash, page=excluded.page, position=excluded.position,
             progress=excluded.progress, finished=excluded.finished, shelved=excluded.shelved,
             read_at=excluded.read_at, server_url=excluded.server_url, work_id=excluded.work_id,
             work=excluded.work",
        rusqlite::params![
            key(&reading.source, &reading.source_id),
            reading.source,
            reading.source_id,
            reading.version,
            reading.content_hash,
            reading.edit_hash,
            reading.page,
            text(&reading.position),
            reading.progress,
            reading.finished,
            reading.shelved,
            reading.read_at,
            reading.server_url,
            reading.work_id,
            text(&reading.work),
        ],
    )
    .map_err(records_error)?;
    Ok(())
}

#[cfg(test)]
pub(super) fn shelf(
    tx: &rusqlite::Transaction<'_>,
    server_url: &str,
    limit: i64,
) -> Result<Vec<LocalReading>, ClientError> {
    shelf_by_status(tx, server_url, limit, false)
}

fn shelf_by_status(
    tx: &rusqlite::Transaction<'_>,
    server_url: &str,
    limit: i64,
    finished: bool,
) -> Result<Vec<LocalReading>, ClientError> {
    let mut query = tx
        .prepare(
            "SELECT * FROM readings WHERE server_url = ?1 AND shelved = 1 AND finished = ?3
             ORDER BY read_at DESC LIMIT ?2",
        )
        .map_err(records_error)?;
    let rows = query
        .query_map(
            rusqlite::params![server_url, limit.clamp(1, MAX_SHELF), finished],
            row_to_reading,
        )
        .map_err(records_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(records_error)
}

/// この作品の、端末での記録を返す。
#[tauri::command]
pub async fn reading_local_get(
    db: State<'_, Arc<DeviceDb>>,
    source: String,
    source_id: String,
) -> Result<Option<LocalReading>, ClientError> {
    if !valid_source(&source, &source_id) {
        return Err(ClientError::remote());
    }
    db.transaction(|tx| get(tx, &source, &source_id))
}

/// 記録を置き換える。返った時点で確定している。
#[tauri::command]
pub async fn reading_local_put(
    db: State<'_, Arc<DeviceDb>>,
    reading: LocalReading,
) -> Result<(), ClientError> {
    validate(&reading)?;
    db.transaction(|tx| put(tx, &reading))
}

/// 「続きを読む」。この接続先で読みかけの作品を、新しく読んだ順に返す。
#[tauri::command]
pub async fn reading_local_shelf(
    db: State<'_, Arc<DeviceDb>>,
    server_url: String,
    limit: i64,
    finished: Option<bool>,
) -> Result<Vec<LocalReading>, ClientError> {
    db.transaction(|tx| shelf_by_status(tx, &server_url, limit, finished.unwrap_or(false)))
}

/// localStorage に残っていた記録を移す。すでにある記録より古いものは捨てる。
/// 全件を一つの transaction で移すので、途中で失敗すれば何も変わらない。
#[tauri::command]
pub async fn reading_local_import(
    db: State<'_, Arc<DeviceDb>>,
    readings: Vec<LocalReading>,
) -> Result<usize, ClientError> {
    import(&db, readings)
}

pub(super) fn import(db: &DeviceDb, readings: Vec<LocalReading>) -> Result<usize, ClientError> {
    if readings.len() > MAX_IMPORT {
        return Err(ClientError::remote());
    }
    for reading in &readings {
        validate(reading)?;
    }
    db.transaction(|tx| {
        let mut imported = 0;
        for reading in &readings {
            let existing = get(tx, &reading.source, &reading.source_id)?;
            if existing.is_none_or(|old| old.read_at < reading.read_at) {
                put(tx, reading)?;
                imported += 1;
            }
        }
        Ok(imported)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pc_client::device_db::tests::TempDir;

    fn reading(source_id: &str, read_at: i64) -> LocalReading {
        LocalReading {
            source: "pixiv".into(),
            source_id: source_id.into(),
            version: 1,
            content_hash: Some("hash".into()),
            edit_hash: None,
            page: 2,
            position: Some(serde_json::json!({ "page": 2, "top": 120, "ratio": 0.4 })),
            progress: 0.3,
            finished: false,
            shelved: true,
            read_at,
            server_url: Some("https://pc.example/".into()),
            work_id: Some(7),
            work: Some(serde_json::json!({ "id": 7, "title": "作品" })),
        }
    }

    #[test]
    fn a_saved_reading_reads_back_exactly() {
        let dir = TempDir::new("local-roundtrip");
        let db = DeviceDb::new(&dir.0);
        let saved = reading("1", 10);
        db.transaction(|tx| put(tx, &saved)).unwrap();
        assert_eq!(
            db.transaction(|tx| get(tx, "pixiv", "1")).unwrap(),
            Some(saved)
        );
        assert_eq!(db.transaction(|tx| get(tx, "pixiv", "2")).unwrap(), None);
    }

    #[test]
    fn the_shelf_keeps_this_server_unfinished_shelved_works_newest_first() {
        let dir = TempDir::new("local-shelf");
        let db = DeviceDb::new(&dir.0);
        let mut other_server = reading("2", 30);
        other_server.server_url = Some("https://other.example/".into());
        let mut finished = reading("3", 40);
        finished.finished = true;
        let mut opened_only = reading("4", 50);
        opened_only.shelved = false;
        for item in [
            reading("1", 10),
            other_server,
            finished,
            opened_only,
            reading("5", 20),
        ] {
            db.transaction(|tx| put(tx, &item)).unwrap();
        }
        let shelf = db
            .transaction(|tx| shelf(tx, "https://pc.example/", 10))
            .unwrap();
        let ids: Vec<_> = shelf.iter().map(|item| item.source_id.as_str()).collect();
        assert_eq!(ids, ["5", "1"]);
        let finished = db
            .transaction(|tx| shelf_by_status(tx, "https://pc.example/", 10, true))
            .unwrap();
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].source_id, "3");
    }

    #[test]
    fn import_keeps_the_newer_record_and_rolls_back_on_a_bad_entry() {
        let dir = TempDir::new("local-import");
        let db = DeviceDb::new(&dir.0);
        db.transaction(|tx| put(tx, &reading("1", 100))).unwrap();
        let mut older = reading("1", 50);
        older.page = 9;
        assert_eq!(import(&db, vec![older, reading("2", 60)]).unwrap(), 1);
        assert_eq!(
            db.transaction(|tx| get(tx, "pixiv", "1"))
                .unwrap()
                .unwrap()
                .page,
            2
        );
        let mut bad = reading("3", 70);
        bad.progress = 2.0;
        assert!(import(&db, vec![reading("4", 80), bad]).is_err());
        assert_eq!(db.transaction(|tx| get(tx, "pixiv", "4")).unwrap(), None);
    }

    #[test]
    fn a_record_is_rejected_when_it_is_too_large_or_malformed() {
        let mut large = reading("1", 1);
        large.work = Some(serde_json::json!({ "title": "x".repeat(MAX_WORK_JSON) }));
        assert!(validate(&large).is_err());
        let mut source = reading("1", 1);
        source.source = "web".into();
        assert!(validate(&source).is_err());
        let mut position = reading("1", 1);
        position.position = Some(serde_json::json!([1, 2]));
        assert!(validate(&position).is_err());
    }
}
