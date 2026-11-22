//! 棚の読書位置を同期する。
//!
//! 書き込みはまず端末の SQLite に置き、通信はあとで
//! 行う。圏外で読んだ分もここに残り、次につながったときに届く。
//!
//! 書き込みには要求 UUID と手元の更新世代を添える。送り直しても一度だけ効き、別の端末や
//! デスクトップが先に読み進めていれば 409 と今の位置が返る。端末の時刻で勝者を決めない。
//! 先に進んでいたのがこの端末自身の書き込み（応答が届かなかった分）なら、今の世代で書き直す。
//! ほかの端末の位置なら書き込みを捨て、読む画面が「続きの位置があります」と案内する。
//!
//! 記録は棚・利用者・同期の世代ごと。どれかが変われば位置も未送信の書き込みも捨てる
//! （サーバーを戻したあとに、戻す前の要求を新しい操作として当てない）。

use super::device_db::{DeviceDb, records_error};
use super::shelf::{ShelfScope, canonical_generation, check_scope, library_changed};
use super::{ClientError, PcClientState, REQUEST_TIMEOUT, bounded_response_bytes};
use piep_library_protocol::{Id, MAX_IDENTITY_BYTES, MAX_MANIFEST_BYTES};
use piep_mobile_api::{
    MobileReading, MobileReadingLocation, MobileReadingPage, MobileReadingUpdate,
};
use reqwest::StatusCode;
use rusqlite::{OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;
use tokio::sync::Mutex as AsyncMutex;

/// 表の形（端末の記録の版 5）。
pub(super) const SHELF_READING_SCHEMA: &str = "CREATE TABLE shelf_reading_meta (
    id INTEGER PRIMARY KEY CHECK(id=1), scope TEXT NOT NULL, cursor TEXT NOT NULL);
    CREATE TABLE shelf_positions (work TEXT PRIMARY KEY, data TEXT NOT NULL);
    CREATE TABLE shelf_pending (work TEXT PRIMARY KEY, data TEXT NOT NULL);";

const PULL_LIMIT: usize = 200;
// 1 回の同期で読むページ数の上限。残りは次の同期で続きから読む。
const MAX_PULL_PAGES: usize = 50;
const RATIO_SCALE: u32 = 1_000_000;
const MAX_PAGE_NUMBER: u64 = 1_000_000;

pub struct ShelfReadingState {
    db: Arc<DeviceDb>,
    sync_lock: AsyncMutex<()>,
}

/// まだ棚へ届いていない書き込み。送り直しても同じ要求として届くよう、要求 UUID と世代ごと残す。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingWrite {
    request: String,
    expected: String,
    location: MobileReadingLocation,
    finished: bool,
}

fn scope_key(scope: &ShelfScope) -> String {
    format!(
        "{}/{}/{}",
        scope.library_id().get(),
        scope.subject_id().get(),
        scope.sync_epoch().get()
    )
}

fn valid_location(location: &MobileReadingLocation) -> bool {
    matches!(location.view_kind.as_str(), "snapshot" | "edit")
        && uuid::Uuid::parse_str(&location.view_id)
            .is_ok_and(|id| !id.is_nil() && id.to_string() == location.view_id)
        && location.page <= MAX_PAGE_NUMBER
        && location.ratio_ppm <= RATIO_SCALE
}

fn valid_reading(reading: &MobileReading) -> bool {
    uuid::Uuid::parse_str(&reading.work).is_ok_and(|id| id.to_string() == reading.work)
        && canonical_generation(&reading.generation)
        && reading.location.as_ref().is_none_or(valid_location)
}

fn generation(value: &str) -> u64 {
    value.parse().unwrap_or(0)
}

fn query_json<T: serde::de::DeserializeOwned>(
    tx: &Transaction<'_>,
    table: &str,
    work: &str,
) -> Result<Option<T>, ClientError> {
    let json: Option<String> = tx
        .query_row(
            &format!("SELECT data FROM {table} WHERE work = ?1"),
            [work],
            |row| row.get(0),
        )
        .optional()
        .map_err(records_error)?;
    json.map(|value| serde_json::from_str(&value).map_err(|_| ClientError::device_records()))
        .transpose()
}

