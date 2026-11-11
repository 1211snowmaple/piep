use super::{
    Capture, CaptureResult, CaptureSource, EntityKind, Error, Fingerprint, Library, LibraryId,
    ReaderBinding, Result,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::io::Cursor;

pub(super) const MAX_ACQUISITION_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

/// 取得前の状態。ID はアクセス権ではなく、書き手の処理を結び付ける識別子。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AcquisitionTicket {
    pub id: LibraryId,
    pub entity: LibraryId,
    pub provider: String,
    pub kind: EntityKind,
    pub external_key: String,
    pub route: String,
    pub epoch: i64,
    pub base_snapshot: Option<LibraryId>,
    pub base_generation: i64,
    pub started_at: i64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AcquisitionState {
    Pending,
    Complete,
    Superseded,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcquisitionInfo {
    pub ticket: AcquisitionTicket,
    pub state: AcquisitionState,
    pub input_manifest: Option<Fingerprint>,
    pub result: Option<CaptureResult>,
    /// 再起動後も解釈前の応答へ戻れるよう、受信済みの原本を列挙する。
    pub payloads: Vec<AcquisitionPayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reader: Option<AcquisitionReaderOutput>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AcquisitionReaderOutput {
    pub entity: LibraryId,
    pub snapshot: LibraryId,
    pub recipe: i64,
    pub bindings_hash: Fingerprint,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AcquisitionPayload {
    pub slot: String,
    pub file: Fingerprint,
}

pub(super) type RawPart = AcquisitionPayload;

pub(super) struct CompletionContext {
    pub ticket: AcquisitionTicket,
    pub parts: Vec<RawPart>,
    pub reader_bindings: Option<Vec<ReaderBinding>>,
}

#[derive(Serialize, Deserialize)]
pub(super) struct AcquisitionManifest {
    pub format: String,
    pub ticket: AcquisitionTicket,
    pub parts: Vec<RawPart>,
    pub capture: Box<serde_json::value::RawValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reader_bindings: Option<Vec<ReaderBinding>>,
}

fn sql_id(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<LibraryId> {
    let bytes: Vec<u8> = row.get(column)?;
    LibraryId::from_slice(&bytes).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Blob, Box::new(e))
    })
}
fn optional_id(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<Option<LibraryId>> {
    row.get::<_, Option<Vec<u8>>>(column)?
        .map(|b| {
            LibraryId::from_slice(&b).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    column,
                    rusqlite::types::Type::Blob,
                    Box::new(e),
                )
            })
        })
        .transpose()
}
fn fingerprint(bytes: Vec<u8>) -> Result<Fingerprint> {
    Ok(Fingerprint(bytes.try_into().map_err(|_| {
        Error::Integrity("受信記録のファイル識別子が不正です".into())
    })?))
}

fn reader_output(conn: &Connection, id: LibraryId) -> Result<Option<AcquisitionReaderOutput>> {
    conn.query_row("SELECT entity_id,snapshot_id,recipe,bindings_hash FROM acquisition_reader_outputs WHERE attempt_id=?1",[id.as_bytes().as_slice()],|r|Ok((sql_id(r,0)?,sql_id(r,1)?,r.get::<_,i64>(2)?,r.get::<_,Vec<u8>>(3)?))).optional()?
        .map(|(entity,snapshot,recipe,hash)|Ok(AcquisitionReaderOutput {entity,snapshot,recipe,bindings_hash:fingerprint(hash)?})).transpose()
}

