//! ジョブ単位で旧記録を照合する。封をした履歴を追いつきで書き換えない。
use super::super::rows::LegacyRow;
use super::*;
use crate::library::Fingerprint;
use rusqlite::OptionalExtension;
use std::collections::BTreeSet;
use std::io::Write;

#[derive(Default)]
struct JobRows {
    header: Option<Fingerprint>,
    items: BTreeMap<i64, Fingerprint>,
    logs: BTreeMap<i64, Fingerprint>,
}
#[derive(Default)]
pub(super) struct JobFingerprints {
    jobs: BTreeMap<String, JobRows>,
    items: BTreeSet<i64>,
    logs: BTreeSet<i64>,
}
impl JobFingerprints {
    fn add(&mut self, table: &str, row: LegacyRow) -> Result<()> {
        let key = row
            .text(if table == "update_jobs" {
                "id"
            } else {
                "job_id"
            })?
            .to_owned();
        let hash = Fingerprint::of(&serde_json::to_vec(&row.0)?);
        let job = self.jobs.entry(key).or_default();
        if table == "update_jobs" {
            if job.header.replace(hash).is_some() {
                return Err(Error::Integrity("ジョブ ID が重複しています".into()));
            }
        } else {
            let id = row.integer("id")?;
            let (seen, rows) = if table == "update_job_items" {
                (&mut self.items, &mut job.items)
            } else {
                (&mut self.logs, &mut job.logs)
            };
            if !seen.insert(id) {
                return Err(Error::Integrity(
                    "ジョブの項目・ログの ID が重複しています".into(),
                ));
            }
            rows.insert(id, hash);
        }
        Ok(())
    }
    fn finish(self) -> Result<BTreeMap<String, Fingerprint>> {
        self.jobs
            .into_iter()
            .map(|(key, job)| {
                let mut bytes = job
                    .header
                    .ok_or_else(|| {
                        Error::Integrity("項目・ログの所属するジョブがありません".into())
                    })?
                    .0
                    .to_vec();
                for (tag, rows) in [(b'i', job.items), (b'l', job.logs)] {
                    bytes.push(tag);
                    for (id, hash) in rows {
                        bytes.extend_from_slice(&id.to_le_bytes());
                        bytes.extend_from_slice(&hash.0);
                    }
                }
                Ok((key, Fingerprint::of(&bytes)))
            })
            .collect()
    }
}

