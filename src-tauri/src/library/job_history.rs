use super::{Error, Fingerprint, Library, LibraryId, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

pub(super) const MAX_JOB_PAYLOAD_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobHistory {
    pub id: LibraryId,
    pub origin: String,
    pub scope: String,
    pub mode: String,
    pub status: String,
    pub watch_saved: bool,
    pub new_works_only: bool,
    /// 当時記録された集計。現在の作品情報から再計算しない。
    pub totals: i64,
    pub processed: i64,
    pub candidate_count: i64,
    pub saved_count: i64,
    pub error_count: i64,
    pub active_label: Option<String>,
    pub started_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobHistoryItem {
    pub id: LibraryId,
    pub sequence: i64,
    pub item_type: String,
    pub entity: Option<LibraryId>,
    pub target_type: Option<String>,
    pub title: String,
    pub payload_file: Fingerprint,
    pub status: String,
    pub error: Option<String>,
    /// 旧記録は保存した版を示さない。現在版を当時の結果として補わない。
    pub result_work: Option<LibraryId>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobHistoryLog {
    pub id: LibraryId,
    pub sequence: i64,
    pub log_type: String,
    pub message: String,
    pub created_at: i64,
}

fn id(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<LibraryId> {
    let bytes: Vec<u8> = row.get(index)?;
    LibraryId::from_slice(&bytes).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Blob, Box::new(e))
    })
}
fn optional_id(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<LibraryId>> {
    row.get::<_, Option<Vec<u8>>>(index)?
        .map(|b| {
            LibraryId::from_slice(&b).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    index,
                    rusqlite::types::Type::Blob,
                    Box::new(e),
                )
            })
        })
        .transpose()
}
fn page_limit(limit: usize) -> Result<i64> {
    if !(1..=1000).contains(&limit) {
        return Err(Error::Unsupported(
            "履歴の取得件数は 1〜1000 件で指定してください".into(),
        ));
    }
    Ok(limit as i64)
}
fn require_job(conn: &Connection, job: LibraryId) -> Result<()> {
    if !conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM job_history WHERE id=?1 AND sealed=1)",
        [job.as_bytes().as_slice()],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(Error::ResourceUnavailable);
    }
    Ok(())
}
fn summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobHistory> {
    Ok(JobHistory {
        id: id(row, 0)?,
        origin: row.get(1)?,
        scope: row.get(2)?,
        mode: row.get(3)?,
        status: row.get(4)?,
        watch_saved: row.get(5)?,
        new_works_only: row.get(6)?,
        totals: row.get(7)?,
        processed: row.get(8)?,
        candidate_count: row.get(9)?,
        saved_count: row.get(10)?,
        error_count: row.get(11)?,
        active_label: row.get(12)?,
        started_at: row.get(13)?,
        updated_at: row.get(14)?,
        finished_at: row.get(15)?,
    })
}
const SUMMARY: &str = "id,origin,scope,mode,status,watch_saved,new_works_only,totals,processed,candidate_count,saved_count,error_count,active_label,started_at,updated_at,finished_at";

impl Library {
    /// 履歴専用。読み出しても取得処理や再送を開始しない。
    pub fn job_history(&self, limit: usize) -> Result<Vec<JobHistory>> {
        self.job_history_page(None, limit)
    }