pub(super) fn load_info(conn: &Connection, id: LibraryId) -> Result<AcquisitionInfo> {
    let mut statement=conn.prepare("SELECT a.id,a.entity_id,a.provider,a.kind,a.external_key,a.route,a.epoch,a.base_snapshot_id,a.base_generation,a.started_at,a.state,a.input_manifest_file_id,a.result_snapshot_id,o.created_revision FROM acquisition_attempts a LEFT JOIN acquisition_observations o ON o.id=a.observation_id WHERE a.id=?1")?;
    let mut rows = statement.query([id.as_bytes().as_slice()])?;
    let row = rows
        .next()?
        .ok_or_else(|| Error::Integrity("取得開始の記録がありません".into()))?;
    let kind = match row.get::<_, String>(3)?.as_str() {
        "work" => EntityKind::Work,
        "person" => EntityKind::Person,
        "series" => EntityKind::Series,
        _ => return Err(Error::Integrity("取得対象の種類が不正です".into())),
    };
    let state = match row.get::<_, String>(10)?.as_str() {
        "pending" => AcquisitionState::Pending,
        "complete" => AcquisitionState::Complete,
        "superseded" => AcquisitionState::Superseded,
        "cancelled" => AcquisitionState::Cancelled,
        _ => return Err(Error::Integrity("取得処理の状態が不正です".into())),
    };
    let entity = sql_id(row, 1)?;
    Ok(AcquisitionInfo {
        ticket: AcquisitionTicket {
            id: sql_id(row, 0)?,
            entity,
            provider: row.get(2)?,
            kind,
            external_key: row.get(4)?,
            route: row.get(5)?,
            epoch: row.get(6)?,
            base_snapshot: optional_id(row, 7)?,
            base_generation: row.get(8)?,
            started_at: row.get(9)?,
        },
        state,
        input_manifest: row
            .get::<_, Option<Vec<u8>>>(11)?
            .map(fingerprint)
            .transpose()?,
        result: optional_id(row, 12)?
            .map(|snapshot_id| {
                Ok::<_, Error>(CaptureResult {
                    entity_id: entity,
                    snapshot_id,
                    created_revision: row
                        .get::<_, Option<bool>>(13)?
                        .ok_or_else(|| Error::Integrity("完了した取得の観測がありません".into()))?,
                })
            })
            .transpose()?,
        payloads: raw_parts(conn, id)?,
        reader: reader_output(conn, id)?,
    })
}

