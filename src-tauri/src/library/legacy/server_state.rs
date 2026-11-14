//! デスクトップ版の `server/`（内蔵サーバーの置き場）のうち、利用者の状態だけを移す。
//!
//! `server/state.db` には端末の登録と鍵、管理者のアカウントとセッション、監査の記録、
//! 送信の記録も入っている。これらは棚のデータではないので読まない。読むのは次の二つの表だけ。
//!
//! - `reading`: 携帯が記録した読書位置。作品ごとに一行。新しい棚の `reading_positions` へ移す。
//!   位置は旧系統の形（ページと割合）のまま `ReadingAnchor::Page` として持ち、文字の位置へ
//!   推測して直さない。版は保存状態へ、編集の印は編集版へ結ぶ。結べない行は未解決にする。
//! - `favorite_changes`: 携帯からのお気に入りの変更。内蔵サーバーは同じ流れで本棚の
//!   `downloads.favorite` にも書くので、正本はそちら（作品の移行で移す）。値が一致する行は
//!   写しとして済ませ、食い違う行だけを未解決にする。

use super::{rows::marked, LegacyImporter};
use crate::library::{
    reading::{ReadingAnchor, ReadingLocation, PAGE_RATIO_SCALE},
    Error, Library, LibraryId, Result,
};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use std::path::Path;

const READING: &str = "server_state.reading";
const FAVORITES: &str = "server_state.favorite_changes";

/// 内蔵サーバーが `server/` に作るもの。これ以外があれば、中身を推測せず未解決にする。
pub(super) fn known_server_entries(dir: &Path) -> Result<bool> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let known = matches!(
            name.to_str(),
            Some(
                "settings.json"
                    | "state.db"
                    | "state.db-wal"
                    | "state.db-shm"
                    | "state.db-journal"
                    | "outgoing"
                    | "tls"
            )
        );
        if !known {
            return Ok(false);
        }
    }
    Ok(true)
}

struct LegacyReading {
    source: String,
    source_id: String,
    version: Option<i64>,
    edit_hash: Option<String>,
    page: i64,
    position: f64,
    finished: bool,
    device_id: String,
    device_time: i64,
}

/// 旧い端末の識別子から、新しい棚の端末の識別子を決める。同じ端末は必ず同じ値になる。
fn device_id(legacy: &str) -> LibraryId {
    let digest = Sha256::digest(format!("piep-server-device\0{legacy}").as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_custom_bytes(bytes).into_uuid()
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |r| r.get(0),
    )?)
}

