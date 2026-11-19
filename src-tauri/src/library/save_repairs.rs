//! 更新確認が見つけた、作品の属性ではない事実：まだ取り込んでいない改稿と、一部保存の補完。
//!
//! 補完は「今の版で一度だけ」試す。旧棚から移した印（`repair_attempt_markers`）は原記録として
//! 変えず、切り替えた後の試行は `incomplete_save_attempts` に書く。版（現在の写し）が進めば、
//! どちらの印も効かなくなる。

use super::{Error, Library, LibraryId, Result};
use rusqlite::params;

#[cfg(test)]
#[path = "save_repairs_tests.rs"]
mod tests;

/// 補完で取り直す pixiv 作品の上限（一度に返す件数）。
pub const MAX_INCOMPLETE_SAVE_REPAIRS: usize = 500;

impl Library {
    /// 改稿の候補がまだ判断されていない棚の作品と、更新確認がそれを見つけた時刻（ミリ秒）。
    /// 棚から外した作品は含めない。
    pub fn pending_revisions(&self) -> Result<Vec<(LibraryId, i64)>> {
        let conn = self.reader()?;
        let mut statement = conn.prepare(
            "SELECT c.work_id,c.updated_at FROM discovery_candidates c
               JOIN work_current_projection p ON p.work_id=c.work_id
              WHERE c.kind='revision' AND c.decision='pending'
              ORDER BY c.work_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.map(|row| {
            let (work, found) = row?;
            let work = LibraryId::from_slice(&work)
                .map_err(|_| Error::Integrity("候補の作品 ID が不正です".into()))?;
            Ok((work, found))
        })
        .collect()
    }

    /// 改稿の候補がまだ判断されていない棚の作品か。
    pub fn has_pending_revision(&self, work: LibraryId) -> Result<bool> {
        let conn = self.reader()?;
        Ok(conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM discovery_candidates c
               JOIN work_current_projection p ON p.work_id=c.work_id
              WHERE c.work_id=?1 AND c.kind='revision' AND c.decision='pending')",
            [work.as_bytes().as_slice()],
            |row| row.get(0),
        )?)
    }

    /// 取得元から取り直せば補える見込みのある一部保存の pixiv 作品を `(作品 ID, 題名)` で返す。
    /// 今の版でまだ試していないものだけ。
    pub fn incomplete_save_repairs(&self, limit: usize) -> Result<Vec<(String, String)>> {
        let limit = limit.clamp(1, MAX_INCOMPLETE_SAVE_REPAIRS) as i64;
        let conn = self.reader()?;
        let mut statement = conn.prepare(
            "SELECT p.external_key,p.title FROM discovery_candidates c
               JOIN work_current_projection p ON p.work_id=c.work_id
              WHERE p.provider='pixiv' AND c.decision='held' AND c.hold_kind='incomplete'
                AND NOT EXISTS(SELECT 1 FROM incomplete_save_attempts a
                                WHERE a.work_id=c.work_id AND a.snapshot_id=p.snapshot_id)
                AND NOT EXISTS(SELECT 1 FROM repair_attempt_markers m
                                WHERE m.entity_id=c.work_id AND m.snapshot_id=p.snapshot_id)
              ORDER BY c.work_id
              LIMIT ?1",
        )?;
        let rows = statement.query_map([limit], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// 補完の保存ジョブへ渡した pixiv 作品を、今の版で試したと記録する。結果がどうであれ、
    /// 同じ版では繰り返さない。棚に無い作品は飛ばす。記録した件数を返す。
    pub fn record_incomplete_save_attempts(&self, keys: &[String], at: i64) -> Result<usize> {
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let mut count = 0;
        for key in keys {
            count += tx.execute(
                "INSERT INTO incomplete_save_attempts(work_id,snapshot_id,attempted_at)
                 SELECT p.work_id,p.snapshot_id,?2 FROM work_current_projection p
                  WHERE p.provider='pixiv' AND p.external_key=?1
                 ON CONFLICT(work_id) DO UPDATE SET snapshot_id=excluded.snapshot_id,attempted_at=excluded.attempted_at",
                params![key, at],
            )?;
        }
        tx.commit()?;
        Ok(count)
    }
}
