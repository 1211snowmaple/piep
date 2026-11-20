//! 棚の写しを別の機械（単独サーバー）へ送る。
//!
//! 送り手は台帳（DB）の写しを一つ作り、その台帳が指す実体（部分実体）の一覧を出す。受け手は
//! まだ持っていない実体だけを受け取り、台帳の写しと、受け取った実体・いま配っている棚の実体から、
//! 検証済みの棚を別の場所に組む。実体は不変なので、いま配っている棚の実体はハードリンクで共有し、
//! 送るたびに棚全体を写し直さない。組んだ棚は全体を検査してから公開する。
//!
//! 写しを入れ替える前に携帯が書いた読書位置は、組んだ棚へ当て直す（`adopt_newer_positions`）。

use super::{
    store::{regular_directory, sync_directory},
    Error, FileChunk, Fingerprint, Library, LibraryId, ObjectStore, ReadingPosition, ReadingUpdate,
    Result,
};
use rusqlite::{Connection, OpenFlags};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

impl Library {
    /// 送るべき変更があったかを見る印（記録した操作の通し番号）。読書位置だけの変更は数えない
    /// （送るたびに台帳を写すので、頁をめくるたびには送らない。次に送るときに一緒に届く）。
    pub fn change_marker(&self) -> Result<i64> {
        Ok(self.reader()?.query_row(
            "SELECT coalesce(max(sequence),0) FROM outbox WHERE operation<>'reading_position'",
            [],
            |row| row.get(0),
        )?)
    }

    /// 送る写しの台帳を `database`（まだ無いファイル）へ書き、その台帳が指す実体を返す
    /// （重ねずに識別子の順）。読み取りの接続から写すので、保存を止めない。
    pub fn ship_snapshot(&self, database: &Path) -> Result<Vec<FileChunk>> {
        if database.try_exists()? {
            return Err(Error::UnsafePath(database.into()));
        }
        {
            let source = self.reader()?;
            let mut target = Connection::open(database)?;
            // 一度の手順で全部を写す（途中で書かれても写し直しにならない）。
            let copying = rusqlite::backup::Backup::new(&source, &mut target)?;
            copying.run_to_completion(i32::MAX, std::time::Duration::from_millis(0), None)?;
            drop(copying);
            target.execute_batch("PRAGMA journal_mode=DELETE;")?;
        }
        let conn = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        chunks_of(&conn)
    }

    /// 受け取った台帳と実体から、検証済みの棚を `destination`（まだ無い場所）に組む。
    /// 実体は `received`（受け取ったもの）か `previous`（いま配っている棚）から取る。どちらにも
    /// 無い実体があれば組まない。組んだ棚の作品の数を返す。
    pub fn assemble_shipment(
        destination: &Path,
        database: &Path,
        received: &ObjectStore,
        previous: Option<&ObjectStore>,
        expected: LibraryId,
    ) -> Result<u64> {
        let parent = destination
            .parent()
            .ok_or_else(|| Error::UnsafePath(destination.into()))?;
        regular_directory(parent)?;
        if destination.try_exists()? {
            return Err(Error::UnsafePath(destination.into()));
        }
        super::store::regular_file(database)?;
        let stage = parent.join(format!(".piep-shipment-{}.part", LibraryId::now_v7()));
        let assembled = (|| {
            let assembled = Library::create_backup_stage(&stage, expected)?;
            {
                let source =
                    Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
                let mut target = assembled.conn.lock().map_err(|_| Error::Busy)?;
                let copying = rusqlite::backup::Backup::new(&source, &mut target)?;
                copying.run_to_completion(i32::MAX, std::time::Duration::from_millis(0), None)?;
                drop(copying);
                // 送られた台帳が同じ棚のものか（形式の版も）をここで確かめる。
                super::schema::open(&target, expected)?;
            }
            let chunks = {
                let conn = assembled.conn.lock().map_err(|_| Error::Busy)?;
                chunks_of(&conn)?
            };
            for chunk in &chunks {
                if received.has_object(chunk.id)? {
                    assembled.objects.adopt_chunk(received, chunk)?;
                } else if let Some(previous) =
                    previous.filter(|p| p.has_object(chunk.id).unwrap_or(false))
                {
                    assembled.objects.adopt_chunk(previous, chunk)?;
                } else {
                    return Err(Error::Integrity(format!(
                        "台帳が指す実体が届いていません: {}",
                        chunk.id
                    )));
                }
            }
            assembled.verify()?;
            let works: i64 =
                assembled
                    .reader()?
                    .query_row("SELECT count(*) FROM library_items", [], |r| r.get(0))?;
            {
                let conn = assembled.conn.lock().map_err(|_| Error::Busy)?;
                conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            }
            Ok(u64::try_from(works).unwrap_or(0))
        })();
        let works = match assembled {
            Ok(works) => works,
            Err(error) => {
                let _ = fs::remove_dir_all(&stage);
                return Err(error);
            }
        };
        if destination.try_exists()? {
            let _ = fs::remove_dir_all(&stage);
            return Err(Error::UnsafePath(destination.into()));
        }
        fs::rename(&stage, destination)?;
        sync_directory(parent)?;
        fs::rename(
            destination.join(".backup-marker.json"),
            destination.join("library.json"),
        )?;
        sync_directory(destination)?;
        Ok(works)
    }

