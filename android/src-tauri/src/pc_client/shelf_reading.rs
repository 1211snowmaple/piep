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
//! 未送信の位置は棚と利用者ごとに残す。同期の世代が変わったら、棚の位置を読み直してから、
//! 新しい更新世代を使って送り直す。対象の作品・版と位置は、棚が書き込み時に確かめる。

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

pub(super) const SHELF_READING_SCHEMA: &str = "CREATE TABLE shelf_reading_cursors (
    scope TEXT PRIMARY KEY, cursor TEXT NOT NULL);
    CREATE TABLE shelf_positions (
        scope TEXT NOT NULL, work TEXT NOT NULL, data TEXT NOT NULL, PRIMARY KEY(scope, work));
    CREATE TABLE shelf_pending (
        scope TEXT NOT NULL, work TEXT NOT NULL, data TEXT NOT NULL, PRIMARY KEY(scope, work));
    CREATE TABLE shelf_reading_rejections (
        scope TEXT NOT NULL, work TEXT NOT NULL, epoch TEXT NOT NULL,
        request TEXT NOT NULL, reason TEXT NOT NULL, PRIMARY KEY(scope, work));";

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
    epoch: Id,
    request: String,
    expected: String,
    location: MobileReadingLocation,
    finished: bool,
}

struct ReadingKeys {
    owner: String,
    epoch: String,
}