impl LegacyImporter {
    pub(super) fn normalize_server_state(&self, target: &Library) -> Result<()> {
        let path = self.root.join("server").join("state.db");
        if !path.try_exists()? {
            return Ok(());
        }
        {
            let conn = target.conn.lock().map_err(|_| Error::Busy)?;
            if marked(&conn, READING)? && marked(&conn, FAVORITES)? {
                return Ok(());
            }
        }
        let state = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        state.pragma_update(None, "query_only", true)?;
        let readings = if table_exists(&state, "reading")? {
            let mut stmt = state.prepare(
                "SELECT source,source_id,version,edit_hash,page,position,finished,device_id,device_time FROM reading ORDER BY cursor",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(LegacyReading {
                    source: r.get(0)?,
                    source_id: r.get(1)?,
                    version: r.get(2)?,
                    edit_hash: r.get(3)?,
                    page: r.get(4)?,
                    position: r.get(5)?,
                    finished: r.get(6)?,
                    device_id: r.get(7)?,
                    device_time: r.get(8)?,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        let favorites: Vec<(String, String, bool)> = if table_exists(&state, "favorite_changes")? {
            let mut stmt = state.prepare(
                "SELECT source,source_id,favorite FROM favorite_changes ORDER BY cursor",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        } else {
            Vec::new()
        };
        drop(state);

        // 本文の復元と位置の検査は、書き込みの錠を持たずに先に済ませる。
        let mut placed = Vec::with_capacity(readings.len());
        for reading in &readings {
            placed.push(self.place_reading(target, reading));
        }
        let mut favorite_checks = Vec::with_capacity(favorites.len());
        for (source, source_id, favorite) in &favorites {
            let current: Option<bool> = self
                .conn
                .query_row(
                    "SELECT favorite FROM downloads WHERE source=?1 AND source_id=?2",
                    params![source, source_id],
                    |r| r.get(0),
                )
                .optional()?;
            // 作品が棚から消えていれば、その変更に当てる先はない。
            favorite_checks.push(current.is_none_or(|current| current == *favorite));
        }

        let user = target.id;
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        if marked(&tx, READING)? && marked(&tx, FAVORITES)? {
            return Ok(());
        }
        for (reading, placed) in readings.iter().zip(placed) {
            let key = format!("{}:{}", reading.source, reading.source_id);
            match placed {
                Ok((work, location)) => {
                    let (snapshot, edit, anchor) = match &location {
                        ReadingLocation::Source { snapshot, anchor } => {
                            (Some(snapshot.as_bytes().to_vec()), None, anchor)
                        }
                        ReadingLocation::Edit { edit, anchor } => {
                            (None, Some(edit.as_bytes().to_vec()), anchor)
                        }
                    };
                    // 移行のやり直しで、移したあとの読書を上書きしない。
                    tx.execute(
                        "INSERT INTO reading_positions(work_id,user_id,snapshot_id,edit_id,anchor_json,cleared,finished,device_id,updated_at,generation) VALUES(?1,?2,?3,?4,?5,0,?6,?7,?8,1) ON CONFLICT(work_id,user_id) DO NOTHING",
                        params![
                            work.as_bytes().as_slice(),
                            user.as_bytes().as_slice(),
                            snapshot,
                            edit,
                            serde_json::to_string(anchor)?,
                            reading.finished,
                            device_id(&reading.device_id).as_bytes().as_slice(),
                            reading.device_time,
                        ],
                    )?;
                    tx.execute("INSERT INTO migration_records(source_table,source_key,target_id,status) VALUES(?1,?2,?3,'imported') ON CONFLICT(source_table,source_key) DO UPDATE SET target_id=excluded.target_id,status=excluded.status",params![READING,key,work.as_bytes().as_slice()])?;
                }
                Err(error) => {
                    log::warn!("読書位置を移せません（{key}）: {error}");
                    tx.execute("INSERT INTO migration_records(source_table,source_key,status) VALUES(?1,?2,'unresolved') ON CONFLICT(source_table,source_key) DO UPDATE SET status='unresolved'",params![READING,key])?;
                }
            }
        }
        for ((source, source_id, _), matches) in favorites.iter().zip(favorite_checks) {
            tx.execute("INSERT INTO migration_records(source_table,source_key,status) VALUES(?1,?2,?3) ON CONFLICT(source_table,source_key) DO UPDATE SET status=excluded.status",params![FAVORITES,format!("{source}:{source_id}"),if matches {"imported"} else {"unresolved"}])?;
        }
        for table in [READING, FAVORITES] {
            tx.execute("INSERT INTO migration_records(source_table,source_key,status) VALUES(?1,'__all_rows__','imported') ON CONFLICT(source_table,source_key) DO UPDATE SET status='imported'",[table])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 旧い読書位置を、新しい棚の作品と保存状態（または編集版）へ結ぶ。
    fn place_reading(
        &self,
        target: &Library,
        reading: &LegacyReading,
    ) -> Result<(LibraryId, ReadingLocation)> {
        let invalid = |message: &str| Error::Integrity(format!("読書位置: {message}"));
        let old: i64 = self
            .conn
            .query_row(
                "SELECT id FROM downloads WHERE source=?1 AND source_id=?2",
                params![reading.source, reading.source_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| invalid("作品が旧棚にありません"))?;
        if !(0.0..=1.0).contains(&reading.position) {
            return Err(invalid("ページの中の割合が 0〜1 にありません"));
        }
        let page = u64::try_from(reading.page).map_err(|_| invalid("ページが負です"))?;
        let anchor = ReadingAnchor::Page {
            page,
            ratio_ppm: (reading.position * f64::from(PAGE_RATIO_SCALE)).round() as u32,
        };
        let record = |table: &str, key: String| -> Result<Option<LibraryId>> {
            let conn = target.reader()?;
            let id: Option<Vec<u8>> = conn.query_row("SELECT target_id FROM migration_records WHERE source_table=?1 AND source_key=?2 AND status='imported'",params![table,key],|r|r.get(0)).optional()?.flatten();
            id.map(|id| {
                LibraryId::from_slice(&id).map_err(|_| invalid("移行の記録の識別子が不正です"))
            })
            .transpose()
        };
        let work: Vec<u8> = target.reader()?.query_row("SELECT entity_id FROM external_identities WHERE provider=?1 AND kind='work' AND external_key=?2",params![reading.source,reading.source_id],|r|r.get(0)).optional()?.ok_or_else(|| invalid("作品を移行できていません"))?;
        let work = LibraryId::from_slice(&work).map_err(|_| invalid("作品の識別子が不正です"))?;
        let location = match reading.edit_hash.as_deref().filter(|hash| !hash.is_empty()) {
            Some(hash) => {
                // 同じ中身の編集が複数あれば、今使っている編集を選ぶ。中身が同じなので位置は同じ。
                let edit: i64 = self.conn.query_row("SELECT id FROM work_edit_revisions WHERE download_id=?1 AND content_hash=?2 ORDER BY status='active' DESC,id DESC LIMIT 1",params![old,hash],|r|r.get(0)).optional()?.ok_or_else(|| invalid("編集の印に合う編集がありません"))?;
                let edit = record("work_edit_revisions", edit.to_string())?
                    .ok_or_else(|| invalid("編集を移行できていません"))?;
                ReadingLocation::Edit { edit, anchor }
            }
            None => {
                let number = reading.version.ok_or_else(|| invalid("版がありません"))?;
                let mut stmt = self.conn.prepare(
                    "SELECT id FROM download_versions WHERE download_id=?1 AND version=?2",
                )?;
                let ids = stmt
                    .query_map(params![old, number], |r| r.get::<_, i64>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let [version] = ids[..] else {
                    return Err(invalid("版が一意ではありません"));
                };
                let snapshot = record("download_versions", format!("{old}:{version}:{number}"))?
                    .ok_or_else(|| invalid("版を移行できていません"))?;
                ReadingLocation::Source { snapshot, anchor }
            }
        };
        target.check_reading_location(work, &location)?;
        Ok((work, location))
    }
}

#[cfg(test)]
#[path = "server_state_tests.rs"]
mod tests;