pub(super) fn raw_parts(conn: &Connection, id: LibraryId) -> Result<Vec<RawPart>> {
    let mut stmt=conn.prepare("SELECT slot,file_id FROM acquisition_inputs WHERE attempt_id=?1 AND role='raw_part' ORDER BY slot")?;
    let records = stmt
        .query_map([id.as_bytes().as_slice()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?
        .map(|r| {
            let (slot, file) = r?;
            Ok(RawPart {
                slot,
                file: fingerprint(file)?,
            })
        })
        .collect();
    records
}

impl Library {
    /// 上流へ接続する前に呼ぶ。同じ entity の後から始まった取得が古い結果に負けない。
    pub fn begin_acquisition(
        &self,
        id: LibraryId,
        provider: &str,
        kind: EntityKind,
        key: &str,
        route: &str,
    ) -> Result<AcquisitionInfo> {
        if provider.is_empty()
            || provider.len() > 128
            || key.is_empty()
            || key.len() > 4096
            || route.is_empty()
            || route.len() > 512
            || kind == EntityKind::Collection
        {
            return Err(Error::Integrity("取得開始の対象が不正です".into()));
        }
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM acquisition_attempts WHERE id=?1)",
            [id.as_bytes().as_slice()],
            |r| r.get(0),
        )?;
        if exists {
            let info = load_info(&tx, id)?;
            if info.ticket.provider != provider
                || info.ticket.kind != kind
                || info.ticket.external_key != key
                || info.ticket.route != route
            {
                return Err(Error::Integrity(
                    "同じ要求 ID が異なる取得に使われました".into(),
                ));
            }
            return Ok(info);
        }
        let used: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM acquisition_observations WHERE request_id=?1)",
            [id.as_bytes().as_slice()],
            |r| r.get(0),
        )?;
        if used {
            return Err(Error::Integrity(
                "この要求 ID は既に別の取得に使われています".into(),
            ));
        }
        let now = chrono::Utc::now().timestamp_millis();
        let entity = Self::relation_entity(&tx, provider, kind, key, now)?;
        let current: Option<(Vec<u8>, i64)> = tx
            .query_row(
                "SELECT snapshot_id,generation FROM entity_current WHERE entity_id=?1",
                [entity.as_bytes().as_slice()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (base, generation) = current.map(|(s, g)| (Some(s), g)).unwrap_or((None, 0));
        let old: i64 = tx
            .query_row(
                "SELECT epoch FROM acquisition_heads WHERE entity_id=?1",
                [entity.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let epoch = old
            .checked_add(1)
            .ok_or_else(|| Error::Integrity("取得の世代が上限を超えました".into()))?;
        tx.execute("INSERT INTO acquisition_heads VALUES(?1,?2) ON CONFLICT(entity_id) DO UPDATE SET epoch=excluded.epoch",params![entity.as_bytes().as_slice(),epoch])?;
        tx.execute("INSERT INTO acquisition_attempts(id,entity_id,provider,kind,external_key,route,epoch,base_snapshot_id,base_generation,started_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![id.as_bytes().as_slice(),entity.as_bytes().as_slice(),provider,kind.name(),key,route,epoch,base,generation,now])?;
        tx.execute(
            "UPDATE library_meta SET generation=generation+1 WHERE singleton=1",
            [],
        )?;
        let info = load_info(&tx, id)?;
        tx.commit()?;
        Ok(info)
    }

    pub fn acquisition_status(&self, id: LibraryId) -> Result<AcquisitionInfo> {
        let conn = self.reader()?;
        load_info(&conn, id)
    }

    /// 受信した原本を解釈より前に保管する。閉じた処理へ新しい原本を追加しない。
    pub fn record_acquisition_payload(
        &self,
        id: LibraryId,
        slot: &str,
        source: &CaptureSource,
    ) -> Result<Fingerprint> {
        if slot.is_empty() || slot.len() > 1024 {
            return Err(Error::Integrity("受信記録の識別が不正です".into()));
        }
        {
            let conn = self.conn.lock().map_err(|_| Error::Busy)?;
            let info = load_info(&conn, id)?;
            let existing:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM acquisition_inputs WHERE attempt_id=?1 AND role='raw_part' AND slot=?2)",params![id.as_bytes().as_slice(),slot],|r|r.get(0))?;
            if info.state != AcquisitionState::Pending && !existing {
                return Err(Error::Integrity(
                    "終了した取得には受信記録を追加できません".into(),
                ));
            }
        }
        let prepared = source.store(&self.objects)?;
        let file = prepared.file.clone();
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let previous:Option<Vec<u8>>=tx.query_row("SELECT file_id FROM acquisition_inputs WHERE attempt_id=?1 AND role='raw_part' AND slot=?2",params![id.as_bytes().as_slice(),slot],|r|r.get(0)).optional()?;
        if let Some(previous) = previous {
            if previous != file.id.0 {
                return Err(Error::Integrity(
                    "同じ受信記録に異なるデータが送られました".into(),
                ));
            }
            return Ok(file.id);
        }
        if load_info(&tx, id)?.state != AcquisitionState::Pending {
            return Err(Error::Canceled);
        }
        Self::register_file(&tx, &prepared)?;
        tx.execute(
            "INSERT INTO acquisition_inputs VALUES(?1,'raw_part',?2,?3)",
            params![id.as_bytes().as_slice(), slot, file.id.0.as_slice()],
        )?;
        tx.execute(
            "UPDATE library_meta SET generation=generation+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(file.id)
    }

    pub fn complete_acquisition(&self, id: LibraryId, capture: Capture) -> Result<CaptureResult> {
        self.complete_with_reader(id, capture, None)
    }

    /// 保存版・現在版・読書用の対応・取得の完了をまとめて確定する。
    pub fn complete_acquisition_with_reader(
        &self,
        id: LibraryId,
        capture: Capture,
        mut bindings: Vec<ReaderBinding>,
    ) -> Result<CaptureResult> {
        if capture.kind != EntityKind::Work {
            return Err(Error::Integrity(
                "読書用の保存は作品に対してだけ行えます".into(),
            ));
        }
        super::reader::normalize_input_bindings(&mut bindings)?;
        self.complete_with_reader(id, capture, Some(bindings))
    }

    fn complete_with_reader(
        &self,
        id: LibraryId,
        capture: Capture,
        reader_bindings: Option<Vec<ReaderBinding>>,
    ) -> Result<CaptureResult> {
        let context = {
            let conn = self.conn.lock().map_err(|_| Error::Busy)?;
            let info = load_info(&conn, id)?;
            if info.state == AcquisitionState::Cancelled {
                return Err(Error::Canceled);
            }
            if capture.request_id != id
                || capture.provider != info.ticket.provider
                || capture.external_key != info.ticket.external_key
                || capture.kind != info.ticket.kind
                || capture.route != info.ticket.route
            {
                return Err(Error::Integrity(
                    "取得開始の対象と保存対象が一致しません".into(),
                ));
            }
            CompletionContext {
                ticket: info.ticket,
                parts: info.payloads,
                reader_bindings,
            }
        };
        self.capture_prepared(capture, None, Some(context))
    }

    pub fn cancel_acquisition(&self, id: LibraryId) -> Result<AcquisitionInfo> {
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let info = load_info(&tx, id)?;
        if info.state != AcquisitionState::Pending {
            return Ok(info);
        }
        tx.execute(
            "UPDATE acquisition_attempts SET state='cancelled',finished_at=?2 WHERE id=?1",
            params![
                id.as_bytes().as_slice(),
                chrono::Utc::now().timestamp_millis()
            ],
        )?;
        tx.execute(
            "UPDATE library_meta SET generation=generation+1 WHERE singleton=1",
            [],
        )?;
        let info = load_info(&tx, id)?;
        tx.commit()?;
        Ok(info)
    }
}

pub(super) fn encode_manifest(context: &CompletionContext, request: Vec<u8>) -> Result<Vec<u8>> {
    let capture = serde_json::value::RawValue::from_string(
        String::from_utf8(request)
            .map_err(|_| Error::Integrity("取得入力が UTF-8 ではありません".into()))?,
    )?;
    let bytes = serde_json::to_vec(&AcquisitionManifest {
        format: if context.reader_bindings.is_some() {
            "capture-request-v3"
        } else {
            "capture-request-v2"
        }
        .into(),
        ticket: context.ticket.clone(),
        parts: context.parts.clone(),
        capture,
        reader_bindings: context.reader_bindings.clone(),
    })?;
    if bytes.len() as u64 > MAX_ACQUISITION_MANIFEST_BYTES {
        return Err(Error::Unsupported(
            "取得入力が記録の読み込み上限を超えています".into(),
        ));
    }
    Ok(bytes)
}

pub(super) fn read_manifest(
    library: &Library,
    conn: &Connection,
    file: Fingerprint,
) -> Result<AcquisitionManifest> {
    let file = Library::file_from_conn(conn, file)?;
    if file.length > MAX_ACQUISITION_MANIFEST_BYTES {
        return Err(Error::Unsupported(
            "取得入力が記録の読み込み上限を超えています".into(),
        ));
    }
    let mut bytes = Vec::new();
    library.objects.export(&file, &mut bytes)?;
    Ok(serde_json::from_reader(Cursor::new(bytes))?)
}

type InputFiles = std::collections::BTreeMap<(String, String), Fingerprint>;

struct ObservationLink {
    entity: Vec<u8>,
    request: Vec<u8>,
    snapshot: Vec<u8>,
    route: String,
    raw: Option<Vec<u8>>,
    body: Option<Vec<u8>>,
}

fn inputs(conn: &Connection, id: LibraryId) -> Result<InputFiles> {
    let mut stmt = conn.prepare(
        "SELECT role,slot,file_id FROM acquisition_inputs WHERE attempt_id=?1 ORDER BY role,slot",
    )?;
    let rows = stmt.query_map([id.as_bytes().as_slice()], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Vec<u8>>(2)?,
        ))
    })?;
    rows.map(|row| {
        let (role, slot, file) = row?;
        Ok(((role, slot), fingerprint(file)?))
    })
    .collect()
}