impl ReadingKeys {
    fn new(scope: ShelfScope) -> Self {
        let owner = format!("{}/{}", scope.library_id().get(), scope.subject_id().get());
        let epoch = format!("{owner}/{}", scope.sync_epoch().get());
        Self { owner, epoch }
    }
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
    key: &str,
    work: &str,
) -> Result<Option<T>, ClientError> {
    let json: Option<String> = tx
        .query_row(
            &format!("SELECT data FROM {table} WHERE scope = ?1 AND work = ?2"),
            [key, work],
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
    key: &str,
    work: &str,
    value: &impl Serialize,
) -> Result<(), ClientError> {
    let json = serde_json::to_string(value).map_err(|_| ClientError::device_records())?;
    tx.execute(
        &format!("INSERT INTO {table}(scope, work, data) VALUES(?1, ?2, ?3) ON CONFLICT(scope, work) DO UPDATE SET data=excluded.data"),
        rusqlite::params![key, work, json],
    )
    .map_err(records_error)?;
    Ok(())
}

/// 棚の位置を受け取る。世代が手元より古いもの（遅れて届いた応答）では書き換えない。
fn accept(tx: &Transaction<'_>, key: &str, reading: &MobileReading) -> Result<(), ClientError> {
    let old: Option<MobileReading> = query_json(tx, "shelf_positions", key, &reading.work)?;
    if old
        .as_ref()
        .is_none_or(|old| generation(&reading.generation) >= generation(&old.generation))
    {
        put_json(tx, "shelf_positions", key, &reading.work, reading)?;
    }
    Ok(())
}

/// 送った書き込みを消す。送るあいだに次の位置が記録されていれば、その新しい内容は残す。
fn forget_sent(
    tx: &Transaction<'_>,
    key: &str,
    work: &str,
    sent: &PendingWrite,
) -> Result<bool, ClientError> {
    let pending: Option<PendingWrite> = query_json(tx, "shelf_pending", key, work)?;
    if pending.as_ref() == Some(sent) {
        tx.execute(
            "DELETE FROM shelf_pending WHERE scope = ?1 AND work = ?2",
            [key, work],
        )
        .map_err(records_error)?;
        return Ok(true);
    }
    Ok(false)
}

fn known_generation(tx: &Transaction<'_>, key: &str, work: &str) -> Result<String, ClientError> {
    let known: Option<MobileReading> = query_json(tx, "shelf_positions", key, work)?;
    Ok(known.map_or_else(|| "0".to_string(), |known| known.generation))
}

#[derive(Deserialize)]
struct Conflict {
    error: String,
    current: MobileReading,
}

#[derive(Deserialize)]
struct Refusal {
    error: String,
}

fn record_rejection(
    tx: &Transaction<'_>,
    key: &str,
    work: &str,
    write: &PendingWrite,
    reason: &str,
) -> Result<(), ClientError> {
    tx.execute(
        "INSERT INTO shelf_reading_rejections(scope, work, epoch, request, reason)
         VALUES(?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(scope, work) DO UPDATE SET epoch=excluded.epoch, request=excluded.request, reason=excluded.reason",
        rusqlite::params![key, work, write.epoch.get().to_string(), write.request, reason],
    )
    .map_err(records_error)?;
    Ok(())
}

/// 一件を送った結果。
enum Sent {
    Stored(MobileReading),
    Conflict(MobileReading),
    /// 棚が受け取らない書き込み（外した作品・本文に無い位置）。持ち続けると後ろが詰まる。
    Refused(&'static str),
}

impl ShelfReadingState {
    pub fn new(db: Arc<DeviceDb>) -> Self {
        Self {
            db,
            sync_lock: AsyncMutex::new(()),
        }
    }

    fn with_current_scope<T>(
        &self,
        pc: &PcClientState,
        scope: ShelfScope,
        action: impl FnOnce(&Transaction<'_>) -> Result<T, ClientError>,
    ) -> Result<T, ClientError> {
        pc.with_shelf_scope(scope, || self.db.transaction(action))
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
        let keys = ReadingKeys::new(*scope);
        self.db.transaction(|tx| {
            let expected = known_generation(tx, &keys.epoch, &work)?;
            // 新しく読んだ位置で置き換える。送りかけの要求の結果は 409 で分かる。
            put_json(
                tx,
                "shelf_pending",
                &keys.owner,
                &work,
                &PendingWrite {
                    epoch: scope.sync_epoch(),
                    request: uuid::Uuid::now_v7().to_string(),
                    expected,
                    location,
                    finished,
                },
            )
        })
    }

    fn get(&self, scope: &ShelfScope, work: Id) -> Result<Option<MobileReading>, ClientError> {
        let keys = ReadingKeys::new(*scope);
        self.db.transaction(|tx| {
            query_json(tx, "shelf_positions", &keys.epoch, &work.get().to_string())
        })
    }

    async fn sync(&self, pc: &PcClientState, scope: ShelfScope) -> Result<(), ClientError> {
        let _running = self.sync_lock.lock().await;
        let keys = ReadingKeys::new(scope);
        let mut pending = self.pending(pc, scope, &keys.owner)?;
        // 写しをまたぐ要求には、新しい写しから読み直した更新世代を使う。
        if pending
            .iter()
            .any(|(_, write)| write.epoch != scope.sync_epoch())
            && !self.pull(pc, scope, &keys.epoch).await?
        {
            return Ok(());
        }
        // 自分の書き込みの続きを書き直したら、もう一度だけ送る。
        if self.push(pc, scope, &keys, pending).await? {
            pending = self.pending(pc, scope, &keys.owner)?;
            self.push(pc, scope, &keys, pending).await?;
        }
        self.pull(pc, scope, &keys.epoch).await?;
        Ok(())
    }

    fn pending(
        &self,
        pc: &PcClientState,
        scope: ShelfScope,
        key: &str,
    ) -> Result<Vec<(String, PendingWrite)>, ClientError> {
        self.with_current_scope(pc, scope, |tx| {
            let mut query = tx
                .prepare("SELECT work, data FROM shelf_pending WHERE scope = ?1 ORDER BY work")
                .map_err(records_error)?;
            let rows = query
                .query_map([key], |row| {
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
        })
    }

    fn prepare_write(
        &self,
        pc: &PcClientState,
        scope: ShelfScope,
        keys: &ReadingKeys,
        work: &str,
        mut write: PendingWrite,
    ) -> Result<Option<PendingWrite>, ClientError> {
        self.with_current_scope(pc, scope, |tx| {
            let current: Option<PendingWrite> = query_json(tx, "shelf_pending", &keys.owner, work)?;
            if current.as_ref() != Some(&write) {
                return Ok(None);
            }
            if write.epoch != scope.sync_epoch() {
                write.epoch = scope.sync_epoch();
                write.expected = known_generation(tx, &keys.epoch, work)?;
                write.request = uuid::Uuid::now_v7().to_string();
                put_json(tx, "shelf_pending", &keys.owner, work, &write)?;
            }
            Ok(Some(write))
        })
    }

    /// 溜まった書き込みを送る。書き直した書き込みがあれば true。
    async fn push(
        &self,
        pc: &PcClientState,
        scope: ShelfScope,
        keys: &ReadingKeys,
        pending: Vec<(String, PendingWrite)>,
    ) -> Result<bool, ClientError> {
        let mut rewritten = false;
        for (work, write) in pending {
            let Some(write) = self.prepare_write(pc, scope, keys, &work, write)? else {
                continue;
            };
            let sent = send(pc, scope, &work, &write).await?;
            rewritten |= self.apply_sent(pc, scope, keys, &work, &write, sent)?;
        }
        Ok(rewritten)
    }

    fn apply_sent(
        &self,
        pc: &PcClientState,
        scope: ShelfScope,
        keys: &ReadingKeys,
        work: &str,
        write: &PendingWrite,
        sent: Sent,
    ) -> Result<bool, ClientError> {
        self.with_current_scope(pc, scope, |tx| match &sent {
            Sent::Stored(stored) => {
                accept(tx, &keys.epoch, stored)?;
                forget_sent(tx, &keys.owner, work, write)?;
                Ok(false)
            }
            Sent::Conflict(current) => {
                accept(tx, &keys.epoch, current)?;
                if !forget_sent(tx, &keys.owner, work, write)? {
                    return Ok(false);
                }
                if !current.mine {
                    record_rejection(tx, &keys.owner, work, write, "reading_conflict")?;
                    return Ok(false);
                }
                // 先に届いていたのはこの端末の前の書き込み。今の世代で書き直す。
                put_json(
                    tx,
                    "shelf_pending",
                    &keys.owner,
                    work,
                    &PendingWrite {
                        request: uuid::Uuid::now_v7().to_string(),
                        expected: current.generation.clone(),
                        ..write.clone()
                    },
                )?;
                Ok(true)
            }
            Sent::Refused(reason) => {
                if forget_sent(tx, &keys.owner, work, write)? {
                    record_rejection(tx, &keys.owner, work, write, reason)?;
                }
                Ok(false)
            }
        })
    }

    /// 前回の続きから、棚の位置の変わった分を読む。最後のページまで読めば true。
    async fn pull(
        &self,
        pc: &PcClientState,
        scope: ShelfScope,
        key: &str,
    ) -> Result<bool, ClientError> {
        for _ in 0..MAX_PULL_PAGES {
            let cursor = self.with_current_scope(pc, scope, |tx| {
                let saved: Option<String> = tx
                    .query_row(
                        "SELECT cursor FROM shelf_reading_cursors WHERE scope = ?1",
                        [key],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(records_error)?;
                Ok(saved.unwrap_or_default())
            })?;
            let Some(page) = fetch(pc, scope, &cursor).await? else {
                // 印が古くなった（サーバーの写しが変わった）。初めから取り直す。
                self.with_current_scope(pc, scope, |tx| {
                    tx.execute("DELETE FROM shelf_reading_cursors WHERE scope = ?1", [key])
                        .map_err(records_error)?;
                    Ok(())
                })?;
                if cursor.is_empty() {
                    return Err(ClientError::protocol());
                }
                continue;
            };
            self.with_current_scope(pc, scope, |tx| {
                for item in &page.items {
                    accept(tx, key, item)?;
                }
                tx.execute(
                    "INSERT INTO shelf_reading_cursors(scope, cursor) VALUES(?1, ?2)
                     ON CONFLICT(scope) DO UPDATE SET cursor=excluded.cursor",
                    [key, &page.cursor],
                )
                .map_err(records_error)?;
                Ok(())
            })?;
            if !page.more {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn connection(
    pc: &PcClientState,
    scope: ShelfScope,
) -> Result<(super::SavedConnection, reqwest::Client), ClientError> {
    let saved = pc.with_shelf_scope(scope, || pc.load())?;
    let saved = saved.ok_or_else(ClientError::invalid_pairing)?;
    let client = pc.client(&saved)?;
    Ok((saved, client))
}

async fn send(
    pc: &PcClientState,
    scope: ShelfScope,
    work: &str,
    write: &PendingWrite,
) -> Result<Sent, ClientError> {
    let (saved, client) = connection(pc, scope)?;
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
        .header("x-piep-subject-id", scope.subject_id().get().to_string())
        .header("x-piep-sync-epoch", scope.sync_epoch().get().to_string())
        .timeout(REQUEST_TIMEOUT)
        .json(&body)
        .send()
        .await
        .map_err(|error| ClientError::from_send(&error, saved.pinned()))?;
    let status = response.status();
    let sent = match status {
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
            let bytes = bounded_response_bytes(response, MAX_IDENTITY_BYTES).await?;
            let refusal: Refusal =
                serde_json::from_slice(&bytes).map_err(|_| ClientError::protocol())?;
            match (status, refusal.error.as_str()) {
                (StatusCode::BAD_REQUEST, "invalid_reading_location") => {
                    Sent::Refused("invalid_reading_location")
                }
                (StatusCode::NOT_FOUND, "resource_unavailable") => {
                    Sent::Refused("resource_unavailable")
                }
                _ => return Err(ClientError::protocol()),
            }
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
    Ok(sent)
}

/// 一ページ読む。印が古いと断られたら None。
async fn fetch(
    pc: &PcClientState,
    scope: ShelfScope,
    cursor: &str,
) -> Result<Option<MobileReadingPage>, ClientError> {
    let (saved, client) = connection(pc, scope)?;
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
        let bytes = bounded_response_bytes(response, MAX_IDENTITY_BYTES).await?;
        let refusal: Refusal =
            serde_json::from_slice(&bytes).map_err(|_| ClientError::protocol())?;
        if refusal.error == "reading_cursor_expired" {
            return Ok(None);
        }
        return Err(ClientError::protocol());
    }
    let bytes = bounded_response_bytes(check_scope(response, scope)?, MAX_MANIFEST_BYTES).await?;
    let page: MobileReadingPage =
        serde_json::from_slice(&bytes).map_err(|_| ClientError::protocol())?;
    if page.cursor.is_empty() || page.cursor.len() > 256 || !page.items.iter().all(valid_reading) {
        return Err(ClientError::protocol());
    }
    Ok(Some(page))
}

/// 読んだ位置を端末に置く。通信はしない。
#[tauri::command]
pub async fn pc_reading_record(
    state: State<'_, ShelfReadingState>,
    pc: State<'_, PcClientState>,
    scope: ShelfScope,
    work: Id,
    location: MobileReadingLocation,
    finished: bool,
) -> Result<(), ClientError> {
    pc.with_shelf_scope(scope, || state.record(&scope, work, location, finished))
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
    pc: State<'_, PcClientState>,
    scope: ShelfScope,
    work: Id,
) -> Result<Option<MobileReading>, ClientError> {
    pc.with_shelf_scope(scope, || state.get(&scope, work))
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
            .db
            .transaction(|tx| {
                query_json(
                    tx,
                    "shelf_pending",
                    &ReadingKeys::new(*scope).owner,
                    &work.get().to_string(),
                )
            })
            .unwrap()
    }

    fn cache(state: &ShelfReadingState, scope: ShelfScope, position: &MobileReading) {
        state
            .db
            .transaction(|tx| accept(tx, &ReadingKeys::new(scope).epoch, position))
            .unwrap();
    }

    #[cfg(not(target_os = "android"))]
    fn client(scope: ShelfScope, generation: u64) -> PcClientState {
        let pc = PcClientState::new(
            "unused".into(),
            Arc::new(crate::secret_cipher::DevelopmentCipher),
        );
        *pc.connection_generation.lock().unwrap() = generation;
        *pc.shelf_scope.lock().unwrap() = Some(scope);
        pc
    }

    #[test]
    fn a_write_takes_the_known_generation_and_the_newest_reading_replaces_it() {
        let state = ShelfReadingState::new(Arc::new(DeviceDb::in_memory()));
        let scope = ShelfScope::for_tests(id(), id(), id(), 1);
        let work = id();
        state.record(&scope, work, location(0), false).unwrap();
        let first = pending(&state, &scope, work).unwrap();
        assert_eq!(first.expected, "0");
        cache(&state, scope, &reading(work, "4", false));
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
        let keys = ReadingKeys::new(scope);
        let work_key = work.get().to_string();
        state
            .db
            .transaction(|tx| forget_sent(tx, &keys.owner, &work_key, &first))
            .unwrap();
        assert_eq!(pending(&state, &scope, work), Some(second.clone()));
        state
            .db
            .transaction(|tx| forget_sent(tx, &keys.owner, &work_key, &second))
            .unwrap();
        assert_eq!(pending(&state, &scope, work), None);
    }

    #[test]
    fn an_older_generation_does_not_replace_a_newer_one_and_another_epoch_starts_empty() {
        let state = ShelfReadingState::new(Arc::new(DeviceDb::in_memory()));
        let (library, subject) = (id(), id());
        let scope = ShelfScope::for_tests(library, subject, id(), 1);
        let work = id();
        cache(&state, scope, &reading(work, "5", false));
        cache(&state, scope, &reading(work, "3", true));
        assert_eq!(state.get(&scope, work).unwrap().unwrap().generation, "5");
        state.record(&scope, work, location(0), false).unwrap();
        // 接続を作り直しただけ（同じ棚・利用者・同期の世代）なら残る。
        let reconnected = ShelfScope::for_tests(library, subject, scope.sync_epoch(), 2);
        assert!(state.get(&reconnected, work).unwrap().is_some());
        assert!(pending(&state, &reconnected, work).is_some());
        // 同じ棚の新しい同期世代では位置を読み直し、未送信の要求は残す。
        let restored = ShelfScope::for_tests(library, subject, id(), 2);
        assert!(state.get(&restored, work).unwrap().is_none());
        assert!(pending(&state, &restored, work).is_some());
    }

    #[test]
    fn switching_shelf_or_subject_keeps_the_original_pending_write() {
        let state = ShelfReadingState::new(Arc::new(DeviceDb::in_memory()));
        let (library, subject, epoch) = (id(), id(), id());
        let scope = ShelfScope::for_tests(library, subject, epoch, 1);
        let work = id();
        state.record(&scope, work, location(1), true).unwrap();
        let original = pending(&state, &scope, work).unwrap();
        for other in [
            ShelfScope::for_tests(id(), subject, id(), 2),
            ShelfScope::for_tests(library, id(), epoch, 3),
        ] {
            assert!(pending(&state, &other, work).is_none());
            state.record(&other, work, location(2), false).unwrap();
        }
        assert_eq!(pending(&state, &scope, work), Some(original));
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn a_delayed_sync_cannot_discard_the_current_connections_reading() {
        let state = ShelfReadingState::new(Arc::new(DeviceDb::in_memory()));
        let (library, subject) = (id(), id());
        let old = ShelfScope::for_tests(library, subject, id(), 1);
        let current = ShelfScope::for_tests(library, subject, id(), 2);
        let pc = client(current, 2);
        let work = id();
        state.record(&current, work, location(1), true).unwrap();
        cache(&state, current, &reading(work, "5", false));
        let result = tauri::async_runtime::block_on(state.sync(&pc, old));
        assert!(result.is_err());
        assert_eq!(state.get(&current, work).unwrap().unwrap().generation, "5");
        assert!(pending(&state, &current, work).is_some());
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn a_response_from_the_previous_epoch_cannot_acknowledge_a_pending_write() {
        let state = ShelfReadingState::new(Arc::new(DeviceDb::in_memory()));
        let (library, subject) = (id(), id());
        let old = ShelfScope::for_tests(library, subject, id(), 1);
        let current = ShelfScope::for_tests(library, subject, id(), 1);
        let pc = client(current, 1);
        let work = id();
        state.record(&old, work, location(1), true).unwrap();
        let write = pending(&state, &old, work).unwrap();
        let result = state.apply_sent(
            &pc,
            old,
            &ReadingKeys::new(old),
            &work.get().to_string(),
            &write,
            Sent::Stored(reading(work, "1", true)),
        );
        assert_eq!(result.unwrap_err().code, "library_changed");
        assert_eq!(pending(&state, &current, work), Some(write));
        assert!(state.get(&old, work).unwrap().is_none());
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn replay_uses_the_new_epochs_generation_and_preserves_the_refusal_reason() {
        let state = ShelfReadingState::new(Arc::new(DeviceDb::in_memory()));
        let (library, subject) = (id(), id());
        let old = ShelfScope::for_tests(library, subject, id(), 1);
        let current = ShelfScope::for_tests(library, subject, id(), 2);
        let pc = client(current, 2);
        let (work, another) = (id(), id());
        cache(&state, old, &reading(work, "8", false));
        state.record(&old, work, location(1), true).unwrap();
        state.record(&old, another, location(2), false).unwrap();
        let original = pending(&state, &old, work).unwrap();
        let keys = ReadingKeys::new(current);
        let work_key = work.get().to_string();
        cache(&state, current, &reading(work, "3", false));
        let replay = state
            .prepare_write(&pc, current, &keys, &work_key, original.clone())
            .unwrap()
            .unwrap();
        assert_eq!(replay.epoch, current.sync_epoch());
        assert_eq!(replay.expected, "3");
        assert_ne!(replay.request, original.request);
        assert_eq!(replay.location, original.location);
        assert_eq!(replay.finished, original.finished);
        assert_eq!(pending(&state, &current, work), Some(replay.clone()));
        let retry = state
            .prepare_write(&pc, current, &keys, &work_key, replay.clone())
            .unwrap()
            .unwrap();
        assert_eq!(retry, replay);
        let refusal = || Sent::Refused("invalid_reading_location");
        state
            .apply_sent(&pc, current, &keys, &work_key, &replay, refusal())
            .unwrap();
        assert!(pending(&state, &current, work).is_none());
        assert!(pending(&state, &current, another).is_some());
        let (request, reason): (String, String) = state
            .db
            .transaction(|tx| {
                tx.query_row(
                "SELECT request, reason FROM shelf_reading_rejections WHERE scope=?1 AND work=?2",
                [&keys.owner, &work_key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).map_err(records_error)
            })
            .unwrap();
        assert_eq!(request, replay.request);
        assert_eq!(reason, "invalid_reading_location");
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