fn put_json(
    tx: &Transaction<'_>,
    table: &str,
    work: &str,
    value: &impl Serialize,
) -> Result<(), ClientError> {
    let json = serde_json::to_string(value).map_err(|_| ClientError::device_records())?;
    tx.execute(
        &format!("INSERT INTO {table}(work, data) VALUES(?1, ?2) ON CONFLICT(work) DO UPDATE SET data=excluded.data"),
        rusqlite::params![work, json],
    )
    .map_err(records_error)?;
    Ok(())
}

/// 棚の位置を受け取る。世代が手元より古いもの（遅れて届いた応答）では書き換えない。
fn accept(tx: &Transaction<'_>, reading: &MobileReading) -> Result<(), ClientError> {
    let old: Option<MobileReading> = query_json(tx, "shelf_positions", &reading.work)?;
    if old
        .as_ref()
        .is_none_or(|old| generation(&reading.generation) >= generation(&old.generation))
    {
        put_json(tx, "shelf_positions", &reading.work, reading)?;
    }
    Ok(())
}

/// 送った書き込みを消す。送るあいだに次の位置が記録されていれば、その新しい内容は残す。
fn forget_sent(tx: &Transaction<'_>, work: &str, sent: &PendingWrite) -> Result<bool, ClientError> {
    let pending: Option<PendingWrite> = query_json(tx, "shelf_pending", work)?;
    if pending.as_ref() == Some(sent) {
        tx.execute("DELETE FROM shelf_pending WHERE work = ?1", [work])
            .map_err(records_error)?;
        return Ok(true);
    }
    Ok(false)
}

fn known_generation(tx: &Transaction<'_>, work: &str) -> Result<String, ClientError> {
    let known: Option<MobileReading> = query_json(tx, "shelf_positions", work)?;
    Ok(known.map_or_else(|| "0".to_string(), |known| known.generation))
}

#[derive(Deserialize)]
struct Conflict {
    error: String,
    current: MobileReading,
}

/// 一件を送った結果。
enum Sent {
    Stored(MobileReading),
    Conflict(MobileReading),
    /// 棚が受け取らない書き込み（外した作品・本文に無い位置）。持ち続けると後ろが詰まる。
    Refused,
}

impl ShelfReadingState {
    pub fn new(db: Arc<DeviceDb>) -> Self {
        Self {
            db,
            sync_lock: AsyncMutex::new(()),
        }
    }