    /// ほかの棚（同じ棚の別の写し）の読書位置を、この棚へ写す。この棚の位置より後に書かれた
    /// ものだけを書き、書いた端末はそのまま残す。`skip_device` が書いた位置・この棚に載って
    /// いない作品・この棚の本文に無い位置は写さない。写したら真。
    pub fn adopt_reading(
        &self,
        work: LibraryId,
        user: LibraryId,
        other: &ReadingPosition,
        skip_device: Option<LibraryId>,
    ) -> Result<bool> {
        let Some(device) = other.device else {
            return Ok(false);
        };
        if other.generation == 0 || skip_device == Some(device) {
            return Ok(false);
        }
        if self.library_item_generation(work)?.is_none() {
            return Ok(false);
        }
        let local = self.reading_position(work, user)?;
        if local.location == other.location && local.finished == other.finished {
            return Ok(false);
        }
        // 二つの棚に共通の更新世代は無いので、どちらが後に読んだかは書いた時刻で決める。
        if other.updated_at.unwrap_or(0) <= local.updated_at.unwrap_or(0) {
            return Ok(false);
        }
        match self.set_reading_position(ReadingUpdate {
            request: LibraryId::now_v7(),
            work,
            user,
            device,
            expected: local.generation,
            location: other.location.clone(),
            finished: other.finished,
        }) {
            Ok(_) => Ok(true),
            // 本文に無い位置（この棚では別の版）や、いま書かれた位置との食い違い。写さない。
            Err(
                error @ (Error::Integrity(_)
                | Error::ResourceUnavailable
                | Error::Sql(rusqlite::Error::QueryReturnedNoRows)),
            ) => {
                log::warn!("ほかの写しの読書位置を写せません（{work}）: {error}");
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    /// 前の写し `older` の読書位置のうち、この棚より後に書かれたものを全部この棚へ写す。
    /// 写しを入れ替えるとき、送った後に携帯が書いた位置を失わないために使う。写した数を返す。
    pub fn adopt_newer_positions(&self, older: &Library) -> Result<usize> {
        let rows: Vec<(Vec<u8>, Vec<u8>)> = older
            .reader()?
            .prepare("SELECT work_id,user_id FROM reading_positions ORDER BY work_id,user_id")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        let mut adopted = 0;
        for (work, user) in rows {
            let (Ok(work), Ok(user)) = (LibraryId::from_slice(&work), LibraryId::from_slice(&user))
            else {
                continue;
            };
            let position = older.reading_position(work, user)?;
            if self.adopt_reading(work, user, &position, None)? {
                adopted += 1;
            }
        }
        Ok(adopted)
    }
}

/// 台帳が指す実体（重ねずに識別子の順）。
fn chunks_of(conn: &Connection) -> Result<Vec<FileChunk>> {
    let ids = conn
        .prepare("SELECT id FROM content_files ORDER BY id")?
        .query_map([], |r| r.get::<_, Vec<u8>>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut chunks = BTreeMap::new();
    for id in ids {
        let id: [u8; 32] = id
            .try_into()
            .map_err(|_| Error::Integrity("ファイル ID が不正です".into()))?;
        for chunk in Library::file_from_conn(conn, Fingerprint(id))?.chunks {
            chunks.entry(chunk.id.0).or_insert(chunk);
        }
    }
    Ok(chunks.into_values().collect())
}

#[cfg(test)]
#[path = "shipment_tests.rs"]
mod tests;
