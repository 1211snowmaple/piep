use super::{
    rows::{mark_row, mark_table, marked},
    LegacyImporter,
};
use crate::library::{Error, Fingerprint, Library, LibraryId, Result};
use rusqlite::{params, OptionalExtension, Transaction};
use std::sync::atomic::{AtomicBool, Ordering};

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::Canceled)
    } else {
        Ok(())
    }
}
fn evidence(tx: &Transaction<'_>, table: &str) -> Result<Fingerprint> {
    let bytes:Option<Vec<u8>>=tx.query_row("SELECT evidence_file_id FROM migration_records WHERE source_table=?1 AND source_key='__all_rows__'",[table],|r|r.get(0)).optional()?.flatten();
    Ok(Fingerprint(
        bytes
            .ok_or_else(|| Error::Integrity("保守記録の原本を先に保持する必要があります".into()))?
            .try_into()
            .map_err(|_| Error::Integrity("保守記録の原本識別子が不正です".into()))?,
    ))
}

impl LegacyImporter {
    fn maintenance_snapshot(
        &self,
        tx: &Transaction<'_>,
        old: i64,
        version: i64,
    ) -> Result<(LibraryId, LibraryId, Vec<String>)> {
        if version <= 0 {
            return Err(Error::Integrity("保守記録の保存版が不正です".into()));
        }
        let provider: String =
            self.conn
                .query_row("SELECT source FROM downloads WHERE id=?1", [old], |r| {
                    r.get(0)
                })?;
        if provider != "pixiv" {
            return Err(Error::Unsupported(
                "保守記録の取得元を旧検査の対象として確認できません".into(),
            ));
        }
        let mut stmt=self.conn.prepare("SELECT id,json_path,original_json_path FROM download_versions WHERE download_id=?1 AND version=?2")?;
        let versions = stmt
            .query_map(params![old, version], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if versions.len() != 1 {
            return Err(Error::Integrity(
                "保守記録の保存版を一意に特定できません".into(),
            ));
        }
        let (row, path, original) = versions.into_iter().next().unwrap();
        let key = format!("{old}:{row}:{version}");
        let snapshot:Option<Vec<u8>>=tx.query_row("SELECT target_id FROM migration_records WHERE source_table='download_versions' AND source_key=?1 AND status='imported'",[key],|r|r.get(0)).optional()?.flatten();
        let snapshot = LibraryId::from_slice(&snapshot.ok_or_else(|| {
            Error::Integrity("保守記録の対象の保存版が移行されていません".into())
        })?)
        .map_err(|_| Error::Integrity("保守記録の保存版の識別が不正です".into()))?;
        let work = self.work_from_old(tx, old)?;
        let matches:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM revision_snapshots WHERE entity_id=?1 AND id=?2 AND published=1)",params![work.as_bytes().as_slice(),snapshot.as_bytes().as_slice()],|r|r.get(0))?;
        if !matches {
            return Err(Error::Integrity(
                "保守記録の保存版が別の作品に属しています".into(),
            ));
        }
        let mut paths = vec![path];
        if let Some(original) = original {
            paths.push(original);
        }
        Ok((work, snapshot, paths))
    }

    pub(super) fn normalize_maintenance(
        &self,
        target: &Library,
        cancel: &AtomicBool,
    ) -> Result<()> {
        check_cancel(cancel)?;
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        if !marked(&tx, "incomplete_save_repairs")? {
            self.each_row("incomplete_save_repairs",|row| {
                check_cancel(cancel)?;
                let old=row.integer("download_id")?;
                let (work,snapshot,_)=self.maintenance_snapshot(&tx,old,row.integer("version")?)?;
                let id=LibraryId::now_v7();let raw=evidence(&tx,"incomplete_save_repairs")?;
                tx.execute("INSERT INTO repair_attempt_markers(id,entity_id,snapshot_id,origin,attempted_at,evidence_file_id) VALUES(?1,?2,?3,'legacy-incomplete-save-v1',?4,?5)",params![id.as_bytes().as_slice(),work.as_bytes().as_slice(),snapshot.as_bytes().as_slice(),row.time("attempted_at")?,raw.0.as_slice()])?;
                mark_row(&tx,"incomplete_save_repairs",&old.to_string(),id)
            })?;
            mark_table(&tx, "incomplete_save_repairs")?;
        }
        if !marked(&tx, "saved_asset_audits")? {
            let has_rule = self.has_column("saved_asset_audits", "rule_version")?;
            self.each_row("saved_asset_audits",|row| {
                check_cancel(cancel)?;
                let old=row.integer("download_id")?;
                let (work,snapshot,paths)=self.maintenance_snapshot(&tx,old,row.integer("version")?)?;
                let path=row.text("json_path")?;
                if !paths.iter().any(|p|p==path) {
                    return Err(Error::Integrity("旧検査のパスが記録された保存版のパスと一致しません".into()));
                }
                let rule=if has_rule {row.integer("rule_version")?} else {1};
                if rule<=0 {return Err(Error::Integrity("旧検査の規則の版が不正です".into()));}
                let id=LibraryId::now_v7();let raw=evidence(&tx,"saved_asset_audits")?;
                tx.execute("INSERT INTO asset_audit_markers(id,entity_id,snapshot_id,origin,legacy_rule,evidence_file_id) VALUES(?1,?2,?3,'legacy-saved-assets-v1',?4,?5)",params![id.as_bytes().as_slice(),work.as_bytes().as_slice(),snapshot.as_bytes().as_slice(),rule,raw.0.as_slice()])?;
                mark_row(&tx,"saved_asset_audits",&old.to_string(),id)
            })?;
            mark_table(&tx, "saved_asset_audits")?;
        }
        check_cancel(cancel)?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "maintenance_tests.rs"]
mod tests;