    /// この棚の記録で読み書きする。棚・利用者・同期の世代が変わっていれば先に捨てる。
    fn with_scope<T>(
        &self,
        scope: &ShelfScope,
        action: impl FnOnce(&Transaction<'_>) -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        let key = scope_key(scope);
        self.db.transaction(|tx| {
            let saved: Option<String> = tx
                .query_row("SELECT scope FROM shelf_reading_meta WHERE id=1", [], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(records_error)?;
            if saved.as_deref() != Some(key.as_str()) {
                tx.execute_batch("DELETE FROM shelf_positions; DELETE FROM shelf_pending;")
                    .map_err(records_error)?;
                tx.execute(
                    "INSERT INTO shelf_reading_meta(id, scope, cursor) VALUES(1, ?1, '') ON CONFLICT(id) DO UPDATE SET scope=excluded.scope, cursor=''",
                    [&key],
                )
                .map_err(records_error)?;
            }
            action(tx)
        })
    }

    fn record(
        &self,
        scope: &ShelfScope,
        work: Id,
        location: MobileReadingLocation,
        finished: bool,
    ) -> Result<(), ClientError> {
        if !valid_location(&location) {
            return Err(ClientError::remote());
        }
        let work = work.get().to_string();
        self.with_scope(scope, |tx| {
            let expected = known_generation(tx, &work)?;
            // 新しく読んだ位置で置き換える。送りかけの要求の結果は 409 で分かる。
            put_json(
                tx,
                "shelf_pending",
                &work,
                &PendingWrite {
                    request: uuid::Uuid::now_v7().to_string(),
                    expected,
                    location,
                    finished,
                },
            )
        })
    }

    fn get(&self, scope: &ShelfScope, work: Id) -> Result<Option<MobileReading>, ClientError> {
        self.with_scope(scope, |tx| {
            query_json(tx, "shelf_positions", &work.get().to_string())
        })
    }

    async fn sync(&self, pc: &PcClientState, scope: ShelfScope) -> Result<(), ClientError> {
        let _running = self.sync_lock.lock().await;
        // 自分の書き込みの続きを書き直したら、もう一度だけ送る。
        for _ in 0..2 {
            if !self.push(pc, scope).await? {
                break;
            }
        }
        self.pull(pc, scope).await
    }

    /// 溜まった書き込みを送る。書き直した書き込みがあれば true。
    async fn push(&self, pc: &PcClientState, scope: ShelfScope) -> Result<bool, ClientError> {
        let pending = self.with_scope(&scope, |tx| {
            let mut query = tx
                .prepare("SELECT work, data FROM shelf_pending ORDER BY work")
                .map_err(records_error)?;
            let rows = query
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(records_error)?;
            let mut pending = Vec::new();
            for row in rows {
                let (work, data) = row.map_err(records_error)?;
                let write: PendingWrite =
                    serde_json::from_str(&data).map_err(|_| ClientError::device_records())?;
                pending.push((work, write));
            }
            Ok(pending)
        })?;
        let mut rewritten = false;
        for (work, write) in pending {
            let sent = send(pc, scope, &work, &write).await?;
            rewritten |= self.with_scope(&scope, |tx| match &sent {
                Sent::Stored(stored) => {
                    accept(tx, stored)?;
                    forget_sent(tx, &work, &write)?;
                    Ok(false)
                }
                Sent::Conflict(current) => {
                    accept(tx, current)?;
                    if !forget_sent(tx, &work, &write)? {
                        return Ok(false);
                    }
                    if !current.mine {
                        return Ok(false);
                    }
                    // 先に届いていたのはこの端末の前の書き込み。今の世代で書き直す。
                    put_json(
                        tx,
                        "shelf_pending",
                        &work,
                        &PendingWrite {
                            request: uuid::Uuid::now_v7().to_string(),
                            expected: current.generation.clone(),
                            ..write.clone()
                        },
                    )?;
                    Ok(true)
                }
                Sent::Refused => {
                    forget_sent(tx, &work, &write)?;
                    Ok(false)
                }
            })?;
        }
        Ok(rewritten)
    }

    /// 前回の続きから、棚の位置の変わった分を読む。
    async fn pull(&self, pc: &PcClientState, scope: ShelfScope) -> Result<(), ClientError> {
        for _ in 0..MAX_PULL_PAGES {
            let cursor: String = self.with_scope(&scope, |tx| {
                tx.query_row(
                    "SELECT cursor FROM shelf_reading_meta WHERE id=1",
                    [],
                    |row| row.get(0),
                )
                .map_err(records_error)
            })?;
            let Some(page) = fetch(pc, scope, &cursor).await? else {
                // 印が古くなった（サーバーの写しが変わった）。初めから取り直す。
                self.with_scope(&scope, |tx| {
                    tx.execute("UPDATE shelf_reading_meta SET cursor='' WHERE id=1", [])
                        .map_err(records_error)?;
                    Ok(())
                })?;
                if cursor.is_empty() {
                    return Err(ClientError::protocol());
                }
                continue;
            };
            self.with_scope(&scope, |tx| {
                for item in &page.items {
                    accept(tx, item)?;
                }
                tx.execute(
                    "UPDATE shelf_reading_meta SET cursor=?1 WHERE id=1",
                    [&page.cursor],
                )
                .map_err(records_error)?;
                Ok(())
            })?;
            if !page.more {
                break;
            }
        }
        Ok(())
    }
}

fn connection(
    pc: &PcClientState,
    scope: ShelfScope,
) -> Result<(u64, super::SavedConnection, reqwest::Client), ClientError> {
    let (generation, saved) = pc.connection_snapshot()?;
    if !scope.connection_is(generation) {
        return Err(library_changed());
    }
    let saved = saved.ok_or_else(ClientError::invalid_pairing)?;
    let client = pc.client(&saved)?;
    Ok((generation, saved, client))
}

async fn send(
    pc: &PcClientState,
    scope: ShelfScope,
    work: &str,
    write: &PendingWrite,
) -> Result<Sent, ClientError> {
    let (generation, saved, client) = connection(pc, scope)?;
    let body = MobileReadingUpdate {
        request_id: write.request.clone(),
        expected: write.expected.clone(),
        location: write.location.clone(),
        finished: write.finished,
    };
    let response = client
        .put(format!(
            "{}/api/libraries/{}/works/{work}/reading",
            saved.url,
            scope.library_id().get(),
        ))
        .bearer_auth(&saved.token)
        .timeout(REQUEST_TIMEOUT)
        .json(&body)
        .send()
        .await
        .map_err(|error| ClientError::from_send(&error, saved.pinned()))?;
    let sent = match response.status() {
        // 世代の食い違いは棚の入れ替わり（409）と同じ番号で返るので、本文で見分ける。
        StatusCode::CONFLICT if scope.matches(response.headers()) => {
            let bytes = bounded_response_bytes(response, MAX_IDENTITY_BYTES).await?;
            let conflict: Conflict =
                serde_json::from_slice(&bytes).map_err(|_| library_changed())?;
            if conflict.error != "reading_conflict"
                || conflict.current.work != work
                || !valid_reading(&conflict.current)
            {
                return Err(library_changed());
            }
            Sent::Conflict(conflict.current)
        }
        StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND if scope.matches(response.headers()) => {
            Sent::Refused
        }
        _ => {
            let bytes =
                bounded_response_bytes(check_scope(response, scope)?, MAX_IDENTITY_BYTES).await?;
            let stored: MobileReading =
                serde_json::from_slice(&bytes).map_err(|_| ClientError::protocol())?;
            if stored.work != work || !valid_reading(&stored) {
                return Err(ClientError::protocol());
            }
            Sent::Stored(stored)
        }
    };
    pc.with_connection(generation, || Ok(sent))
}

/// 一ページ読む。印が古いと断られたら None。
async fn fetch(
    pc: &PcClientState,
    scope: ShelfScope,
    cursor: &str,
) -> Result<Option<MobileReadingPage>, ClientError> {
    let (generation, saved, client) = connection(pc, scope)?;
    let mut url = reqwest::Url::parse(&format!(
        "{}/api/libraries/{}/reading",
        saved.url,
        scope.library_id().get()
    ))
    .map_err(|_| ClientError::storage())?;
    url.query_pairs_mut()
        .append_pair("cursor", cursor)
        .append_pair("limit", &PULL_LIMIT.to_string());
    let response = client
        .get(url)
        .bearer_auth(&saved.token)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|error| ClientError::from_send(&error, saved.pinned()))?;
    if response.status() == StatusCode::BAD_REQUEST && scope.matches(response.headers()) {
        return pc.with_connection(generation, || Ok(None));
    }
    let bytes = bounded_response_bytes(check_scope(response, scope)?, MAX_MANIFEST_BYTES).await?;
    let page: MobileReadingPage =
        serde_json::from_slice(&bytes).map_err(|_| ClientError::protocol())?;
    if page.cursor.is_empty() || page.cursor.len() > 256 || !page.items.iter().all(valid_reading) {
        return Err(ClientError::protocol());
    }
    pc.with_connection(generation, || Ok(Some(page)))
}

/// 読んだ位置を端末に置く。通信はしない。
#[tauri::command]
pub async fn pc_reading_record(
    state: State<'_, ShelfReadingState>,
    scope: ShelfScope,
    work: Id,
    location: MobileReadingLocation,
    finished: bool,
) -> Result<(), ClientError> {
    state.record(&scope, work, location, finished)
}

/// 溜まった位置を送り、棚の位置の変わった分を受け取る。
#[tauri::command]
pub async fn pc_reading_sync(
    state: State<'_, ShelfReadingState>,
    pc: State<'_, PcClientState>,
    scope: ShelfScope,
) -> Result<(), ClientError> {
    state.sync(&pc, scope).await
}

/// 最後に受け取った、この作品の棚の位置を返す。通信はしない。
#[tauri::command]
pub async fn pc_reading_get(
    state: State<'_, ShelfReadingState>,
    scope: ShelfScope,
    work: Id,
) -> Result<Option<MobileReading>, ClientError> {
    state.get(&scope, work)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> Id {
        Id::new(uuid::Uuid::now_v7()).unwrap()
    }

    fn location(page: u64) -> MobileReadingLocation {
        MobileReadingLocation {
            view_kind: "snapshot".into(),
            view_id: uuid::Uuid::now_v7().to_string(),
            page,
            ratio_ppm: 500_000,
        }
    }

    fn reading(work: Id, generation: &str, mine: bool) -> MobileReading {
        MobileReading {
            work: work.get().to_string(),
            location: Some(location(1)),
            finished: false,
            mine,
            updated_at: Some(1),
            generation: generation.into(),
        }
    }

    fn pending(state: &ShelfReadingState, scope: &ShelfScope, work: Id) -> Option<PendingWrite> {
        state
            .with_scope(scope, |tx| {
                query_json(tx, "shelf_pending", &work.get().to_string())
            })
            .unwrap()
    }

    #[test]
    fn a_write_takes_the_known_generation_and_the_newest_reading_replaces_it() {
        let state = ShelfReadingState::new(Arc::new(DeviceDb::in_memory()));
        let scope = ShelfScope::for_tests(id(), id(), id(), 1);
        let work = id();
        state.record(&scope, work, location(0), false).unwrap();
        let first = pending(&state, &scope, work).unwrap();
        assert_eq!(first.expected, "0");
        state
            .with_scope(&scope, |tx| accept(tx, &reading(work, "4", false)))
            .unwrap();
        state.record(&scope, work, location(2), true).unwrap();
        let second = pending(&state, &scope, work).unwrap();
        assert_eq!(
            (
                second.expected.as_str(),
                second.location.page,
                second.finished
            ),
            ("4", 2, true)
        );
        assert_ne!(first.request, second.request);
        // 送るあいだに次の位置が記録されたら、送った分を消しても新しい内容は残る。
        state
            .with_scope(&scope, |tx| {
                forget_sent(tx, &work.get().to_string(), &first)
            })
            .unwrap();
        assert_eq!(pending(&state, &scope, work), Some(second.clone()));
        state
            .with_scope(&scope, |tx| {
                forget_sent(tx, &work.get().to_string(), &second)
            })
            .unwrap();
        assert_eq!(pending(&state, &scope, work), None);
    }

    #[test]
    fn an_older_generation_does_not_replace_a_newer_one_and_another_epoch_starts_empty() {
        let state = ShelfReadingState::new(Arc::new(DeviceDb::in_memory()));
        let (library, subject) = (id(), id());
        let scope = ShelfScope::for_tests(library, subject, id(), 1);
        let work = id();
        state
            .with_scope(&scope, |tx| {
                accept(tx, &reading(work, "5", false))?;
                accept(tx, &reading(work, "3", true))
            })
            .unwrap();
        assert_eq!(state.get(&scope, work).unwrap().unwrap().generation, "5");
        state.record(&scope, work, location(0), false).unwrap();
        // 接続を作り直しただけ（同じ棚・利用者・同期の世代）なら残る。
        let reconnected = ShelfScope::for_tests(library, subject, scope.sync_epoch(), 2);
        assert!(state.get(&reconnected, work).unwrap().is_some());
        assert!(pending(&state, &reconnected, work).is_some());
        // サーバーを戻した（同期の世代が変わった）なら、位置も未送信の要求も捨てる。
        let restored = ShelfScope::for_tests(library, subject, id(), 2);
        assert!(state.get(&restored, work).unwrap().is_none());
        assert!(pending(&state, &restored, work).is_none());
    }

    #[test]
    fn readings_from_the_shelf_are_checked_before_they_are_kept() {
        let work = id();
        assert!(valid_reading(&reading(work, "1", false)));
        let mut bad = reading(work, "01", false);
        assert!(!valid_reading(&bad));
        bad = reading(work, "1", false);
        bad.work = bad.work.to_uppercase();
        assert!(!valid_reading(&bad));
        for change in [
            |l: &mut MobileReadingLocation| l.view_kind = "page".into(),
            |l: &mut MobileReadingLocation| l.view_id = "x".into(),
            |l: &mut MobileReadingLocation| l.ratio_ppm = RATIO_SCALE + 1,
            |l: &mut MobileReadingLocation| l.page = MAX_PAGE_NUMBER + 1,
        ] {
            let mut bad = reading(work, "1", false);
            change(bad.location.as_mut().unwrap());
            assert!(!valid_reading(&bad));
        }
        let cleared = MobileReading {
            location: None,
            ..reading(work, "2", true)
        };
        assert!(valid_reading(&cleared));
    }
}
