use super::{Error, Fingerprint, Library, LibraryId, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepairAttemptMarker {
    pub id: LibraryId,
    pub entity: LibraryId,
    pub snapshot: LibraryId,
    pub origin: String,
    /// 試行の旧記録。要求の送信・完了・成功を保証しない。
    pub attempted_at: i64,
    pub evidence_file: Fingerprint,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssetAuditMarker {
    pub id: LibraryId,
    pub entity: LibraryId,
    pub snapshot: LibraryId,
    pub origin: String,
    /// 旧検査の規則。新形式での検査を合格済みにするための値ではない。
    pub legacy_rule: i64,
    pub evidence_file: Fingerprint,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MaintenanceHistory {
    pub entity: LibraryId,
    pub snapshot: LibraryId,
    pub repair_attempts: Vec<RepairAttemptMarker>,
    pub asset_audits: Vec<AssetAuditMarker>,
}

fn id(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<LibraryId> {
    let bytes: Vec<u8> = row.get(index)?;
    LibraryId::from_slice(&bytes).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Blob, Box::new(e))
    })
}
fn hash(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Fingerprint> {
    let bytes: Vec<u8> = row.get(index)?;
    bytes.try_into().map(Fingerprint).map_err(|_| {
        rusqlite::Error::InvalidColumnType(
            index,
            "evidence_file_id".into(),
            rusqlite::types::Type::Blob,
        )
    })
}
fn repair(row: &rusqlite::Row<'_>) -> rusqlite::Result<RepairAttemptMarker> {
    Ok(RepairAttemptMarker {
        id: id(row, 0)?,
        entity: id(row, 1)?,
        snapshot: id(row, 2)?,
        origin: row.get(3)?,
        attempted_at: row.get(4)?,
        evidence_file: hash(row, 5)?,
    })
}
fn revision(conn: &Connection, entity: LibraryId, snapshot: LibraryId) -> Result<Vec<u8>> {
    conn.query_row("SELECT s.revision_id FROM revision_snapshots s JOIN works w ON w.id=s.entity_id WHERE s.entity_id=?1 AND s.id=?2 AND s.published=1",params![entity.as_bytes().as_slice(),snapshot.as_bytes().as_slice()],|r|r.get(0)).optional()?.ok_or(Error::ResourceUnavailable)
}

impl Library {
    pub fn maintenance_history(
        &self,
        entity: LibraryId,
        snapshot: LibraryId,
    ) -> Result<MaintenanceHistory> {
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        revision(&conn, entity, snapshot)?;
        let mut stmt=conn.prepare("SELECT id,entity_id,snapshot_id,origin,attempted_at,evidence_file_id FROM repair_attempt_markers WHERE entity_id=?1 AND snapshot_id=?2 ORDER BY attempted_at,id")?;
        let repairs = stmt
            .query_map(
                params![entity.as_bytes().as_slice(), snapshot.as_bytes().as_slice()],
                repair,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut stmt=conn.prepare("SELECT id,entity_id,snapshot_id,origin,legacy_rule,evidence_file_id FROM asset_audit_markers WHERE entity_id=?1 AND snapshot_id=?2 ORDER BY legacy_rule,id")?;
        let audits = stmt
            .query_map(
                params![entity.as_bytes().as_slice(), snapshot.as_bytes().as_slice()],
                |r| {
                    Ok(AssetAuditMarker {
                        id: id(r, 0)?,
                        entity: id(r, 1)?,
                        snapshot: id(r, 2)?,
                        origin: r.get(3)?,
                        legacy_rule: r.get(4)?,
                        evidence_file: hash(r, 5)?,
                    })
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(MaintenanceHistory {
            entity,
            snapshot,
            repair_attempts: repairs,
            asset_audits: audits,
        })
    }

    /// 同じ作者の改稿に対して取り直しを試した旧記録。記録なしは再取得の許可ではない。
    /// 実際の取得にはジョブの排他と begin_acquisition の照合を別途必要とする。
    pub fn prior_incomplete_repair_for_revision(
        &self,
        entity: LibraryId,
        snapshot: LibraryId,
    ) -> Result<Option<RepairAttemptMarker>> {
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let revision = revision(&conn, entity, snapshot)?;
        Ok(conn.query_row("SELECT r.id,r.entity_id,r.snapshot_id,r.origin,r.attempted_at,r.evidence_file_id FROM repair_attempt_markers r JOIN revision_snapshots s ON s.id=r.snapshot_id AND s.entity_id=r.entity_id WHERE r.entity_id=?1 AND s.revision_id=?2 ORDER BY r.attempted_at DESC,r.id DESC LIMIT 1",params![entity.as_bytes().as_slice(),revision],repair).optional()?)
    }

    pub(super) fn verify_maintenance_markers(conn: &Connection) -> Result<()> {
        let mut stmt = conn.prepare("SELECT attempted_at FROM repair_attempt_markers")?;
        for date in stmt.query_map([], |r| r.get::<_, i64>(0))? {
            if chrono::DateTime::from_timestamp_millis(date?).is_none() {
                return Err(Error::Integrity("修復試行の日時が不正です".into()));
            }
        }
        // 証跡は対応する旧表の原記録でなければならない。別の原本への付け替えも検出する。
        let bad:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM repair_attempt_markers r WHERE NOT EXISTS(SELECT 1 FROM migration_records m WHERE m.source_table='incomplete_save_repairs' AND m.source_key='__all_rows__' AND m.evidence_file_id=r.evidence_file_id)) OR EXISTS(SELECT 1 FROM asset_audit_markers a WHERE NOT EXISTS(SELECT 1 FROM migration_records m WHERE m.source_table='saved_asset_audits' AND m.source_key='__all_rows__' AND m.evidence_file_id=a.evidence_file_id))",[],|r|r.get(0))?;
        if bad {
            return Err(Error::Integrity(
                "保守記録と旧棚の原記録が一致しません".into(),
            ));
        }
        Ok(())
    }
}
