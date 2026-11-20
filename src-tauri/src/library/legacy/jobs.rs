use super::{
    rows::{mapped_id, mark_row, mark_table, marked, LegacyRow},
    LegacyImporter,
};
use crate::library::{EntityKind, Error, Library, Result};
use rusqlite::{params, OptionalExtension};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::{AtomicBool, Ordering},
};
#[path = "jobs_fingerprints.rs"]
mod fingerprints;
const TABLES: [&str; 3] = ["update_jobs", "update_job_items", "update_job_logs"];
fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::Canceled)
    } else {
        Ok(())
    }
}
fn runnable(status: &str) -> bool {
    matches!(status, "queued" | "running" | "candidate" | "failed")
}
fn validate_input(row: &LegacyRow) -> Result<()> {
    if !runnable(row.text("status")?) {
        return Ok(());
    }
    if !matches!(row.optional_text("source")?, Some("pixiv" | "fanbox"))
        || row.optional_text("source_id")?.is_none_or(str::is_empty)
    {
        return Err(Error::Unsupported(
            "実行項目の取得元・作品 ID を解釈できません".into(),
        ));
    }
    let payload: serde_json::Value = serde_json::from_str(row.text("payload_json")?)?;
    if !payload.is_object() {
        return Err(Error::Unsupported(
            "実行項目の入力がオブジェクトではありません".into(),
        ));
    }
    if row.text("item_type")? == "target" {
        let text = |key: &str| {
            payload
                .get(key)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    Error::Integrity(format!("監視対象の入力の {key} が文字列ではありません"))
                })
        };
        let source = text("source")?;
        let key = text("sourceKey")?;
        let kind = text("targetType")?;
        for name in ["displayName", "createdAt", "updatedAt"] {
            text(name)?;
        }
        let id = payload
            .get("id")
            .ok_or_else(|| Error::Integrity("監視対象の識別子がありません".into()))?;
        if id.as_i64().is_none()
            && !id
                .as_str()
                .is_some_and(|v| v.parse::<crate::library::LibraryId>().is_ok())
        {
            return Err(Error::Integrity("監視対象の識別子を解釈できません".into()));
        }
        if payload
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .is_none()
        {
            return Err(Error::Integrity(
                "監視対象の有効状態が真偽値ではありません".into(),
            ));
        }
        for name in [
            "lastCheckedAt",
            "lastSeenSourceId",
            "lastSeenSourceUpdatedAt",
            "metadataJson",
            "lastHitAt",
            "iconPath",
        ] {
            if payload
                .get(name)
                .is_some_and(|v| !v.is_null() && !v.is_string())
            {
                return Err(Error::Integrity(
                    "監視対象の任意の文字列の型が不正です".into(),
                ));
            }
        }
        if payload
            .get("watchGeneration")
            .is_some_and(|v| !v.is_null() && v.as_i64().is_none())
            || payload
                .get("consecutiveErrors")
                .is_some_and(|v| v.as_i64().is_none())
        {
            return Err(Error::Integrity("監視対象の整数の型が不正です".into()));
        }
        if payload.get("watchGroups").is_some_and(|v| {
            !v.as_array()
                .is_some_and(|a| a.iter().all(serde_json::Value::is_string))
        }) {
            return Err(Error::Integrity("監視グループの入力の型が不正です".into()));
        }
        if !matches!(
            (source, kind),
            ("pixiv", "author" | "series") | ("fanbox", "author")
        ) {
            return Err(Error::Unsupported(
                "実行する監視対象の種類を処理できません".into(),
            ));
        }
        if Some(source) != row.optional_text("source")?
            || Some(key) != row.optional_text("source_id")?
            || Some(kind) != row.optional_text("target_type")?
        {
            return Err(Error::Integrity(
                "実行する監視対象と入力の取得元が一致しません".into(),
            ));
        }
    } else if row.text("item_type")? == "candidate"
        && payload.get("kind").is_some_and(|v| {
            !matches!(
                v.as_str(),
                Some("new" | "sequel" | "revision" | "save" | "permission_check")
            )
        })
    {
        return Err(Error::Unsupported(
            "実行項目の入力の種類を解釈できません".into(),
        ));
    }
    Ok(())
}
impl LegacyImporter {
    pub(super) fn normalize_job_history(
        &self,
        target: &Library,
        cancel: &AtomicBool,
    ) -> Result<()> {
        check_cancel(cancel)?;
        let complete = {
            let conn = target.reader()?;
            TABLES
                .iter()
                .map(|t| marked(&conn, t))
                .collect::<Result<Vec<_>>>()?
                .iter()
                .all(|done| *done)
        };
        if complete {
            target.conn.lock().map_err(|_|Error::Busy)?.execute("INSERT OR IGNORE INTO job_import_evidence SELECT source_table,evidence_file_id FROM migration_records WHERE source_key='__all_rows__' AND source_table IN ('update_jobs','update_job_items','update_job_logs') AND evidence_file_id IS NOT NULL",[])?;
            return Ok(());
        }
        let hashes = self.job_fingerprints(cancel)?;
        let mut pending = BTreeSet::new();
        let mut unfinished = BTreeSet::new();
        self.each_row("update_job_items", |row| {
            check_cancel(cancel)?;
            match row.text("status")? {
                "queued" | "running" => {
                    pending.insert(row.text("job_id")?.to_owned());
                    unfinished.insert(row.text("job_id")?.to_owned());
                }
                "candidate" => {
                    pending.insert(row.text("job_id")?.to_owned());
                }
                "failed" if row.text("item_type")? == "candidate" => {
                    pending.insert(row.text("job_id")?.to_owned());
                }
                "done" | "saved" | "skipped" | "held" | "failed" => {}
                _ => {
                    return Err(Error::Unsupported(
                        "ジョブの項目の状態を解釈できません".into(),
                    ))
                }
            }
            Ok(())
        })?;
        let mut execution = BTreeMap::new();
        self.each_row("update_jobs", |row| {
            check_cancel(cancel)?;
            let key = row.text("id")?;
            let status = row.text("status")?;
            if !matches!(
                status,
                "completed"
                    | "queued"
                    | "running"
                    | "canceling"
                    | "paused"
                    | "canceled"
                    | "auth_required"
                    | "failed"
            ) {
                return Err(Error::Unsupported(format!(
                    "更新ジョブ {key} の状態 ({status}) を解釈できません"
                )));
            }
            let queue = status != "completed" || pending.contains(key);
            if queue
                && (!matches!(
                    row.text("scope")?,
                    "all" | "work" | "author" | "series" | "targets" | "save"
                ) || !matches!(row.text("mode")?, "check_only" | "auto_save" | "save"))
            {
                return Err(Error::Unsupported(
                    "実行ジョブの範囲・方法を解釈できません".into(),
                ));
            }
            execution.insert(key.to_owned(), queue);
            Ok(())
        })?;
        let mut unchanged = BTreeSet::new();
        {
            let conn = target.reader()?;
            for (key, hash) in &hashes {
                let old:Option<Option<Vec<u8>>>=conn.query_row("SELECT f.fingerprint FROM migration_records m JOIN job_history j ON j.id=m.target_id LEFT JOIN migration_fingerprints f ON f.source_table=m.source_table AND f.source_key=m.source_key WHERE m.source_table='update_jobs' AND m.source_key=?1 AND j.sealed=1",[key],|r|r.get(0)).optional()?;
                if let Some(old) = old {
                    if execution[key] {
                        return Err(Error::Integrity(format!("完了履歴 {key} に再開可能な項目があります。封をした原本を保ち、新しい空の移行先へ実行キューとして移してください")));
                    }
                    if old.as_deref() != Some(hash.0.as_slice()) {
                        return Err(Error::Integrity(format!(
                            "完了履歴 {key} の旧記録が変わっています。封をした原本は上書きしません"
                        )));
                    }
                    unchanged.insert(key.clone());
                }
            }
        }
        let mut payloads = BTreeMap::new();
        self.each_row("update_job_items", |row| {
            check_cancel(cancel)?;
            if unchanged.contains(row.text("job_id")?) {
                return Ok(());
            }
            let queue = *execution
                .get(row.text("job_id")?)
                .ok_or_else(|| Error::Integrity("項目の所属するジョブがありません".into()))?;
            if queue {
                validate_input(&row)?;
                if row.integer("id")? <= 0 {
                    return Err(Error::Unsupported(
                        "実行項目の連番が正ではありません".into(),
                    ));
                }
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
                return Err(Error::Integrity("ジョブの項目 ID が重複しています".into()));
            }
            Ok(())
        })?;
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        // 切り替える前の追いつき。キューは旧棚が正本なので、その現在の行で作り直す。
        // 封をした履歴と、この移行器に属さないキューには触れない。
        tx.execute("DELETE FROM execution_jobs WHERE id IN (SELECT target_id FROM migration_records WHERE source_table='update_jobs' AND status='imported')",[])?;
        tx.execute("DELETE FROM migration_records WHERE source_table IN ('update_job_items','update_job_logs') AND status='imported' AND target_id IS NULL",[])?;
        self.each_row("update_jobs",|row| {
            check_cancel(cancel)?;let key=row.text("id")?;
            if unchanged.contains(key) {return Ok(());}
            let id=mapped_id(&tx,"update_jobs",key)?;
            // 時刻を検証し、キューには元の文字列、履歴には従来のミリ秒を保持する。
            let started=row.time("started_at")?;let updated=row.time("updated_at")?;let finished=row.optional_time("finished_at")?;
            if execution[key] {
                let incomplete=row.text("status")?=="completed"&&unfinished.contains(key);
                let status=if incomplete {"paused"}else{row.text("status")?};
                let label=if incomplete {Some("移行前のジョブに未処理の項目が残っています")}else{row.optional_text("active_label")?};
                let finished_raw=if incomplete {None}else{row.optional_text("finished_at")?};
                tx.execute("INSERT INTO execution_jobs(id,scope,mode,status,watch_saved,new_works_only,totals,processed,candidate_count,saved_count,error_count,active_label,started_at,updated_at,finished_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",params![id.as_bytes().as_slice(),row.text("scope")?,row.text("mode")?,status,row.boolean("watch_saved")?,row.boolean("new_works_only")?,row.integer("totals")?,row.integer("processed")?,row.integer("candidate_count")?,row.integer("saved_count")?,row.integer("error_count")?,label,row.text("started_at")?,row.text("updated_at")?,finished_raw])?;
            } else {
                tx.execute("INSERT INTO job_history(id,origin,scope,mode,status,watch_saved,new_works_only,totals,processed,candidate_count,saved_count,error_count,active_label,started_at,updated_at,finished_at) VALUES(?1,'legacy-update-v1',?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",params![id.as_bytes().as_slice(),row.text("scope")?,row.text("mode")?,row.text("status")?,row.boolean("watch_saved")?,row.boolean("new_works_only")?,row.integer("totals")?,row.integer("processed")?,row.integer("candidate_count")?,row.integer("saved_count")?,row.integer("error_count")?,row.optional_text("active_label")?,started,updated,finished])?;
            }
            mark_row(&tx,"update_jobs",key,id)?;
            tx.execute("INSERT INTO migration_fingerprints VALUES('update_jobs',?1,?2) ON CONFLICT(source_table,source_key) DO UPDATE SET fingerprint=excluded.fingerprint",params![key,hashes[key].0.as_slice()])?;
            Ok(())
        })?;
        self.each_row("update_job_items",|row| {
            check_cancel(cancel)?;let owner=row.text("job_id")?;if unchanged.contains(owner) {return Ok(());}
            let sequence=row.integer("id")?;let key=sequence.to_string();let job=mapped_id(&tx,"update_jobs",owner)?;
            let kind=match row.text("item_type")? {
                "work"|"candidate"=>EntityKind::Work,
                "target"=>match row.optional_text("target_type")? {Some("author"|"creator")=>EntityKind::Person,Some("series")=>EntityKind::Series,Some("work")=>EntityKind::Work,_=>return Err(Error::Unsupported("更新ジョブの対象の種類を解釈できません".into()))},
                _=>return Err(Error::Unsupported("更新ジョブの項目の種類を解釈できません".into())),
            };
            let created=row.time("created_at")?;let updated=row.time("updated_at")?;
            let entity=match (row.optional_text("source")?,row.optional_text("source_id")?) {
                (Some(provider),Some(key)) if !provider.is_empty()&&!key.is_empty()=>Some(Library::relation_entity(&tx,provider,kind,key,created)?),
                (None,None)=>None,_=>return Err(Error::Integrity("更新ジョブの取得元の識別が不完全です".into())),
            };
            let result=row.optional_integer("result_download_id")?.map(|old|self.work_from_old(&tx,old)).transpose()?;
            if kind==EntityKind::Work&&result.is_some()&&result!=entity {return Err(Error::Integrity("更新ジョブの保存結果が別の作品を指しています".into()));}
            let file=payloads.get(&sequence).ok_or_else(||Error::Integrity("ジョブの原本がありません".into()))?;Library::register_file(&tx,file)?;
            if execution[owner] {
                let payload:serde_json::Value=serde_json::from_str(row.text("payload_json")?)?;
                let generation=if runnable(row.text("status")?) {crate::library::legacy_recheck_generation(&tx,entity,&payload)?}else{None};
                tx.execute("INSERT INTO execution_items(id,source_sequence,job_id,item_type,entity_id,source,source_id,target_type,title,payload_file_id,display_json,known_hold,status,error,result_work_id,created_at,updated_at,resume_generation) VALUES(?1,?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",params![sequence,job.as_bytes().as_slice(),row.text("item_type")?,entity.map(|v|v.as_bytes().to_vec()),row.optional_text("source")?,row.optional_text("source_id")?,row.optional_text("target_type")?,row.text("title")?,file.file.id.0.as_slice(),crate::library::ExecutionMetadata::execution_display(row.text("payload_json")?)?,payload.get("knownHold").and_then(serde_json::Value::as_bool).unwrap_or(false),row.text("status")?,row.optional_text("error")?,result.map(|v|v.as_bytes().to_vec()),row.text("created_at")?,row.text("updated_at")?,generation])?;
                tx.execute("INSERT INTO migration_records(source_table,source_key,status) VALUES('update_job_items',?1,'imported') ON CONFLICT(source_table,source_key) DO UPDATE SET target_id=NULL,status='imported'",[key])?;
            } else {
                let id=mapped_id(&tx,"update_job_items",&key)?;
                tx.execute("INSERT INTO job_history_items(id,job_id,sequence,item_type,entity_id,target_type,title,payload_file_id,status,error,result_work_id,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",params![id.as_bytes().as_slice(),job.as_bytes().as_slice(),sequence,row.text("item_type")?,entity.map(|v|v.as_bytes().to_vec()),row.optional_text("target_type")?,row.text("title")?,file.file.id.0.as_slice(),row.text("status")?,row.optional_text("error")?,result.map(|v|v.as_bytes().to_vec()),created,updated])?;
                mark_row(&tx,"update_job_items",&key,id)?;
            }
            Ok(())
        })?;
        self.each_row("update_job_logs",|row| {
            check_cancel(cancel)?;let owner=row.text("job_id")?;if unchanged.contains(owner) {return Ok(());}
            let sequence=row.integer("id")?;let key=sequence.to_string();let job=mapped_id(&tx,"update_jobs",owner)?;
            let created=row.time("created_at")?;
            if execution[owner] {
                if sequence<=0 {return Err(Error::Unsupported("実行ログの連番が正ではありません".into()));}
                tx.execute("INSERT INTO execution_logs(id,source_sequence,job_id,log_type,message,created_at) VALUES(?1,?1,?2,?3,?4,?5)",params![sequence,job.as_bytes().as_slice(),row.text("log_type")?,row.text("message")?,row.text("created_at")?])?;
                tx.execute("INSERT INTO migration_records(source_table,source_key,status) VALUES('update_job_logs',?1,'imported') ON CONFLICT(source_table,source_key) DO UPDATE SET target_id=NULL,status='imported'",[key])?;
            } else {
                let id=mapped_id(&tx,"update_job_logs",&key)?;
                tx.execute("INSERT INTO job_history_logs VALUES(?1,?2,?3,?4,?5,?6)",params![id.as_bytes().as_slice(),job.as_bytes().as_slice(),sequence,row.text("log_type")?,row.text("message")?,created])?;
                mark_row(&tx,"update_job_logs",&key,id)?;
            }
            Ok(())
        })?;
        check_cancel(cancel)?;
        tx.execute("UPDATE job_history SET sealed=1 WHERE sealed=0", [])?;
        for table in TABLES {
            mark_table(&tx, table)?;
        }
        tx.execute("INSERT OR IGNORE INTO job_import_evidence SELECT source_table,evidence_file_id FROM migration_records WHERE source_key='__all_rows__' AND source_table IN ('update_jobs','update_job_items','update_job_logs') AND evidence_file_id IS NOT NULL",[])?;

        tx.commit()?;
        Ok(())
    }
}
#[cfg(test)]
#[path = "jobs_tests.rs"]
mod tests;
