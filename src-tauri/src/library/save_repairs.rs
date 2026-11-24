//! 更新確認が見つけた、作品の属性ではない事実：まだ取り込んでいない改稿と、一部保存の補完。
//!
//! 補完の回数は、取得元によらない補完の記録（`repair_attempts`）に書く。同じ版・同じ欠けでは
//! 上限まで。版が進めば、前の版の記録は保存と一緒に消える。

use super::{repair_attempts, Error, Library, LibraryId, Result};
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

    /// 取得元から取り直せば補える見込みのある一部保存の pixiv 作品を `(作品 ID, 題名, 今の版)` で
    /// 返す。今の版で補完の上限に届いていないものだけ。試した回数は、ここで返した版に数える。
    pub fn incomplete_save_repairs(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, String, LibraryId)>> {
        let limit = limit.clamp(1, MAX_INCOMPLETE_SAVE_REPAIRS) as i64;
        let conn = self.reader()?;
        let mut statement = conn.prepare(
            "SELECT p.external_key,p.title,p.snapshot_id FROM discovery_candidates c
               JOIN work_current_projection p ON p.work_id=c.work_id
              WHERE p.provider='pixiv' AND c.decision='held' AND c.hold_kind='incomplete'
                AND NOT EXISTS(SELECT 1 FROM repair_attempts a
                                WHERE a.work_id=c.work_id AND a.snapshot_id=p.snapshot_id
                                  AND a.attempts>=?2)
              ORDER BY c.work_id
              LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit, repair_attempts::limit(&conn)?], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get::<_, Vec<u8>>(2)?))
        })?;
        rows.map(|row| {
            let (key, title, snapshot) = row?;
            let snapshot = LibraryId::from_slice(&snapshot)
                .map_err(|_| Error::Integrity("保存版の ID が不正です".into()))?;
            Ok((key, title, snapshot))
        })
        .collect()
    }

    /// 補完の保存ジョブへ渡した pixiv 作品を、候補として返した版（`incomplete_save_repairs`）で
    /// 一回試したと数える。結果を待たずに数え、取り直して新しい版になれば、その保存で記録は消える。
    /// 印を付けるまでにジョブが新しい版を保存していたら数えない（新しい版は次の候補に残る）。
    /// 棚に無い作品・その作品の今の版でないものは飛ばす。数えた件数を返す。
    ///
    /// 欠けの指紋は、見られなかった挿絵の参照と伏せられた作品情報から作る。補完は保存ジョブの
    /// 外で数えるので、欠けた版を作った最初の保存も 1 回目として数える（FANBOX と同じ数え方）。
    pub fn record_incomplete_save_attempts(
        &self,
        attempts: &[(String, LibraryId)],
        at: i64,
    ) -> Result<usize> {
        let mut tried = Vec::new();
        for (key, snapshot) in attempts {
            let work = self.reader()?.query_row(
                "SELECT work_id FROM work_current_projection
                  WHERE provider='pixiv' AND external_key=?1 AND snapshot_id=?2",
                params![key, snapshot.as_bytes().as_slice()],
                |row| row.get::<_, Vec<u8>>(0),
            );
            let work = match work {
                Ok(work) => LibraryId::from_slice(&work)
                    .map_err(|_| Error::Integrity("作品 ID が不正です".into()))?,
                Err(rusqlite::Error::QueryReturnedNoRows) => continue,
                Err(error) => return Err(error.into()),
            };
            tried.push((work, *snapshot, self.pixiv_gaps(work, *snapshot)?));
        }
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let mut count = 0;
        for (work, snapshot, gaps) in tried {
            let current: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM entity_current WHERE entity_id=?1 AND snapshot_id=?2)",
                params![work.as_bytes().as_slice(), snapshot.as_bytes().as_slice()],
                |row| row.get(0),
            )?;
            if !current {
                continue;
            }
            let fingerprint = repair_attempts::gap_fingerprint(&tx, snapshot, &gaps)?;
            repair_attempts::record_retry(&tx, work, snapshot, fingerprint, at)?;
            count += 1;
        }
        tx.commit()?;
        Ok(count)
    }

    /// pixiv の版の欠け：表示できない挿絵の参照と、取得元で伏せられていた作品情報。
    fn pixiv_gaps(&self, work: LibraryId, snapshot: LibraryId) -> Result<Vec<String>> {
        let mut gaps = match self.pixiv_source_image_gaps(work, snapshot) {
            Ok(gaps) => gaps,
            Err(Error::ResourceUnavailable) => Vec::new(),
            Err(error) => return Err(error),
        };
        let original = match self.work_original(work, snapshot) {
            Ok(original) => original,
            Err(Error::ResourceUnavailable) => None,
            Err(error) => return Err(error),
        };
        if let Some(original) = original {
            let value = serde_json::from_str::<serde_json::Value>(&original)?;
            gaps.extend(
                super::masked_pixiv_metadata(&value)
                    .into_iter()
                    .map(|field| format!("metadata:{field}")),
            );
        }
        Ok(gaps)
    }
}
