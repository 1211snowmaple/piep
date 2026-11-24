//! 端末内の読書位置と「続きを読む」。
//!
//! 呼び出しが返った時点で SQLite に確定している。WebView の localStorage は書き込みが遅れ、
//! ホームへ戻ってすぐアプリを止めると最後の位置が消えるので使わない。
//!
//! 記録は棚・利用者・作品の UUID で見分け、位置は保存版と編集の UUID に結ぶ。

use super::device_db::{DeviceDb, records_error};
use super::shelf::ShelfScope;
use super::*;
use rusqlite::OptionalExtension;
use uuid::Uuid;

const MAX_WORK_JSON: usize = 64 * 1024;
const MAX_POSITION_JSON: usize = 4 * 1024;
const MAX_SHELF: i64 = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadingScope {
    library_id: String,
    subject_id: String,
}

impl From<ShelfScope> for ReadingScope {
    fn from(scope: ShelfScope) -> Self {
        Self {
            library_id: scope.library_id().get().to_string(),
            subject_id: scope.subject_id().get().to_string(),
        }
    }
}

impl ReadingScope {
    pub(super) fn valid(&self) -> bool {
        valid_id(&self.library_id) && valid_id(&self.subject_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadingKey {
    scope: ReadingScope,
    work_id: String,
}

impl ReadingKey {
    fn valid(&self) -> bool {
        self.scope.valid() && valid_id(&self.work_id)
    }
}

/// 1 作品の、端末での読書の記録。位置は `snapshot_id`・`edit_id` の本文のもの。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalReading {
    key: ReadingKey,
    snapshot_id: String,
    edit_id: Option<String>,
    page: i64,
    /// 画面の形を含む位置。中身は画面側が決める。
    position: Option<serde_json::Value>,
    progress: f64,
    finished: bool,
    /// 「続きを読む」に並べるか。開いただけの作品は並べない。
    shelved: bool,
    read_at: i64,
    /// 棚に出すための作品情報。
    work: Option<serde_json::Value>,
}

fn valid_id(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}

fn validate(reading: &LocalReading) -> Result<(), ClientError> {
    let json_len = |value: &Option<serde_json::Value>| {
        value.as_ref().map_or(0, |value| value.to_string().len())
    };
    if !reading.key.valid()
        || !valid_id(&reading.snapshot_id)
        || reading.edit_id.as_deref().is_some_and(|id| !valid_id(id))
        || !(0..=1_000_000).contains(&reading.page)
        || !(0.0..=1.0).contains(&reading.progress)
        || reading.read_at < 0
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
        key: ReadingKey {
            scope: ReadingScope {
                library_id: row.get("library_id")?,
                subject_id: row.get("subject_id")?,
            },
            work_id: row.get("work_id")?,
        },
        snapshot_id: row.get("snapshot_id")?,
        edit_id: row.get("edit_id")?,
        page: row.get("page")?,
        position: json(row.get("position")?),
        progress: row.get("progress")?,
        finished: row.get("finished")?,
        shelved: row.get("shelved")?,
        read_at: row.get("read_at")?,
        work: json(row.get("work")?),
    })
}

fn get(
    tx: &rusqlite::Transaction<'_>,
    key: &ReadingKey,
) -> Result<Option<LocalReading>, ClientError> {
    tx.query_row(
        "SELECT * FROM readings WHERE library_id = ?1 AND subject_id = ?2 AND work_id = ?3",
        rusqlite::params![key.scope.library_id, key.scope.subject_id, key.work_id],
        row_to_reading,
    )
    .optional()
    .map_err(records_error)
}

fn put(tx: &rusqlite::Transaction<'_>, reading: &LocalReading) -> Result<(), ClientError> {
    let text = |value: &Option<serde_json::Value>| value.as_ref().map(serde_json::Value::to_string);
    tx.execute(
        "INSERT INTO readings(library_id, subject_id, work_id, snapshot_id, edit_id, page, position,
             progress, finished, shelved, read_at, work)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(library_id, subject_id, work_id) DO UPDATE SET snapshot_id=excluded.snapshot_id,
             edit_id=excluded.edit_id, page=excluded.page, position=excluded.position,
             progress=excluded.progress, finished=excluded.finished, shelved=excluded.shelved,
             read_at=excluded.read_at, work=excluded.work",
        rusqlite::params![
            reading.key.scope.library_id,
            reading.key.scope.subject_id,
            reading.key.work_id,
            reading.snapshot_id,
            reading.edit_id,
            reading.page,
            text(&reading.position),
            reading.progress,
            reading.finished,
            reading.shelved,
            reading.read_at,
            text(&reading.work),
        ],
    )
    .map_err(records_error)?;
    Ok(())
}

