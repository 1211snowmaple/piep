use super::{Error, Library, LibraryId, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// 公開元の改稿日時とは別の、取得や確認についてのアプリ側の記録。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceEntityState {
    pub entity: LibraryId,
    pub last_checked_at: Option<i64>,
    pub last_fetched_at: Option<i64>,
    pub last_deep_checked_at: Option<i64>,
}

impl Library {
    /// 確認した保存版がまだ現在版のときだけ確認時刻を進める。
    /// `deep` は取り寄せた本文がこの保存版と一致したときだけ指定する。
    pub fn record_work_check(
        &self,
        work: LibraryId,
        snapshot: LibraryId,
        checked_at: i64,
        deep: bool,
    ) -> Result<()> {
        if chrono::DateTime::from_timestamp_millis(checked_at).is_none() {
            return Err(Error::Integrity("取得元の確認日時が不正です".into()));
        }
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let current: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM entity_current c JOIN library_items li ON li.work_id=c.entity_id WHERE c.entity_id=?1 AND c.snapshot_id=?2)",
            rusqlite::params![work.as_bytes().as_slice(), snapshot.as_bytes().as_slice()],
            |r| r.get(0),
        )?;
        if !current {
            return Err(Error::Superseded);
        }
        tx.execute(
            "INSERT INTO source_entity_state(entity_id,last_checked_at,last_deep_checked_at) VALUES(?1,?2,?3)
             ON CONFLICT(entity_id) DO UPDATE SET
               last_checked_at=max(coalesce(last_checked_at,excluded.last_checked_at),excluded.last_checked_at),
               last_deep_checked_at=CASE WHEN excluded.last_deep_checked_at IS NULL THEN last_deep_checked_at
                 ELSE max(coalesce(last_deep_checked_at,excluded.last_deep_checked_at),excluded.last_deep_checked_at) END",
            rusqlite::params![work.as_bytes().as_slice(), checked_at, deep.then_some(checked_at)],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn source_entity_state(&self, entity: LibraryId) -> Result<SourceEntityState> {
        let conn = self.reader()?;
        Ok(conn.query_row("SELECT s.last_checked_at,s.last_fetched_at,s.last_deep_checked_at FROM entities e LEFT JOIN source_entity_state s ON s.entity_id=e.id WHERE e.id=?1 AND e.kind IN ('work','person','series')",[entity.as_bytes().as_slice()],|r|Ok(SourceEntityState {entity,last_checked_at:r.get(0)?,last_fetched_at:r.get(1)?,last_deep_checked_at:r.get(2)?}))?)
    }
}

pub(super) fn verify_source_state(conn: &Connection) -> Result<()> {
    let mut stmt=conn.prepare("SELECT e.kind,s.last_checked_at,s.last_fetched_at,s.last_deep_checked_at FROM source_entity_state s JOIN entities e ON e.id=s.entity_id")?;
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            [
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, Option<i64>>(3)?,
            ],
        ))
    })? {
        let (kind, dates) = row?;
        if !matches!(kind.as_str(), "work" | "person" | "series")
            || dates
                .into_iter()
                .flatten()
                .any(|date| chrono::DateTime::from_timestamp_millis(date).is_none())
        {
            return Err(Error::Integrity(
                "取得元の確認日時または所属が不正です".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::EntityKind;
    #[test]
    fn invalid_check_times_are_detected_and_the_database_rejects_wrong_or_changed_owners() {
        let root = std::env::temp_dir().join(format!("piep-check-state-{}", LibraryId::now_v7()));
        let library = Library::create(&root).unwrap();
        let work = library
            .begin_acquisition(
                LibraryId::now_v7(),
                "pixiv",
                EntityKind::Work,
                "one",
                "offline-test",
            )
            .unwrap()
            .ticket
            .entity;
        let other = library
            .begin_acquisition(
                LibraryId::now_v7(),
                "pixiv",
                EntityKind::Work,
                "other",
                "offline-test",
            )
            .unwrap()
            .ticket
            .entity;
        let collection = LibraryId::now_v7();
        {
            let conn = library.conn.lock().unwrap();
            conn.execute(
                "INSERT INTO entities(id,kind,created_at) VALUES(?1,'collection',0)",
                [collection.as_bytes().as_slice()],
            )
            .unwrap();
            assert!(conn
                .execute(
                    "INSERT INTO source_entity_state(entity_id) VALUES(?1)",
                    [collection.as_bytes().as_slice()]
                )
                .is_err());
            conn.execute(
                "INSERT INTO source_entity_state(entity_id,last_checked_at) VALUES(?1,1234)",
                [work.as_bytes().as_slice()],
            )
            .unwrap();
            assert!(conn
                .execute(
                    "UPDATE source_entity_state SET entity_id=?1",
                    [other.as_bytes().as_slice()]
                )
                .is_err());
        }
        library.verify().unwrap();
        library
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE source_entity_state SET last_deep_checked_at=?1",
                [i64::MAX],
            )
            .unwrap();
        assert!(matches!(library.verify(), Err(Error::Integrity(_))));
        drop(library);
        assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
        std::fs::remove_dir_all(root).unwrap();
    }
}