    pub fn job_history_page(
        &self,
        before: Option<LibraryId>,
        limit: usize,
    ) -> Result<Vec<JobHistory>> {
        let limit = page_limit(limit)?;
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let before = before
            .map(|id| {
                conn.query_row(
                    "SELECT started_at,id FROM job_history WHERE id=?1 AND sealed=1",
                    [id.as_bytes().as_slice()],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)),
                )
                .optional()?
                .ok_or(Error::ResourceUnavailable)
            })
            .transpose()?;
        let (date, id) = before
            .map(|(date, id)| (Some(date), Some(id)))
            .unwrap_or((None, None));
        let mut stmt = conn.prepare(&format!("SELECT {SUMMARY} FROM job_history WHERE sealed=1 AND (?2 IS NULL OR (started_at,id)<(?2,?3)) ORDER BY started_at DESC,id DESC LIMIT ?1"))?;
        let rows = stmt
            .query_map(params![limit, date, id], summary)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn job_history_entry(&self, job: LibraryId) -> Result<JobHistory> {
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        conn.query_row(
            &format!("SELECT {SUMMARY} FROM job_history WHERE id=?1 AND sealed=1"),
            [job.as_bytes().as_slice()],
            summary,
        )
        .optional()?
        .ok_or(Error::ResourceUnavailable)
    }

    pub fn job_history_items(
        &self,
        job: LibraryId,
        after: Option<i64>,
        limit: usize,
    ) -> Result<Vec<JobHistoryItem>> {
        let limit = page_limit(limit)?;
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        require_job(&conn, job)?;
        let mut stmt = conn.prepare("SELECT id,sequence,item_type,entity_id,title,payload_file_id,status,error,result_work_id,created_at,updated_at,target_type FROM job_history_items WHERE job_id=?1 AND (?2 IS NULL OR sequence>?2) ORDER BY sequence LIMIT ?3")?;
        let rows = stmt
            .query_map(params![job.as_bytes().as_slice(), after, limit], |r| {
                let bytes: Vec<u8> = r.get(5)?;
                let hash = bytes.try_into().map_err(|_| {
                    rusqlite::Error::InvalidColumnType(
                        5,
                        "payload_file_id".into(),
                        rusqlite::types::Type::Blob,
                    )
                })?;
                Ok(JobHistoryItem {
                    id: id(r, 0)?,
                    sequence: r.get(1)?,
                    item_type: r.get(2)?,
                    entity: optional_id(r, 3)?,
                    target_type: r.get(11)?,
                    title: r.get(4)?,
                    payload_file: Fingerprint(hash),
                    status: r.get(6)?,
                    error: r.get(7)?,
                    result_work: optional_id(r, 8)?,
                    created_at: r.get(9)?,
                    updated_at: r.get(10)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn job_history_logs(
        &self,
        job: LibraryId,
        after: Option<i64>,
        limit: usize,
    ) -> Result<Vec<JobHistoryLog>> {
        let limit = page_limit(limit)?;
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        require_job(&conn, job)?;
        let mut stmt = conn.prepare("SELECT id,sequence,log_type,message,created_at FROM job_history_logs WHERE job_id=?1 AND (?2 IS NULL OR sequence>?2) ORDER BY sequence LIMIT ?3")?;
        let rows = stmt
            .query_map(params![job.as_bytes().as_slice(), after, limit], |r| {
                Ok(JobHistoryLog {
                    id: id(r, 0)?,
                    sequence: r.get(1)?,
                    log_type: r.get(2)?,
                    message: r.get(3)?,
                    created_at: r.get(4)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 対象のジョブと項目の所属を照合し、元の JSON の字句を返す。
    pub fn job_history_payload(&self, job: LibraryId, item: LibraryId) -> Result<String> {
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let hash: Option<Vec<u8>> = conn.query_row("SELECT i.payload_file_id FROM job_history_items i JOIN job_history j ON j.id=i.job_id WHERE j.id=?1 AND j.sealed=1 AND i.id=?2",params![job.as_bytes().as_slice(),item.as_bytes().as_slice()],|r|r.get(0)).optional()?;
        let hash = hash.ok_or(Error::ResourceUnavailable)?;
        let hash = Fingerprint(
            hash.try_into()
                .map_err(|_| Error::Integrity("履歴の原本識別子が不正です".into()))?,
        );
        self.read_job_payload(&conn, hash)
    }

    fn read_job_payload(&self, conn: &Connection, hash: Fingerprint) -> Result<String> {
        let file = Self::file_from_conn(conn, hash)?;
        if file.length > MAX_JOB_PAYLOAD_BYTES {
            return Err(Error::Unsupported(
                "ジョブの原記録が読み込み上限を超えています".into(),
            ));
        }
        let mut bytes = Vec::new();
        self.objects.export(&file, &mut bytes)?;
        let text = String::from_utf8(bytes)
            .map_err(|_| Error::Integrity("ジョブの原記録が UTF-8 ではありません".into()))?;
        let _: &serde_json::value::RawValue = serde_json::from_str(&text)?;
        Ok(text)
    }

    pub(super) fn verify_job_history(&self, conn: &Connection) -> Result<()> {
        let pending: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM job_history WHERE sealed=0)",
            [],
            |r| r.get(0),
        )?;
        if pending {
            return Err(Error::Integrity("未確定のジョブ履歴があります".into()));
        }
        let mut stmt = conn.prepare("SELECT DISTINCT payload_file_id FROM job_history_items")?;
        for hash in stmt.query_map([], |r| r.get::<_, Vec<u8>>(0))? {
            let hash = Fingerprint(
                hash?
                    .try_into()
                    .map_err(|_| Error::Integrity("履歴の原本識別子が不正です".into()))?,
            );
            self.read_job_payload(conn, hash)?;
        }
        Ok(())
    }
}
