//! ジョブより長く残る発見候補と利用者の判断。原入力は可逆圧縮し、一覧は台帳だけで引く。

use super::{EntityKind, Error, Fingerprint, Library, LibraryId, Result};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Value};

pub struct DiscoveryInput<'a> {
    pub provider: &'a str,
    pub key: &'a str,
    pub kind: &'a str,
    pub title: &'a str,
    pub payload: &'a str,
    pub target_type: Option<&'a str>,
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;

#[derive(Clone, Debug)]
pub struct Discovery {
    pub id: LibraryId,
    pub work: LibraryId,
    pub provider: String,
    pub key: String,
    pub kind: String,
    pub title: String,
    pub decision: String,
    pub hold_kind: Option<String>,
    pub payload: Fingerprint,
    pub target_type: Option<String>,
    pub first_seen_at: i64,
    pub updated_at: i64,
    pub generation: i64,
    pub summary: Option<Value>,
}

fn identifier(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<LibraryId> {
    LibraryId::from_slice(&row.get::<_, Vec<u8>>(index)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Blob,
            Box::new(error),
        )
    })
}
fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Discovery> {
    let bytes: Vec<u8> = row.get(8)?;
    Ok(Discovery {
        id: identifier(row, 0)?,
        work: identifier(row, 1)?,
        provider: row.get(2)?,
        key: row.get(3)?,
        kind: row.get(4)?,
        title: row.get(5)?,
        decision: row.get(6)?,
        hold_kind: row.get(7)?,
        payload: Fingerprint(
            bytes
                .try_into()
                .map_err(|_| rusqlite::Error::InvalidQuery)?,
        ),
        target_type: row.get(9)?,
        first_seen_at: row.get(10)?,
        updated_at: row.get(11)?,
        generation: row.get(12)?,
        summary: row
            .get::<_, Option<String>>(13)?
            .map(|v| {
                serde_json::from_str(&v).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        13,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })
            })
            .transpose()?,
    })
}
const COLUMNS: &str = "c.id,c.work_id,x.provider,x.external_key,c.kind,c.title,c.decision,c.hold_kind,c.payload_file_id,c.target_type,c.first_seen_at,c.updated_at,coalesce(s.generation,0),p.summary_json";
const TABLES: &str = "discovery_candidates c JOIN external_identities x ON x.entity_id=c.work_id AND x.kind='work' LEFT JOIN discovery_state s ON s.work_id=c.work_id LEFT JOIN discovery_summaries p ON p.candidate_id=c.id AND p.payload_file_id=c.payload_file_id";
type Observation<'a> = (
    &'a DiscoveryInput<'a>,
    &'a str,
    Option<&'a str>,
    super::repository::PreparedFile,
);

fn summary(payload: &Value) -> Value {
    fn text(value: &Value, paths: &[&str]) -> Option<String> {
        paths.iter().find_map(|p| {
            value
                .pointer(p)
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_owned)
        })
    }
    let value = payload.get("originalData").unwrap_or(payload);
    let value = value
        .get("body")
        .filter(|body| body.get("id").is_some())
        .unwrap_or(value);
    json!({"kind":payload.get("kind"),"holdKind":payload.get("holdKind"),"holdReason":payload.get("holdReason"),"originalData":{
        "user":{"name":text(value,&["/user/name","/creator/name","/authorName","/userName","/author_name","/detail/user/name","/detail/authorName"])},
        "create_date":text(value,&["/create_date","/createDate","/publishedDatetime","/cdate","/detail/createDate","/detail/cdate"]),
        "uploadDate":text(value,&["/uploadDate","/updateDate","/updatedDatetime","/detail/uploadDate"])
    }})
}

fn validate_key(provider: &str, key: &str) -> Result<()> {
    if !matches!(provider, "pixiv" | "fanbox") || key.trim().is_empty() || key.len() > 128 {
        return Err(Error::Integrity("候補の取得元と作品 ID が不正です".into()));
    }
    Ok(())
}

