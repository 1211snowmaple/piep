//! 読書位置をサーバーと同期する。
//!
//! 読書を止めないよう、書き込みはまず端末の SQLite（`device_db`）に置き、通信はあとで行う。
//! 圏外で読んだ分もここに残り、次につながったときに届く。

use super::device_db::DeviceDb;
use super::*;
use std::collections::BTreeMap;
use tokio::sync::Mutex as AsyncMutex;

const PULL_LIMIT: i64 = 200;
// 1 回の同期で読むページ数の上限。残りは次の同期で続きから読む。
const MAX_PULL_PAGES: usize = 50;

pub struct ReadingSyncState {
    db: Arc<DeviceDb>,
    sync_lock: AsyncMutex<()>,
}

struct SyncTarget {
    saved: SavedConnection,
    generation: u64,
}

/// サーバーが持つ読書位置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadingPosition {
    source: String,
    source_id: String,
    version: Option<i64>,
    edit_hash: Option<String>,
    page: i64,
    position: f64,
    finished: bool,
    device_id: String,
    device_time: i64,
    cursor: i64,
}

/// まだサーバーへ届いていない書き込み。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadingWrite {
    source: String,
    source_id: String,
    version: Option<i64>,
    edit_hash: Option<String>,
    page: i64,
    position: f64,
    finished: bool,
    device_time: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SyncFile {
    url: String,
    device_id: String,
    cursor: i64,
    positions: BTreeMap<String, ReadingPosition>,
    pending: BTreeMap<String, ReadingWrite>,
}

fn reading_key(source: &str, source_id: &str) -> String {
    format!("{source}:{source_id}")
}

fn validate_write(write: &ReadingWrite) -> Result<(), ClientError> {
    if !matches!(write.source.as_str(), "pixiv" | "fanbox")
        || write.source_id.is_empty()
        || write.source_id.chars().count() > 200
        || !(0..=1_000_000).contains(&write.page)
        || !(0.0..=1.0).contains(&write.position)
        || write.device_time <= 0
        || write
            .edit_hash
            .as_ref()
            .is_some_and(|hash| hash.len() > 128)
    {
        return Err(ClientError::remote());
    }
    Ok(())
}

fn write_endpoint(base: &str, source: &str, source_id: &str) -> Result<Url, ClientError> {
    let mut url = Url::parse(base).map_err(|_| ClientError::storage())?;
    url.path_segments_mut()
        .map_err(|_| ClientError::storage())?
        .pop_if_empty()
        .extend(["api", "v1", "reading", source, source_id]);
    Ok(url)
}

fn storage_error(_: rusqlite::Error) -> ClientError {
    ClientError::storage()
}

fn query_entry<T: serde::de::DeserializeOwned>(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    key: &str,
) -> Result<Option<T>, ClientError> {
    use rusqlite::OptionalExtension;
    let json: Option<String> = tx
        .query_row(
            &format!("SELECT data FROM {table} WHERE key = ?1"),
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage_error)?;
    json.map(|value| serde_json::from_str(&value).map_err(|_| ClientError::storage()))
        .transpose()
}

fn put_entry(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    key: &str,
    value: &impl Serialize,
) -> Result<(), ClientError> {
    let json = serde_json::to_string(value).map_err(|_| ClientError::storage())?;
    tx.execute(&format!("INSERT INTO {table}(key, data) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET data=excluded.data"), rusqlite::params![key, json]).map_err(storage_error)?;
    Ok(())
}

fn accept_position(
    tx: &rusqlite::Transaction<'_>,
    position: ReadingPosition,
) -> Result<(), ClientError> {
    let key = reading_key(&position.source, &position.source_id);
    let old: Option<ReadingPosition> = query_entry(tx, "positions", &key)?;
    if old
        .as_ref()
        .is_none_or(|old| position.device_time >= old.device_time && position != *old)
    {
        put_entry(tx, "positions", &key, &position)?;
    }
    Ok(())
}