/// 原記録は SQLite の型と文字列のバイトを保持した JSONL。表全体をメモリーへ展開しない。
struct EvidenceLines<'a> {
    current: Vec<u8>,
    failure: Option<Error>,
    line: &'a mut dyn FnMut(LegacyRow) -> Result<()>,
}
fn evidence_row(bytes: &[u8]) -> Result<LegacyRow> {
    let values: BTreeMap<String, serde_json::Value> = serde_json::from_slice(bytes)?;
    let values = values
        .into_iter()
        .map(|(key, value)| {
            let value = if value.is_null() || value.is_number() {
                value
            } else if let Some(bits) = value
                .get("sqlite_real_bits")
                .and_then(serde_json::Value::as_u64)
            {
                let number = f64::from_bits(bits);
                if !number.is_finite() {
                    return Err(Error::Integrity("ジョブの原記録の実数が不正です".into()));
                }
                serde_json::Value::from(number)
            } else if let Some(bytes) = value
                .get("sqlite_text_bytes")
                .and_then(serde_json::Value::as_array)
            {
                let bytes = bytes
                    .iter()
                    .map(|v| {
                        v.as_u64()
                            .and_then(|v| u8::try_from(v).ok())
                            .ok_or_else(|| {
                                Error::Integrity("ジョブの原記録の文字列が不正です".into())
                            })
                    })
                    .collect::<Result<Vec<_>>>()?;
                serde_json::Value::String(String::from_utf8(bytes).map_err(|_| {
                    Error::Integrity("ジョブの原記録が UTF-8 ではありません".into())
                })?)
            } else {
                return Err(Error::Integrity(
                    "ジョブの原記録の型を解釈できません".into(),
                ));
            };
            Ok((key, value))
        })
        .collect::<Result<_>>()?;
    Ok(LegacyRow(values))
}
impl Write for EvidenceLines<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let result = (|| {
            for part in bytes.split_inclusive(|b| *b == b'\n') {
                if self.current.len() + part.len() > 128 * 1024 * 1024 {
                    return Err(Error::Unsupported(
                        "ジョブの原記録の一行が上限を超えています".into(),
                    ));
                }
                self.current.extend_from_slice(part);
                if part.last() == Some(&b'\n') {
                    (self.line)(evidence_row(&self.current)?)?;
                    self.current.clear();
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.failure = Some(error);
            return Err(std::io::Error::other("ジョブの原記録を照合できません"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl LegacyImporter {
    pub(super) fn job_fingerprints(
        &self,
        cancel: &AtomicBool,
    ) -> Result<BTreeMap<String, Fingerprint>> {
        let mut hashes = JobFingerprints::default();
        for table in TABLES {
            self.each_row(table, |row| {
                check_cancel(cancel)?;
                hashes.add(table, row)
            })?;
        }
        hashes.finish()
    }

    /// preserve_tables が原記録を差し替える前に、旧版で作った完了履歴の照合の基準を補う。
    pub(in crate::library::legacy) fn prepare_job_refresh(
        &self,
        target: &Library,
        cancel: &AtomicBool,
    ) -> Result<()> {
        check_cancel(cancel)?;
        let (archives, missing) = {
            let conn = target.reader()?;
            let mut archives = Vec::new();
            for table in TABLES {
                let row:Option<(Option<Vec<u8>>,String)>=conn.query_row("SELECT evidence_file_id,status FROM migration_records WHERE source_table=?1 AND source_key='__all_rows__'",[table],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
                if let Some((Some(file), status)) = row {
                    archives.push((table, file, status));
                }
            }
            let missing=conn.prepare("SELECT m.source_key FROM migration_records m JOIN job_history j ON j.id=m.target_id WHERE m.source_table='update_jobs' AND m.status='imported' AND j.sealed=1 AND NOT EXISTS(SELECT 1 FROM migration_fingerprints f WHERE f.source_table='update_jobs' AND f.source_key=m.source_key)")?.query_map([],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            (archives, missing)
        };
        let mut hashes = JobFingerprints::default();
        if !missing.is_empty() {
            if archives.iter().any(|(_, _, status)| status != "imported")
                || !archives.iter().any(|(table, _, _)| *table == "update_jobs")
            {
                return Err(Error::Integrity(
                    "以前の完了履歴と原記録の対応が未確定です。新しい空の移行先で照合してください"
                        .into(),
                ));
            }
            for (table, bytes, _) in &archives {
                let file =
                    target.file(Fingerprint(bytes.clone().try_into().map_err(|_| {
                        Error::Integrity("ジョブの原記録の識別子が不正です".into())
                    })?))?;
                let mut on_line = |row| {
                    check_cancel(cancel)?;
                    hashes.add(table, row)
                };
                let mut sink = EvidenceLines {
                    current: Vec::new(),
                    failure: None,
                    line: &mut on_line,
                };
                let result = target.objects.export(&file, &mut sink);
                if let Some(error) = sink.failure {
                    return Err(error);
                }
                result?;
                if !sink.current.is_empty() {
                    return Err(Error::Integrity(
                        "ジョブの原記録が途中で終わっています".into(),
                    ));
                }
            }
        }
        let hashes = hashes.finish()?;
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        for (table, bytes, _) in archives {
            tx.execute(
                "INSERT OR IGNORE INTO job_import_evidence(source_table,file_id) VALUES(?1,?2)",
                params![table, bytes],
            )?;
        }
        for key in missing {
            let hash = hashes
                .get(&key)
                .ok_or_else(|| Error::Integrity("以前の完了履歴の原記録がありません".into()))?;
            tx.execute(
                "INSERT OR IGNORE INTO migration_fingerprints VALUES('update_jobs',?1,?2)",
                params![key, hash.0.as_slice()],
            )?;
        }
        check_cancel(cancel)?;
        tx.commit()?;
        Ok(())
    }
}
