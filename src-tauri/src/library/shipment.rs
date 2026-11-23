//! 棚の写しを別の機械（単独サーバー）へ送る。
//!
//! 送り手は台帳（DB）の写しを一つ作り、その台帳が指す実体（部分実体）の一覧を出す。受け手は
//! まだ持っていない実体だけを受け取り、台帳の写しと、受け取った実体・いま配っている棚の実体から、
//! 検証済みの棚を別の場所に組む。実体は不変なので、いま配っている棚の実体はハードリンクで共有し、
//! 送るたびに棚全体を写し直さない。組んだ棚は全体を検査してから公開する。
//!
//! 写しを入れ替える前に携帯が書いた読書位置は、組んだ棚へ当て直す（`adopt_from_previous`）。

use super::{
    store::{regular_directory, sync_directory},
    Error, FileChunk, Fingerprint, Library, LibraryId, ObjectStore, ReadingPosition, ReadingUpdate,
    Result,
};
use rusqlite::{Connection, OpenFlags};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::Arc;

/// 組んだ棚の数え。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assembled {
    pub works: u64,
    /// 棚のファイルの数と、そのうち中身を読み直して確かめた数。
    pub files: u64,
    pub read: u64,
}

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
    /// （重ねずに識別子の順）。返した一覧が生きている間、回収からその実体を守る。
    /// 読み取りの接続から写すので、ふつうの保存を止めない。
    pub fn ship_snapshot(&self, database: &Path) -> Result<Arc<[FileChunk]>> {
        let _turn = self.storage_gate.lock().map_err(|_| Error::Busy)?;
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
        let chunks: Arc<[_]> = chunks_of(&conn)?.into();
        let mut shipments = self.shipments.lock().map_err(|_| Error::Busy)?;
        shipments.retain(|pin| pin.strong_count() != 0);
        shipments.push(Arc::downgrade(&chunks));
        Ok(chunks)
    }

    /// 受け取った台帳と実体から、検証済みの棚を `destination`（まだ無い場所）に組む。
    /// 実体は `received`（受け取ったもの）か `previous`（いま配っている棚）から取る。どちらにも
    /// 無い実体があれば組まない。組んだ棚の作品の数を返す。
    ///
    /// 検査は台帳を全部確かめるが、ファイルの中身を読み直すのは、新しく受け取った実体を含む
    /// ファイルと、配っている棚と構成が違うファイルだけ（配っている棚は配る前に検査を通しており、
    /// 実体は不変でハードリンクで共有する）。全棚で毎回の全体の読み直しは十数分かかる。
    pub fn assemble_shipment(
        destination: &Path,
        database: &Path,
        received: &ObjectStore,
        previous: Option<&Library>,
        expected: LibraryId,
    ) -> Result<Assembled> {
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
            let mut fresh = HashSet::new();
            for chunk in &chunks {
                if received.has_object(chunk.id)? {
                    assembled.objects.adopt_chunk(received, chunk)?;
                    fresh.insert(chunk.id);
                } else if let Some(previous) =
                    previous.filter(|p| p.objects.has_object(chunk.id).unwrap_or(false))
                {
                    assembled.objects.adopt_chunk(&previous.objects, chunk)?;
                } else {
                    return Err(Error::Integrity(format!(
                        "台帳が指す実体が届いていません: {}",
                        chunk.id
                    )));
                }
            }
            let before = previous.map(Library::reader).transpose()?;
            let (files, read) = assembled.verify_except(|file| {
                let Some(before) = &before else {
                    return false;
                };
                !file.chunks.iter().any(|chunk| fresh.contains(&chunk.id))
                    && Library::file_from_conn(before, file.id).is_ok_and(|old| old == *file)
            })?;
            drop(before);
            log::info!("送られた棚を検査しました（ファイル {files}、読み直し {read}）");
            let works: i64 =
                assembled
                    .reader()?
                    .query_row("SELECT count(*) FROM library_items", [], |r| r.get(0))?;
            {
                let conn = assembled.conn.lock().map_err(|_| Error::Busy)?;
                conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            }
            Ok(Assembled {
                works: u64::try_from(works).unwrap_or(0),
                files,
                read,
            })
        })();
        let assembled = match assembled {
            Ok(assembled) => assembled,
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
        Ok(assembled)
    }

    /// ほかの棚（同じ棚の別の写し）の読書位置を、この棚へ写す。書いた端末はそのまま残す。
    /// `skip_device` が書いた位置・この棚に載っていない作品・この棚の本文に無い位置は写さない。
    ///
    /// `here` は、この棚自身が前に送ってからこの作品の位置を変えたか（`changed_here_since`）。
    /// 変えていなければ（`Some(false)`）ほかの写しの位置を写す。両方で変えた・分からないときだけ、
    /// 書いた時刻の新しいほうにする（機械ごとの時計のずれは、この食い違いのときだけ効く）。
    /// 写したら真。
    pub fn adopt_reading(
        &self,
        work: LibraryId,
        user: LibraryId,
        other: &ReadingPosition,
        skip_device: Option<LibraryId>,
        here: Option<bool>,
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
        if here != Some(false) && other.updated_at.unwrap_or(0) <= local.updated_at.unwrap_or(0) {
            return Ok(false);
        }
        match self.set_reading_position(ReadingUpdate {
            request: adopted_request(),
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

    /// ほかの写しのお気に入りを、この棚へ写す。この棚自身が前に送ってから変えた・分からない
    /// （`here` が `Some(false)` でない）ときはこの棚の値を残す（お気に入りには書いた時刻が無く、
    /// 初めの一覧は送った写しの古い値も返す）。写したら真。
    pub fn adopt_favorite(
        &self,
        work: LibraryId,
        user: LibraryId,
        favorite: bool,
        here: Option<bool>,
    ) -> Result<bool> {
        if here != Some(false) || self.library_item_generation(work)?.is_none() {
            return Ok(false);
        }
        let local = self.user_state(work, user)?;
        if local.favorite == favorite {
            return Ok(false);
        }
        match self.set_favorite(adopted_request(), work, user, local.generation, favorite) {
            Ok(_) => Ok(true),
            Err(error @ Error::Integrity(_)) => {
                log::warn!("ほかの写しのお気に入りを写せません（{work}）: {error}");
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    /// 棚の変更の記録の、今の最後の番号。
    pub fn outbox_sequence(&self) -> Result<i64> {
        Ok(self
            .reader()?
            .query_row("SELECT coalesce(max(sequence),0) FROM outbox", [], |row| {
                row.get(0)
            })?)
    }

    /// 番号 `since` より後に、この棚自身が（ほかの写しから写したのではなく）作品の
    /// `operation`（`reading_position`・`user_state`）を変えたか。
    pub fn changed_here_since(&self, work: LibraryId, operation: &str, since: i64) -> Result<bool> {
        Ok(self.reader()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM outbox WHERE entity_id=?1 AND operation=?2 AND sequence>?3
               AND substr(hex(request_id),13,1)<>'8')",
            rusqlite::params![work.as_bytes().as_slice(), operation, since],
            |row| row.get(0),
        )?)
    }

    /// 前の写し `older` に、送ったあとで書かれた読書位置とお気に入りを、この棚へ写す。
    /// 写しを入れ替えるとき、送ったあとに携帯が書いたものを失わないために使う。
    ///
    /// 送ったあとに書かれた変更は、前の写しの記録にあってこの棚の記録に無い要求のもの
    /// （この棚は送り手の台帳の写しで、送り手の記録を全部持つ）。この棚に前の写しに無い
    /// 送り手自身の変更があれば、その作品は両方で変えたものとして扱う。写した数（位置、
    /// お気に入り）を返す。
    pub fn adopt_from_previous(&self, older: &Library) -> Result<(usize, usize)> {
        let requests = |library: &Library| -> Result<Vec<Request>> {
            Ok(library
                .reader()?
                .prepare(
                    "SELECT request_id,entity_id,operation FROM outbox
                     WHERE operation IN ('reading_position','user_state')",
                )?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<std::result::Result<_, _>>()?)
        };
        let theirs = requests(older)?;
        let ours = requests(self)?;
        let known: HashSet<&Vec<u8>> = ours.iter().map(|(request, _, _)| request).collect();
        let before: HashSet<&Vec<u8>> = theirs.iter().map(|(request, _, _)| request).collect();
        // 送り手が前に送ってから自分で変えたもの（写したものは要求 ID の版で除く）。
        let here: HashSet<(&Vec<u8>, &str)> = ours
            .iter()
            .filter(|(request, _, _)| !before.contains(request) && !is_adopted(request))
            .map(|(_, work, operation)| (work, operation.as_str()))
            .collect();
        let mut changed: BTreeMap<(Vec<u8>, String), ()> = BTreeMap::new();
        for (request, work, operation) in &theirs {
            if !known.contains(request) {
                changed.insert((work.clone(), operation.clone()), ());
            }
        }
        let (mut positions, mut favorites) = (0, 0);
        let owner = self.id;
        for (work_bytes, operation) in changed.into_keys() {
            let Ok(work) = LibraryId::from_slice(&work_bytes) else {
                continue;
            };
            let both = here.contains(&(&work_bytes, operation.as_str()));
            if operation == "reading_position" {
                let users: Vec<Vec<u8>> = older
                    .reader()?
                    .prepare("SELECT user_id FROM reading_positions WHERE work_id=?1")?
                    .query_map([work_bytes.as_slice()], |r| r.get(0))?
                    .collect::<std::result::Result<_, _>>()?;
                for user in users {
                    let Ok(user) = LibraryId::from_slice(&user) else {
                        continue;
                    };
                    let position = older.reading_position(work, user)?;
                    if self.adopt_reading(work, user, &position, None, Some(both))? {
                        positions += 1;
                    }
                }
            } else if older.library_item_generation(work)?.is_some() {
                let favorite = older.user_state(work, owner)?.favorite;
                if self.adopt_favorite(work, owner, favorite, Some(both))? {
                    favorites += 1;
                }
            }
        }
        Ok((positions, favorites))
    }
}

/// 変更の記録の一行（要求 ID・作品・操作）。
type Request = (Vec<u8>, Vec<u8>, String);

/// ほかの写しから写した変更に使う要求 ID。版を 8 にして、自分で変えた要求（版 7・4）と
/// 記録の上で見分ける。
pub fn adopted_request() -> LibraryId {
    let mut bytes = *LibraryId::now_v7().as_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    LibraryId::from_bytes(bytes)
}

fn is_adopted(request: &[u8]) -> bool {
    request.len() == 16 && request[6] >> 4 == 8
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