fn delivered_write(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    sent: &ReadingWrite,
) -> Result<(), ClientError> {
    let pending: Option<ReadingWrite> = query_entry(tx, "pending", key)?;
    // 同じミリ秒に別の位置が記録されても、その新しい内容を消さない。
    if pending.as_ref() == Some(sent) {
        tx.execute("DELETE FROM pending WHERE key=?1", [key])
            .map_err(storage_error)?;
    }
    Ok(())
}

/// 0.5.0 までの `reading-sync.json` を写す。元の JSON は消さずに残す。
pub(super) fn import_legacy_json(
    tx: &rusqlite::Transaction<'_>,
    file: &std::path::Path,
) -> Result<(), ClientError> {
    let legacy: SyncFile = match fs::read(file) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| ClientError::device_records())?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SyncFile::default(),
        Err(_) => return Err(ClientError::device_records()),
    };
    tx.execute(
        "INSERT INTO sync_meta VALUES(1, ?1, ?2, ?3)",
        rusqlite::params![legacy.url, legacy.device_id, legacy.cursor],
    )
    .map_err(storage_error)?;
    for (key, position) in legacy.positions {
        put_entry(tx, "positions", &key, &position)?;
    }
    for (key, pending) in legacy.pending {
        put_entry(tx, "pending", &key, &pending)?;
    }
    Ok(())
}

impl ReadingSyncState {
    pub fn new(db: Arc<DeviceDb>) -> Self {
        Self {
            db,
            sync_lock: AsyncMutex::new(()),
        }
    }

    fn with_store<T>(
        &self,
        saved: &SavedConnection,
        action: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        self.db
            .transaction(|tx| self.adjust_and_run(tx, saved, action))
    }