impl Library {
    /// 編集後の再検査を、読んだ保存状態・編集世代・候補がまだ同じ場合だけ反映する。
    pub(crate) fn reconcile_pixiv_image_hold(
        &self,
        work: LibraryId,
        snapshot: LibraryId,
        edit_generation: i64,
        previous: Option<&Discovery>,
        finding: Option<(&str, &Value)>,
    ) -> Result<()> {
        if previous.is_some_and(|row| {
            row.work != work
                || row.provider != "pixiv"
                || row.decision != "held"
                || row.hold_kind.as_deref() != Some("incomplete")
        }) {
            return Ok(());
        }
        let prepared = finding
            .map(|(_, value)| -> Result<super::repository::PreparedFile> {
                let bytes = serde_json::to_vec(value)?;
                if bytes.len() as u64 > super::job_history::MAX_JOB_PAYLOAD_BYTES {
                    return Err(Error::Unsupported(
                        "候補が読み込み上限を超えています".into(),
                    ));
                }
                Ok(self.objects.put_stored(bytes.as_slice())?.into())
            })
            .transpose()?;
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let current: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM entity_current c JOIN library_items li ON li.work_id=c.entity_id
                JOIN external_identities x ON x.entity_id=c.entity_id AND x.kind='work' AND x.provider='pixiv'
                LEFT JOIN edit_documents e ON e.work_id=c.entity_id AND e.user_id=?3
                WHERE c.entity_id=?1 AND c.snapshot_id=?2 AND coalesce(e.generation,0)=?4)",
            params![work.as_bytes().as_slice(), snapshot.as_bytes().as_slice(), self.id.as_bytes().as_slice(), edit_generation],
            |row| row.get(0),
        )?;
        if !current {
            return Ok(());
        }
        let mut stmt = tx.prepare(&format!(
            "SELECT {COLUMNS} FROM {TABLES} WHERE c.work_id=?1 AND x.provider='pixiv'"
        ))?;
        let rows = stmt
            .query_map([work.as_bytes().as_slice()], row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        let candidate = match (rows.as_slice(), previous) {
            ([], None) => None,
            ([current], Some(previous))
                if current.id == previous.id
                    && current.generation == previous.generation
                    && current.payload == previous.payload
                    && current.decision == "held"
                    && current.hold_kind.as_deref() == Some("incomplete") =>
            {
                Some(current)
            }
            _ => return Ok(()),
        };
        let request = LibraryId::now_v7();
        let now = chrono::Utc::now().timestamp_millis();
        if let (Some(file), Some((title, value))) = (prepared, finding) {
            if candidate.is_some_and(|row| row.payload == file.file.id && row.title == title) {
                return Ok(());
            }
            Self::register_file(&tx, &file)?;
            let id = candidate.map_or(request, |row| row.id);
            tx.execute("INSERT INTO discovery_candidates(id,work_id,kind,title,decision,hold_kind,payload_file_id,target_type,first_seen_at,updated_at)
                VALUES(?1,?2,'revision',?3,'held','incomplete',?4,'work',?5,?5)
                ON CONFLICT(id) DO UPDATE SET title=excluded.title,payload_file_id=excluded.payload_file_id,updated_at=excluded.updated_at",
                params![id.as_bytes().as_slice(),work.as_bytes().as_slice(),title,file.file.id.0.as_slice(),now])?;
            tx.execute("INSERT INTO discovery_summaries(candidate_id,payload_file_id,summary_json) VALUES(?1,?2,?3)
                ON CONFLICT(candidate_id) DO UPDATE SET payload_file_id=excluded.payload_file_id,summary_json=excluded.summary_json",
                params![id.as_bytes().as_slice(),file.file.id.0.as_slice(),summary(value).to_string()])?;
        } else if let Some(row) = candidate {
            tx.execute(
                "DELETE FROM discovery_candidates WHERE id=?1",
                [row.id.as_bytes().as_slice()],
            )?;
        } else {
            return Ok(());
        }
        let generation: i64 = tx.query_row(
            "INSERT INTO discovery_state(work_id,generation) VALUES(?1,1)
            ON CONFLICT(work_id) DO UPDATE SET generation=generation+1 RETURNING generation",
            [work.as_bytes().as_slice()],
            |row| row.get(0),
        )?;
        tx.execute(
            "UPDATE library_meta SET generation=generation+1 WHERE singleton=1",
            [],
        )?;
        let payload = json!({"request":{"snapshot":snapshot,"editGeneration":edit_generation,
            "change":{"kind":"audit_incomplete","held":finding.is_some()}},"generation":generation});
        tx.execute("INSERT INTO outbox(request_id,entity_id,operation,payload_json,created_at) VALUES(?1,?2,'discovery',?3,?4)",
            params![request.as_bytes().as_slice(),work.as_bytes().as_slice(),payload.to_string(),now])?;
        tx.commit()?;
        Ok(())
    }

    /// 取得元の鍵の順で全件をたどる。本文の原本は開かない。
    pub fn discoveries(
        &self,
        status: Option<&str>,
        after: Option<(&str, &str)>,
        limit: usize,
    ) -> Result<Vec<Discovery>> {
        if !(1..=1000).contains(&limit)
            || status.is_some_and(|v| !matches!(v, "pending" | "held" | "dismissed" | "deferred"))
        {
            return Err(Error::Integrity("候補の範囲が不正です".into()));
        }
        let conn = self.reader()?;
        let (provider, key) = after
            .map(|(p, k)| (Some(p), Some(k)))
            .unwrap_or((None, None));
        let mut statement=conn.prepare(&format!("SELECT {COLUMNS} FROM {TABLES}
            WHERE (?1 IS NULL OR c.decision=?1 OR (?1='deferred' AND c.decision IN ('held','dismissed')))
              AND (?2 IS NULL OR (x.provider,x.external_key)>(?2,?3))
            ORDER BY x.provider,x.external_key,c.id LIMIT ?4"))?;
        let rows = statement
            .query_map(params![status, provider, key, limit as i64], row)?
            .collect::<std::result::Result<_, _>>()?;
        Ok(rows)
    }

    pub fn discovery(&self, provider: &str, key: &str) -> Result<Option<Discovery>> {
        let conn = self.reader()?;
        let mut statement=conn.prepare(&format!("SELECT {COLUMNS} FROM {TABLES} WHERE x.provider=?1 AND x.external_key=?2 ORDER BY c.updated_at DESC,c.id DESC"))?;
        let rows = statement
            .query_map(params![provider, key], row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if rows.len() > 1 {
            return Err(Error::Integrity(
                "同じ作品の候補が複数あり、判断を確定できません".into(),
            ));
        }
        Ok(rows.into_iter().next())
    }

    /// 圧縮した元 JSON の字句を保って返す。候補に属するファイルだけを読む。
    pub fn discovery_payload(&self, candidate: LibraryId) -> Result<String> {
        let conn = self.reader()?;
        let hash: Vec<u8> = conn.query_row(
            "SELECT payload_file_id FROM discovery_candidates WHERE id=?1",
            [candidate.as_bytes().as_slice()],
            |r| r.get(0),
        )?;
        let hash = Fingerprint(
            hash.try_into()
                .map_err(|_| Error::Integrity("候補の原本識別子が不正です".into()))?,
        );
        let file = Self::file_from_conn(&conn, hash)?;
        if file.length > super::job_history::MAX_JOB_PAYLOAD_BYTES {
            return Err(Error::Unsupported(
                "候補の原本が読み込み上限を超えています".into(),
            ));
        }
        let mut bytes = Vec::new();
        self.objects.export(&file, &mut bytes)?;
        String::from_utf8(bytes)
            .map_err(|_| Error::Integrity("候補の原本が UTF-8 ではありません".into()))
    }

    /// 自動の再発見は利用者の非表示を戻さない。保留と原入力の差し替えも同じ確定に含める。
    pub fn record_discovery(
        &self,
        request: LibraryId,
        input: DiscoveryInput<'_>,
        held: bool,
    ) -> Result<()> {
        self.record_discovery_input(request, input, held, None, false)
    }

    /// 明示した再確認の結果だけが保留を解除できる。通信開始時の判断の世代を照合する。
    pub fn recheck_discovery(
        &self,
        request: LibraryId,
        input: DiscoveryInput<'_>,
        expected: i64,
    ) -> Result<()> {
        self.record_discovery_input(request, input, false, Some(expected), true)
    }

    fn record_discovery_input(
        &self,
        request: LibraryId,
        input: DiscoveryInput<'_>,
        held: bool,
        expected: Option<i64>,
        release: bool,
    ) -> Result<()> {
        validate_key(input.provider, input.key)?;
        let kind = match input.kind {
            "new" | "sequel" => "new",
            "revision" => "revision",
            _ => return Err(Error::Integrity("候補の種類が不正です".into())),
        };
        if input.title.chars().count() > 2000
            || input.payload.len() as u64 > super::job_history::MAX_JOB_PAYLOAD_BYTES
        {
            return Err(Error::Unsupported(
                "候補が読み込み上限を超えています".into(),
            ));
        }
        let payload: Value = serde_json::from_str(input.payload)?;
        let hold = payload
            .get("holdKind")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty());
        let stored = self.objects.put_stored(input.payload.as_bytes())?;
        let prepared: super::repository::PreparedFile = stored.into();
        let change = json!({"kind":"observe","provider":input.provider,"key":input.key,"candidate_kind":kind,"title":input.title,"file":prepared.file.id,"target_type":input.target_type,"held":held,"release":release});
        self.mutate_discovery(
            request,
            input.provider,
            input.key,
            expected,
            change,
            Some((&input, kind, hold, prepared)),
        )
    }

    /// 利用者の判断は要求 ID と期待世代で一度だけ確定する。解除しても閲覧制限は保留を続ける。
    pub fn decide_discovery(
        &self,
        request: LibraryId,
        provider: &str,
        key: &str,
        expected: Option<i64>,
        dismissed: bool,
    ) -> Result<()> {
        self.mutate_discovery(
            request,
            provider,
            key,
            expected,
            json!({"kind":"decide","dismissed":dismissed}),
            None,
        )
    }

    /// 保存できた候補を片付ける。遅れて終わった保存で、利用者の非表示は取り消さない。
    pub fn clear_discovery(
        &self,
        request: LibraryId,
        provider: &str,
        key: &str,
        unavailable_only: bool,
    ) -> Result<()> {
        self.mutate_discovery(
            request,
            provider,
            key,
            None,
            json!({"kind":"clear","unavailable_only":unavailable_only}),
            None,
        )
    }

    fn mutate_discovery(
        &self,
        request: LibraryId,
        provider: &str,
        key: &str,
        expected: Option<i64>,
        change: Value,
        observed: Option<Observation<'_>>,
    ) -> Result<()> {
        validate_key(provider, key)?;
        let input = json!({"provider":provider,"key":key,"expected":expected,"change":change});
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let previous: Option<(String, String)> = tx
            .query_row(
                "SELECT operation,payload_json FROM outbox WHERE request_id=?1",
                [request.as_bytes().as_slice()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((operation, payload)) = previous {
            if operation != "discovery"
                || serde_json::from_str::<Value>(&payload)?["request"] != input
            {
                return Err(Error::Integrity(
                    "要求 ID が別の候補操作に使われています".into(),
                ));
            }
            return Ok(());
        }
        let now = chrono::Utc::now().timestamp_millis();
        let work = if observed.is_some() {
            Self::relation_entity(&tx, provider, EntityKind::Work, key, now)?
        } else {
            let work:Option<Vec<u8>>=tx.query_row("SELECT entity_id FROM external_identities WHERE provider=?1 AND kind='work' AND external_key=?2",params![provider,key],|r|r.get(0)).optional()?;
            let Some(work) = work else {
                if change["kind"] == "clear" {
                    return Ok(());
                }
                return Err(Error::ResourceUnavailable);
            };
            LibraryId::from_slice(&work)
                .map_err(|_| Error::Integrity("候補の作品 ID が不正です".into()))?
        };
        let generation: i64 = tx.query_row(
            "SELECT coalesce((SELECT generation FROM discovery_state WHERE work_id=?1),0)",
            [work.as_bytes().as_slice()],
            |r| r.get(0),
        )?;
        if expected.is_some_and(|v| v != generation) {
            return Err(Error::Integrity(
                "候補の判断が先に変更されました。読み直してください".into(),
            ));
        }
        let rows: Vec<(Vec<u8>, String)> = tx
            .prepare("SELECT id,decision FROM discovery_candidates WHERE work_id=?1")?
            .query_map([work.as_bytes().as_slice()], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?;
        if rows.len() > 1 {
            return Err(Error::Integrity("同じ作品の候補が複数あります".into()));
        }
        if let Some((candidate, kind, hold, file)) = observed {
            let preserve = change["release"] != true
                && change["held"] != true
                && rows
                    .first()
                    .is_some_and(|r| matches!(r.1.as_str(), "held" | "dismissed"));
            if !preserve {
                Self::register_file(&tx, &file)?;
                let id = rows
                    .first()
                    .map(|r| r.0.clone())
                    .unwrap_or_else(|| request.as_bytes().to_vec());
                let status = match rows.first().map(|r| r.1.as_str()) {
                    _ if change["release"] == true => "pending",
                    Some("dismissed") => "dismissed",
                    _ if change["held"] == true => "held",
                    Some("held") => "held",
                    _ => "pending",
                };
                // 保留の種類は索引用。非表示の理由は原本へ残し、解除時にそこから判定する。
                let hold = if status == "held" { hold } else { None };
                tx.execute("INSERT INTO discovery_candidates(id,work_id,kind,title,decision,hold_kind,payload_file_id,target_type,first_seen_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?9)
                ON CONFLICT(id) DO UPDATE SET kind=excluded.kind,title=excluded.title,decision=excluded.decision,hold_kind=excluded.hold_kind,payload_file_id=excluded.payload_file_id,target_type=excluded.target_type,updated_at=excluded.updated_at",
                params![id,work.as_bytes().as_slice(),kind,candidate.title,status,hold,file.file.id.0.as_slice(),candidate.target_type,now])?;
                let value: Value = serde_json::from_str(candidate.payload)?;
                tx.execute("INSERT INTO discovery_summaries(candidate_id,payload_file_id,summary_json) VALUES(?1,?2,?3) ON CONFLICT(candidate_id) DO UPDATE SET payload_file_id=excluded.payload_file_id,summary_json=excluded.summary_json",params![id,file.file.id.0.as_slice(),summary(&value).to_string()])?;
            }
        } else if change["kind"] == "clear" {
            let removed=tx.execute("DELETE FROM discovery_candidates WHERE work_id=?1 AND decision<>'dismissed' AND (?2=0 OR (decision='held' AND hold_kind='unavailable'))",params![work.as_bytes().as_slice(),change["unavailable_only"]==true])?;
            if removed == 0 {
                tx.commit()?;
                return Ok(());
            }
        } else {
            let Some((candidate, _)) = rows.first() else {
                return Err(Error::ResourceUnavailable);
            };
            let candidate = LibraryId::from_slice(candidate)
                .map_err(|_| Error::Integrity("候補 ID が不正です".into()))?;
            let hash: Vec<u8> = tx.query_row(
                "SELECT payload_file_id FROM discovery_candidates WHERE id=?1",
                [candidate.as_bytes().as_slice()],
                |r| r.get(0),
            )?;
            let file = Self::file_from_conn(
                &tx,
                Fingerprint(
                    hash.try_into()
                        .map_err(|_| Error::Integrity("原本 ID が不正です".into()))?,
                ),
            )?;
            let mut bytes = Vec::new();
            if file.length > super::job_history::MAX_JOB_PAYLOAD_BYTES {
                return Err(Error::Unsupported(
                    "候補の原本が読み込み上限を超えています".into(),
                ));
            }
            self.objects.export(&file, &mut bytes)?;
            let payload: Value = serde_json::from_slice(&bytes)?;
            let restricted = payload.get("holdKind").and_then(Value::as_str) == Some("restricted");
            let dismissed = change["dismissed"] == true;
            tx.execute("UPDATE discovery_candidates SET decision=?2,hold_kind=?3,updated_at=?4 WHERE work_id=?1",params![work.as_bytes().as_slice(),if dismissed {"dismissed"} else if restricted {"held"} else {"pending"},if !dismissed&&restricted {Some("restricted")} else {None},now])?;
        }
        tx.execute("INSERT INTO discovery_state(work_id,generation) VALUES(?1,?2) ON CONFLICT(work_id) DO UPDATE SET generation=excluded.generation",params![work.as_bytes().as_slice(),generation+1])?;
        tx.execute(
            "UPDATE library_meta SET generation=generation+1 WHERE singleton=1",
            [],
        )?;
        tx.execute("INSERT INTO outbox(request_id,entity_id,operation,payload_json,created_at) VALUES(?1,?2,'discovery',?3,?4)",params![request.as_bytes().as_slice(),work.as_bytes().as_slice(),json!({"request":input,"generation":generation+1}).to_string(),now])?;
        tx.commit()?;
        Ok(())
    }
}