fn invalid_manifest() -> Error {
    Error::Integrity("取得の入力記録と保存された参照が一致しません".into())
}

fn manifest_files(
    conn: &Connection,
    manifest: &AcquisitionManifest,
    request: Fingerprint,
) -> Result<InputFiles> {
    // 原本の JSON を再直列化せず、ファイル参照だけを型付きで読む。
    let capture: serde_json::Value = serde_json::from_str(manifest.capture.get())?;
    let fields = capture
        .as_array()
        .filter(|v| v.len() == 13)
        .ok_or_else(invalid_manifest)?;
    let ticket = &manifest.ticket;
    let reader_format = match (manifest.format.as_str(), &manifest.reader_bindings) {
        ("capture-request-v2", None) => true,
        ("capture-request-v3", Some(bindings)) if ticket.kind == EntityKind::Work => {
            let mut normalized = bindings.clone();
            super::reader::normalize_input_bindings(&mut normalized)?;
            &normalized == bindings
        }
        _ => false,
    };
    if !reader_format
        || fields[0] != "capture-request-v1"
        || fields[1] != ticket.provider
        || fields[2] != ticket.external_key
        || fields[3] != ticket.kind.name()
        || fields[4] != ticket.route
        || !fields[12].is_null()
    {
        return Err(invalid_manifest());
    }
    let mut result = InputFiles::new();
    let mut add = |role: &str, slot: &str, file: Fingerprint| -> Result<()> {
        if result.insert((role.into(), slot.into()), file).is_some() {
            return Err(invalid_manifest());
        }
        Ok(())
    };
    for part in &manifest.parts {
        add("raw_part", &part.slot, part.file)?;
    }
    add("raw", "", serde_json::from_value(fields[6].clone())?)?;
    add("request", "", request)?;
    let content_file = |value: &serde_json::Value| -> Result<Fingerprint> {
        let file: super::ContentFile = serde_json::from_value(value.clone())?;
        if Library::file_from_conn(conn, file.id)? != file {
            return Err(invalid_manifest());
        }
        Ok(file.id)
    };
    if !fields[9].is_null() {
        let body = fields[9]
            .as_array()
            .filter(|v| v.len() == 3)
            .ok_or_else(invalid_manifest)?;
        add("body", "", content_file(&body[0])?)?;
    }
    for media in fields[10].as_array().ok_or_else(invalid_manifest)? {
        let slot = media
            .get("slot")
            .and_then(|v| v.as_str())
            .ok_or_else(invalid_manifest)?;
        let file = media.get("file").ok_or_else(invalid_manifest)?;
        if !file.is_null() {
            add("media", slot, content_file(file)?)?;
        }
    }
    for attachment in fields[11].as_array().ok_or_else(invalid_manifest)? {
        let fields = attachment
            .as_array()
            .filter(|v| v.len() == 4)
            .ok_or_else(invalid_manifest)?;
        let slot = fields[0].as_str().ok_or_else(invalid_manifest)?;
        add("attachment", slot, content_file(&fields[3])?)?;
    }
    Ok(result)
}