    fn adjust_and_run<T>(
        &self,
        tx: &rusqlite::Transaction<'_>,
        saved: &SavedConnection,
        action: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        let (url, device_id): (String, String) = tx
            .query_row(
                "SELECT url, device_id FROM sync_meta WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(storage_error)?;
        if url != saved.url {
            tx.execute_batch("DELETE FROM positions; DELETE FROM pending;")
                .map_err(storage_error)?;
        } else if device_id != saved.device_id {
            tx.execute("DELETE FROM positions", [])
                .map_err(storage_error)?;
        }
        if url != saved.url || device_id != saved.device_id {
            tx.execute(
                "UPDATE sync_meta SET url=?1, device_id=?2, cursor=0 WHERE id=1",
                rusqlite::params![saved.url, saved.device_id],
            )
            .map_err(storage_error)?;
        }
        action(tx)
    }

    fn store_current<T>(
        &self,
        pc: &PcClientState,
        target: &SyncTarget,
        action: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        // 登録変更は、この世代確認から SQLite commit まで待つ。
        pc.with_connection(target.generation, || self.with_store(&target.saved, action))
    }

    fn record_current(
        &self,
        pc: &PcClientState,
        target: &SyncTarget,
        write: ReadingWrite,
    ) -> Result<(), ClientError> {
        self.store_current(pc, target, |tx| {
            let key = reading_key(&write.source, &write.source_id);
            let pending: Option<ReadingWrite> = query_entry(tx, "pending", &key)?;
            if pending
                .as_ref()
                .is_none_or(|old| write.device_time >= old.device_time && write != *old)
            {
                put_entry(tx, "pending", &key, &write)?;
            }
            Ok(())
        })
    }

    async fn sync(&self, pc: &PcClientState) -> Result<(), ClientError> {
        // 待っている間に登録が変わるため、接続はロック取得後に読む。
        let _running = self.sync_lock.lock().await;
        let Some(target) = sync_target(pc)? else {
            return Ok(());
        };
        let client = pc.client(&target.saved)?;
        self.push(&client, pc, &target).await?;
        self.pull(&client, pc, &target).await
    }

    /// 溜まった書き込みを送る。圏外やサーバーの不調なら残して次に任せる。
    async fn push(
        &self,
        client: &reqwest::Client,
        pc: &PcClientState,
        target: &SyncTarget,
    ) -> Result<(), ClientError> {
        let saved = &target.saved;
        let pending = self.store_current(pc, target, |tx| {
            let mut query = tx
                .prepare("SELECT key, data FROM pending ORDER BY key")
                .map_err(storage_error)?;
            let rows = query
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(storage_error)?;
            let mut pending = Vec::new();
            for row in rows {
                let (key, data) = row.map_err(storage_error)?;
                pending.push((
                    key,
                    serde_json::from_str::<ReadingWrite>(&data)
                        .map_err(|_| ClientError::storage())?,
                ));
            }
            Ok(pending)
        })?;
        for (key, write) in pending {
            pc.with_connection(target.generation, || Ok(()))?;
            let response = client
                .put(write_endpoint(&saved.url, &write.source, &write.source_id)?)
                .bearer_auth(&saved.token)
                .json(&serde_json::json!({
                    "version": write.version,
                    "editHash": write.edit_hash,
                    "page": write.page,
                    "position": write.position,
                    "finished": write.finished,
                    "deviceTime": write.device_time,
                }))
                .send()
                .await
                .map_err(|error| ClientError::from_send(&error, saved.pinned()))?;
            let status = response.status();
            if status.is_success() {
                let stored: ReadingPosition = response.bounded_json().await?;
                self.store_current(pc, target, |tx| {
                    accept_position(tx, stored)?;
                    delivered_write(tx, &key, &write)
                })?;
            } else if status == reqwest::StatusCode::UNAUTHORIZED {
                return Err(ClientError::revoked());
            } else if status.is_client_error() {
                // サーバーが受け取らない書き込みを持ち続けると、後ろが永久に詰まる。
                self.store_current(pc, target, |tx| delivered_write(tx, &key, &write))?;
            } else {
                return Err(ClientError::remote());
            }
        }
        Ok(())
    }

    /// 前回の続きからサーバーの変更を読む。
    async fn pull(
        &self,
        client: &reqwest::Client,
        pc: &PcClientState,
        target: &SyncTarget,
    ) -> Result<(), ClientError> {
        #[derive(Deserialize)]
        struct Page {
            items: Vec<ReadingPosition>,
            cursor: i64,
        }
        let saved = &target.saved;
        for _ in 0..MAX_PULL_PAGES {
            let since: i64 = self.store_current(pc, target, |tx| {
                tx.query_row("SELECT cursor FROM sync_meta WHERE id=1", [], |row| {
                    row.get(0)
                })
                .map_err(storage_error)
            })?;
            let response = client
                .get(format!(
                    "{}/api/v1/reading?since={since}&limit={PULL_LIMIT}",
                    saved.url
                ))
                .bearer_auth(&saved.token)
                .send()
                .await
                .map_err(|error| ClientError::from_send(&error, saved.pinned()))?;
            let page: Page = check_response(response)?.bounded_json().await?;
            let full = page.items.len() as i64 >= PULL_LIMIT;
            self.store_current(pc, target, |tx| {
                for item in page.items {
                    accept_position(tx, item)?;
                }
                tx.execute(
                    "UPDATE sync_meta SET cursor=?1 WHERE id=1 AND cursor < ?1",
                    [page.cursor],
                )
                .map_err(storage_error)?;
                Ok(())
            })?;
            if !full {
                break;
            }
        }
        Ok(())
    }
}

fn sync_target(pc: &PcClientState) -> Result<Option<SyncTarget>, ClientError> {
    let (generation, saved) = pc.connection_snapshot()?;
    Ok(saved
        .filter(SavedConnection::can_sync_reading)
        .map(|saved| SyncTarget { saved, generation }))
}

/// 読んだ位置を端末に置く。通信はしない。
#[tauri::command]
pub async fn reading_record(
    state: State<'_, ReadingSyncState>,
    pc: State<'_, PcClientState>,
    write: ReadingWrite,
) -> Result<(), ClientError> {
    validate_write(&write)?;
    let Some(saved) = sync_target(&pc)? else {
        return Ok(());
    };
    state.record_current(&pc, &saved, write)
}

/// 溜まった位置を送り、サーバーの変更を受け取る。
#[tauri::command]
pub async fn reading_sync(
    state: State<'_, ReadingSyncState>,
    pc: State<'_, PcClientState>,
) -> Result<(), ClientError> {
    state.sync(&pc).await
}

/// 最後に受け取った、この作品の位置を返す。通信はしない。
#[tauri::command]
pub async fn reading_get(
    state: State<'_, ReadingSyncState>,
    pc: State<'_, PcClientState>,
    source: String,
    source_id: String,
) -> Result<Option<ReadingPosition>, ClientError> {
    let Some(saved) = sync_target(&pc)? else {
        return Ok(None);
    };
    state.store_current(&pc, &saved, |tx| {
        query_entry(tx, "positions", &reading_key(&source, &source_id))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(url: &str, device_id: &str) -> SavedConnection {
        SavedConnection {
            url: url.to_string(),
            fingerprint: None,
            certificate_der: None,
            token: String::new(),
            device_id: device_id.to_string(),
            device_name: "phone".to_string(),
            capabilities: vec!["reading-sync".to_string()],
        }
    }

    fn write(source_id: &str, page: i64, device_time: i64) -> ReadingWrite {
        ReadingWrite {
            source: "pixiv".to_string(),
            source_id: source_id.to_string(),
            version: Some(1),
            edit_hash: None,
            page,
            position: 0.5,
            finished: false,
            device_time,
        }
    }

    fn position(source_id: &str, page: i64, device_time: i64, cursor: i64) -> ReadingPosition {
        ReadingPosition {
            source: "pixiv".to_string(),
            source_id: source_id.to_string(),
            version: Some(1),
            edit_hash: None,
            page,
            position: 0.5,
            finished: false,
            device_id: "other".to_string(),
            device_time,
            cursor,
        }
    }

    fn persisted_pending(
        state: &ReadingSyncState,
        pc: &PcClientState,
        target: &SyncTarget,
    ) -> Option<ReadingWrite> {
        state
            .store_current(pc, target, |tx| query_entry(tx, "pending", "pixiv:1"))
            .unwrap()
    }

    #[test]
    fn newest_write_wins_and_an_older_one_does_not_replace_it() {
        let (root, state, pc) = super::regressions::fixture();
        pc.save(&saved("https://a.example", "one")).unwrap();
        let target = sync_target(&pc).unwrap().unwrap();
        state
            .record_current(&pc, &target, write("1", 5, 200))
            .unwrap();
        state
            .record_current(&pc, &target, write("1", 2, 100))
            .unwrap();
        assert_eq!(persisted_pending(&state, &pc, &target).unwrap().page, 5);
        state
            .record_current(&pc, &target, write("1", 7, 300))
            .unwrap();
        assert_eq!(persisted_pending(&state, &pc, &target).unwrap().page, 7);
        drop((state, pc));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_write_made_while_sending_survives_delivery() {
        let (root, state, pc) = super::regressions::fixture();
        pc.save(&saved("https://a.example", "one")).unwrap();
        let target = sync_target(&pc).unwrap().unwrap();
        state
            .record_current(&pc, &target, write("1", 5, 200))
            .unwrap();
        state
            .record_current(&pc, &target, write("1", 7, 300))
            .unwrap();
        state
            .store_current(&pc, &target, |tx| {
                delivered_write(tx, "pixiv:1", &write("1", 5, 200))
            })
            .unwrap();
        assert_eq!(persisted_pending(&state, &pc, &target).unwrap().page, 7);
        state
            .store_current(&pc, &target, |tx| {
                delivered_write(tx, "pixiv:1", &write("1", 7, 300))
            })
            .unwrap();
        assert!(persisted_pending(&state, &pc, &target).is_none());
        drop((state, pc));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_older_position_from_the_server_does_not_replace_a_newer_one() {
        let (root, state, pc) = super::regressions::fixture();
        pc.save(&saved("https://a.example", "one")).unwrap();
        let target = sync_target(&pc).unwrap().unwrap();
        state
            .store_current(&pc, &target, |tx| {
                accept_position(tx, position("1", 5, 200, 1))?;
                accept_position(tx, position("1", 2, 100, 2))
            })
            .unwrap();
        let found: ReadingPosition = state
            .store_current(&pc, &target, |tx| {
                Ok(query_entry(tx, "positions", "pixiv:1")?.unwrap())
            })
            .unwrap();
        assert_eq!(found.page, 5);
        drop((state, pc));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn another_server_starts_empty_and_a_new_pairing_keeps_only_unsent_reading() {
        let (root, state, pc) = super::regressions::fixture();
        pc.save(&saved("https://a.example", "one")).unwrap();
        let target = sync_target(&pc).unwrap().unwrap();
        state
            .record_current(&pc, &target, write("1", 5, 200))
            .unwrap();
        state
            .store_current(&pc, &target, |tx| {
                accept_position(tx, position("2", 3, 100, 9))?;
                tx.execute("UPDATE sync_meta SET cursor=9", [])
                    .map_err(storage_error)?;
                Ok(())
            })
            .unwrap();
        pc.save(&saved("https://a.example", "two")).unwrap();
        let target = sync_target(&pc).unwrap().unwrap();
        state.store_current(&pc, &target, |tx| {
            let (cursor, positions, pending): (i64, i64, i64) = tx.query_row("SELECT cursor, (SELECT count(*) FROM positions), (SELECT count(*) FROM pending) FROM sync_meta", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).map_err(storage_error)?;
            assert_eq!((cursor, positions, pending), (0, 0, 1));
            Ok(())
        }).unwrap();
        pc.save(&saved("https://b.example", "three")).unwrap();
        let target = sync_target(&pc).unwrap().unwrap();
        assert!(persisted_pending(&state, &pc, &target).is_none());
        drop((state, pc));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validates_what_the_reader_sends() {
        assert!(validate_write(&write("1", 0, 1)).is_ok());
        for bad in [
            ReadingWrite {
                source: "web".to_string(),
                ..write("1", 0, 1)
            },
            ReadingWrite {
                source_id: String::new(),
                ..write("1", 0, 1)
            },
            ReadingWrite {
                page: -1,
                ..write("1", 0, 1)
            },
            ReadingWrite {
                position: 1.5,
                ..write("1", 0, 1)
            },
            ReadingWrite {
                position: f64::NAN,
                ..write("1", 0, 1)
            },
            ReadingWrite {
                device_time: 0,
                ..write("1", 0, 1)
            },
            ReadingWrite {
                edit_hash: Some("x".repeat(129)),
                ..write("1", 0, 1)
            },
        ] {
            assert!(validate_write(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn write_endpoint_keeps_the_path_prefix_and_escapes_the_id() {
        assert_eq!(
            write_endpoint("https://example.com/piep", "fanbox", "a/b c")
                .unwrap()
                .as_str(),
            "https://example.com/piep/api/v1/reading/fanbox/a%2Fb%20c",
        );
        assert_eq!(
            write_endpoint("https://192.168.1.2:38777", "pixiv", "123")
                .unwrap()
                .as_str(),
            "https://192.168.1.2:38777/api/v1/reading/pixiv/123",
        );
    }
}

#[cfg(test)]
#[path = "reading_sync_regressions.rs"]
mod regressions;
