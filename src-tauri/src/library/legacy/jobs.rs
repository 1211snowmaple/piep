use super::{
    rows::{mapped_id, mark_row, mark_table, marked},
    LegacyImporter,
};
use crate::library::{EntityKind, Error, Library, Result};
use rusqlite::params;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};

const TABLES: [&str; 3] = ["update_jobs", "update_job_items", "update_job_logs"];
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::Canceled)
    } else {
        Ok(())
    }
}

impl LegacyImporter {
    pub(super) fn normalize_job_history(
        &self,
        target: &Library,
        cancel: &AtomicBool,
    ) -> Result<()> {
        check_cancel(cancel)?;
        {
            let conn = target.conn.lock().map_err(|_| Error::Busy)?;
            if TABLES
                .iter()
                .map(|table| marked(&conn, table))
                .collect::<Result<Vec<_>>>()?
                .iter()
                .all(|done| *done)
            {
                return Ok(());
            }
        }
        // 旧処理の再開状態を履歴に変えて消費してはならない。原記録は既に保持してある。
        self.each_row("update_jobs", |row| {
            check_cancel(cancel)?;
            let status = row.text("status")?;
            if status != "completed" {
                return Err(Error::Unsupported(format!("更新ジョブ {} は未完了・再開可能な状態 ({status}) です。実行キューの移行前には切り替えられません",row.text("id")?)));
            }
            Ok(())
        })?;
        let mut payloads = BTreeMap::new();
        self.each_row("update_job_items", |row| {
            check_cancel(cancel)?;
            if !matches!(
                row.text("status")?,
                "done" | "saved" | "skipped" | "held" | "failed"
            ) {
                return Err(Error::Unsupported(format!("更新ジョブ {} の項目 {} は未処理の状態 ({}) です。履歴として完了扱いにはできません",row.text("job_id")?,row.integer("id")?,row.text("status")?)));
            }
            let payload = row.text("payload_json")?;
            if payload.len() as u64 > crate::library::job_history::MAX_JOB_PAYLOAD_BYTES {
                return Err(Error::Unsupported(
                    "ジョブの原記録が読み込み上限を超えています".into(),
                ));
            }
            let _: &serde_json::value::RawValue = serde_json::from_str(payload)?;
            if payloads
                .insert(row.integer("id")?, target.store_bytes(payload.as_bytes())?)
                .is_some()
            {
                return Err(Error::Integrity(
                    "更新ジョブの項目 ID が重複しています".into(),
                ));
            }
            Ok(())
        })?;
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        self.each_row("update_jobs", |row| {
            check_cancel(cancel)?;
            let key = row.text("id")?;
            let id = mapped_id(&tx, "update_jobs", key)?;
            tx.execute("INSERT INTO job_history(id,origin,scope,mode,status,watch_saved,new_works_only,totals,processed,candidate_count,saved_count,error_count,active_label,started_at,updated_at,finished_at) VALUES(?1,'legacy-update-v1',?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",params![id.as_bytes().as_slice(),row.text("scope")?,row.text("mode")?,row.text("status")?,row.boolean("watch_saved")?,row.boolean("new_works_only")?,row.integer("totals")?,row.integer("processed")?,row.integer("candidate_count")?,row.integer("saved_count")?,row.integer("error_count")?,row.optional_text("active_label")?,row.time("started_at")?,row.time("updated_at")?,row.optional_time("finished_at")?])?;
            mark_row(&tx,"update_jobs",key,id)
        })?;
        self.each_row("update_job_items", |row| {
            check_cancel(cancel)?;
            let sequence = row.integer("id")?;
            let key = sequence.to_string();
            let id = mapped_id(&tx,"update_job_items",&key)?;
            let job = mapped_id(&tx,"update_jobs",row.text("job_id")?)?;
            let kind = match row.text("item_type")? {
                "work" | "candidate" => EntityKind::Work,
                "target" => match row.optional_text("target_type")? {
                    Some("author" | "creator") => EntityKind::Person,
                    Some("series") => EntityKind::Series,
                    Some("work") => EntityKind::Work,
                    _ => return Err(Error::Unsupported("更新ジョブの対象の種類を解釈できません".into())),
                },
                _ => return Err(Error::Unsupported("更新ジョブの項目の種類を解釈できません".into())),
            };
            let created = row.time("created_at")?;
            let entity = match (row.optional_text("source")?,row.optional_text("source_id")?) {
                (Some(provider),Some(key)) if !provider.is_empty() && !key.is_empty() => Some(Library::relation_entity(&tx,provider,kind,key,created)?),
                (None,None) => None,
                _ => return Err(Error::Integrity("更新ジョブの取得元の識別が不完全です".into())),
            };
            let result = row.optional_integer("result_download_id")?.map(|old|self.work_from_old(&tx,old)).transpose()?;
            if kind==EntityKind::Work && result.is_some() && result!=entity {
                return Err(Error::Integrity("更新ジョブの保存結果が別の作品を指しています".into()));
            }
            let payload = payloads.get(&sequence).ok_or_else(||Error::Integrity("更新ジョブの原本がありません".into()))?;
            Library::register_file(&tx,payload)?;
            tx.execute("INSERT INTO job_history_items(id,job_id,sequence,item_type,entity_id,target_type,title,payload_file_id,status,error,result_work_id,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",params![id.as_bytes().as_slice(),job.as_bytes().as_slice(),sequence,row.text("item_type")?,entity.map(|id|id.as_bytes().to_vec()),row.optional_text("target_type")?,row.text("title")?,payload.file.id.0.as_slice(),row.text("status")?,row.optional_text("error")?,result.map(|id|id.as_bytes().to_vec()),created,row.time("updated_at")?])?;
            mark_row(&tx,"update_job_items",&key,id)
        })?;
        self.each_row("update_job_logs", |row| {
            check_cancel(cancel)?;
            let sequence = row.integer("id")?;
            let key = sequence.to_string();
            let id = mapped_id(&tx, "update_job_logs", &key)?;
            let job = mapped_id(&tx, "update_jobs", row.text("job_id")?)?;
            tx.execute(
                "INSERT INTO job_history_logs VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    id.as_bytes().as_slice(),
                    job.as_bytes().as_slice(),
                    sequence,
                    row.text("log_type")?,
                    row.text("message")?,
                    row.time("created_at")?
                ],
            )?;
            mark_row(&tx, "update_job_logs", &key, id)
        })?;
        check_cancel(cancel)?;
        tx.execute("UPDATE job_history SET sealed=1 WHERE sealed=0", [])?;
        for table in TABLES {
            mark_table(&tx, table)?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;