fn shelf_by_status(
    tx: &rusqlite::Transaction<'_>,
    scope: &ReadingScope,
    limit: i64,
    finished: bool,
) -> Result<Vec<LocalReading>, ClientError> {
    let mut query = tx
        .prepare(
            "SELECT * FROM readings
             WHERE library_id = ?1 AND subject_id = ?2 AND shelved = 1 AND finished = ?3
             ORDER BY read_at DESC LIMIT ?4",
        )
        .map_err(records_error)?;
    let rows = query
        .query_map(
            rusqlite::params![
                scope.library_id,
                scope.subject_id,
                finished,
                limit.clamp(1, MAX_SHELF)
            ],
            row_to_reading,
        )
        .map_err(records_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(records_error)
}

/// この作品の、端末での記録を返す。
#[tauri::command]
pub async fn reading_local_get(
    db: State<'_, Arc<DeviceDb>>,
    key: ReadingKey,
) -> Result<Option<LocalReading>, ClientError> {
    if !key.valid() {
        return Err(ClientError::remote());
    }
    db.transaction(|tx| get(tx, &key))
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

/// 「続きを読む」。この棚と利用者の記録を、新しく読んだ順に返す。
#[tauri::command]
pub async fn reading_local_shelf(
    db: State<'_, Arc<DeviceDb>>,
    scope: ReadingScope,
    limit: i64,
    finished: Option<bool>,
) -> Result<Vec<LocalReading>, ClientError> {
    if !scope.valid() {
        return Err(ClientError::remote());
    }
    db.transaction(|tx| shelf_by_status(tx, &scope, limit, finished.unwrap_or(false)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pc_client::device_db::tests::TempDir;

    fn id(number: u128) -> String {
        Uuid::from_u128(number).to_string()
    }

    fn reading(work_id: u128, read_at: i64) -> LocalReading {
        LocalReading {
            key: ReadingKey {
                scope: ReadingScope {
                    library_id: id(10),
                    subject_id: id(11),
                },
                work_id: id(work_id),
            },
            snapshot_id: id(12),
            edit_id: None,
            page: 2,
            position: Some(serde_json::json!({ "page": 2, "top": 120, "ratio": 0.4 })),
            progress: 0.3,
            finished: false,
            shelved: true,
            read_at,
            work: Some(
                serde_json::json!({ "id": id(work_id), "source": "pixiv", "sourceId": "100", "title": "作品" }),
            ),
        }
    }

    #[test]
    fn local_reading_keeps_same_source_on_two_shelves() {
        let db = DeviceDb::in_memory();
        let first = reading(1, 10);
        let mut second = reading(1, 20);
        second.key.scope.library_id = id(20);
        second.snapshot_id = id(21);
        second.page = 5;
        let mut other_reader = reading(1, 30);
        other_reader.key.scope.subject_id = id(30);
        other_reader.page = 7;
        db.transaction(|tx| put(tx, &first)).unwrap();
        db.transaction(|tx| put(tx, &second)).unwrap();
        db.transaction(|tx| put(tx, &other_reader)).unwrap();
        for reading in [first, second, other_reader] {
            assert_eq!(
                db.transaction(|tx| get(tx, &reading.key)).unwrap().as_ref(),
                Some(&reading)
            );
            assert_eq!(
                db.transaction(|tx| shelf_by_status(tx, &reading.key.scope, 10, false))
                    .unwrap(),
                vec![reading]
            );
        }
    }

    #[test]
    fn a_saved_reading_reads_back_exactly() {
        let dir = TempDir::new("local-roundtrip");
        let db = DeviceDb::new(&dir.0);
        let mut saved = reading(1, 10);
        saved.edit_id = Some(id(13));
        db.transaction(|tx| put(tx, &saved)).unwrap();
        assert_eq!(
            db.transaction(|tx| get(tx, &saved.key)).unwrap(),
            Some(saved)
        );
        assert_eq!(
            db.transaction(|tx| get(tx, &reading(2, 0).key)).unwrap(),
            None
        );
    }

    #[test]
    fn the_shelf_keeps_unfinished_shelved_works_newest_first() {
        let dir = TempDir::new("local-shelf");
        let db = DeviceDb::new(&dir.0);
        let first = reading(1, 10);
        let scope = first.key.scope.clone();
        let mut finished = reading(3, 40);
        finished.finished = true;
        let mut opened_only = reading(4, 50);
        opened_only.shelved = false;
        for item in [first, finished, opened_only, reading(5, 20)] {
            db.transaction(|tx| put(tx, &item)).unwrap();
        }
        let shelf = db
            .transaction(|tx| shelf_by_status(tx, &scope, 10, false))
            .unwrap();
        let ids: Vec<_> = shelf.into_iter().map(|item| item.key.work_id).collect();
        assert_eq!(ids, [id(5), id(1)]);
        let finished = db
            .transaction(|tx| shelf_by_status(tx, &scope, 10, true))
            .unwrap();
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].key.work_id, id(3));
    }

    #[test]
    fn a_record_is_rejected_when_it_is_too_large_or_malformed() {
        let mut large = reading(1, 1);
        large.work = Some(serde_json::json!({ "title": "x".repeat(MAX_WORK_JSON) }));
        assert!(validate(&large).is_err());
        let mut invalid = reading(1, 1);
        invalid.key.work_id = "42".into();
        assert!(validate(&invalid).is_err());
        let mut position = reading(1, 1);
        position.position = Some(serde_json::json!([1, 2]));
        assert!(validate(&position).is_err());
    }
}