impl Library {
    pub(super) fn verify_acquisitions(&self, conn: &Connection) -> Result<()> {
        let invalid_heads: bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM acquisition_heads h WHERE h.epoch IS NOT (SELECT max(a.epoch) FROM acquisition_attempts a WHERE a.entity_id=h.entity_id)) OR EXISTS(SELECT 1 FROM acquisition_attempts a LEFT JOIN acquisition_heads h ON h.entity_id=a.entity_id WHERE h.entity_id IS NULL) OR EXISTS(SELECT 1 FROM acquisition_attempts a WHERE NOT EXISTS(SELECT 1 FROM external_identities e WHERE e.entity_id=a.entity_id AND e.provider=a.provider AND e.kind=a.kind AND e.external_key=a.external_key))",[],|r|r.get(0))?;
        if invalid_heads {
            return Err(Error::Integrity(
                "取得の世代または対象が一致しません".into(),
            ));
        }
        let mut stmt = conn.prepare("SELECT id FROM acquisition_attempts ORDER BY id")?;
        for id in stmt.query_map([], |r| sql_id(r, 0))? {
            let id = id?;
            let info = load_info(conn, id)?;
            let stored = inputs(conn, id)?;
            let mut reader_bindings = None;
            match info.state {
                AcquisitionState::Pending | AcquisitionState::Cancelled => {
                    if stored.keys().any(|(role, _)| role != "raw_part") {
                        return Err(invalid_manifest());
                    }
                }
                AcquisitionState::Complete | AcquisitionState::Superseded => {
                    let request = info.input_manifest.ok_or_else(invalid_manifest)?;
                    let manifest = read_manifest(self, conn, request)?;
                    if manifest.ticket != info.ticket
                        || manifest.parts != raw_parts(conn, id)?
                        || manifest_files(conn, &manifest, request)? != stored
                    {
                        return Err(invalid_manifest());
                    }
                    reader_bindings = manifest.reader_bindings;
                }
            }
            match (&reader_bindings, &info.reader, info.state) {
                (Some(bindings), Some(reader), AcquisitionState::Complete) => {
                    if reader.entity != info.ticket.entity
                        || Some(reader.snapshot) != info.result.as_ref().map(|r| r.snapshot_id)
                        || reader.recipe != 1
                    {
                        return Err(invalid_manifest());
                    }
                    let plan =
                        Self::reader_bindings_from_conn(conn, reader.entity, reader.snapshot)?;
                    if plan.bindings_hash != reader.bindings_hash || &plan.bindings != bindings {
                        return Err(invalid_manifest());
                    }
                }
                (_, None, state)
                    if state != AcquisitionState::Complete || reader_bindings.is_none() => {}
                _ => return Err(invalid_manifest()),
            }
            // 完了以外は、観測や現在版を作らない。完了は同じ要求・原本を指す。
            let observation=conn.query_row("SELECT entity_id,request_hash,result_snapshot_id,route,raw_file_id,body_file_id FROM acquisition_observations WHERE request_id=?1",[id.as_bytes().as_slice()],|r|Ok(ObservationLink {entity:r.get(0)?,request:r.get(1)?,snapshot:r.get(2)?,route:r.get(3)?,raw:r.get(4)?,body:r.get(5)?})).optional()?;
            match (info.result, observation) {
                (None, None) => {}
                (Some(result), Some(observation)) if info.state == AcquisitionState::Complete => {
                    let expected = |role: &str| {
                        stored
                            .get(&(role.into(), String::new()))
                            .map(|fp| fp.0.to_vec())
                    };
                    let linked:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM acquisition_attempts a JOIN acquisition_observations o ON o.id=a.observation_id WHERE a.id=?1 AND o.request_id=a.id)",[id.as_bytes().as_slice()],|r|r.get(0))?;
                    if !linked
                        || observation.entity != info.ticket.entity.as_bytes()
                        || observation.snapshot != result.snapshot_id.as_bytes()
                        || Some(fingerprint(observation.request)?) != info.input_manifest
                        || observation.route != info.ticket.route
                        || observation.raw != expected("raw")
                        || observation.body != expected("body")
                    {
                        return Err(invalid_manifest());
                    }
                }
                _ => return Err(invalid_manifest()),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "acquisitions_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "acquisition_readers_tests.rs"]
mod reader_tests;
