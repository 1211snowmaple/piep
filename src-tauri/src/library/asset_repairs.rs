//! FANBOX の欠けた資産を取り直した状態。作品の保存と一緒に確定し、同じ状態では有限回で止める。
use super::{CaptureResult, Error, Fingerprint, Library, LibraryId, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

pub(crate) const MAX_REPAIR_ATTEMPTS: u32 = 3;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct AssetRepairObservation {
    pub expected_urls: Vec<String>,
    pub complete: bool,
}
impl AssetRepairObservation {
    pub fn normalize(&mut self) {
        self.expected_urls.sort();
        self.expected_urls.dedup();
    }
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AssetRepairPlan {
    pub fingerprint: Fingerprint,
    pub attempts: u32,
}

pub(super) fn fingerprint(
    conn: &Connection,
    snapshot: LibraryId,
    urls: &[String],
) -> Result<Fingerprint> {
    let mut urls = urls.to_vec();
    urls.sort();
    urls.dedup();
    let mut stmt=conn.prepare("SELECT 'media',m.slot,r.file_id FROM snapshot_media m LEFT JOIN media_renditions r ON r.id=m.rendition_id WHERE m.snapshot_id=?1 UNION ALL SELECT 'file',slot,file_id FROM snapshot_files WHERE snapshot_id=?1 ORDER BY 1,2")?;
    let held = stmt
        .query_map([snapshot.as_bytes().as_slice()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<Vec<u8>>>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Fingerprint::of(&serde_json::to_vec(&(
        "shelf-assets-v1",
        urls,
        held,
    ))?))
}

pub(super) fn record(
    conn: &Connection,
    result: &CaptureResult,
    request: LibraryId,
    base: Option<LibraryId>,
    observation: &AssetRepairObservation,
) -> Result<()> {
    let next = fingerprint(conn, result.snapshot_id, &observation.expected_urls)?;
    let prior:Option<(Vec<u8>,Vec<u8>,u32)>=conn.query_row("SELECT snapshot_id,state_fingerprint,attempts FROM asset_repair_state WHERE work_id=?1",[result.entity_id.as_bytes().as_slice()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let attempts = if observation.complete {
        0
    } else {
        match prior {
            Some((snapshot, hash, n))
                if !result.created_revision
                    && hash == next.0
                    && base.is_some_and(|b| b.as_bytes().as_slice() == snapshot) =>
            {
                n.saturating_add(1)
            }
            _ => 1,
        }
    };
    conn.execute("INSERT INTO asset_repair_state(work_id,snapshot_id,state_fingerprint,attempts,request_id) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(work_id) DO UPDATE SET snapshot_id=excluded.snapshot_id,state_fingerprint=excluded.state_fingerprint,attempts=excluded.attempts,request_id=excluded.request_id",params![result.entity_id.as_bytes().as_slice(),result.snapshot_id.as_bytes().as_slice(),next.0.as_slice(),attempts,request.as_bytes().as_slice()])?;
    Ok(())
}

impl Library {
    /// 照合した現在版に限って回数を読む。指紋が変われば、新しい状態として数え直す。
    pub(crate) fn asset_repair_plan(
        &self,
        work: LibraryId,
        snapshot: LibraryId,
        expected_urls: &[String],
    ) -> Result<AssetRepairPlan> {
        let conn = self.reader()?;
        let current:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM entity_current c JOIN library_items li ON li.work_id=c.entity_id WHERE c.entity_id=?1 AND c.snapshot_id=?2)",params![work.as_bytes().as_slice(),snapshot.as_bytes().as_slice()],|r|r.get(0))?;
        if !current {
            return Err(Error::Superseded);
        }
        let hash = fingerprint(&conn, snapshot, expected_urls)?;
        let attempts=conn.query_row("SELECT attempts FROM asset_repair_state WHERE work_id=?1 AND snapshot_id=?2 AND state_fingerprint=?3",params![work.as_bytes().as_slice(),snapshot.as_bytes().as_slice(),hash.0.as_slice()],|r|r.get::<_,u32>(0)).optional()?.unwrap_or(0);
        Ok(AssetRepairPlan {
            fingerprint: hash,
            attempts,
        })
    }

    pub(super) fn verify_asset_repairs(&self, conn: &Connection) -> Result<()> {
        let mut stmt=conn.prepare("SELECT s.snapshot_id,s.work_id,a.id,a.state,a.result_snapshot_id,a.input_manifest_file_id FROM asset_repair_state s LEFT JOIN acquisition_attempts a ON a.id=s.request_id")?;
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<Vec<u8>>>(4)?,
                r.get::<_, Option<Vec<u8>>>(5)?,
            ))
        })? {
            let (snapshot, work, request, status, result, manifest) = row?;
            if status != "complete" || result.as_ref() != Some(&snapshot) {
                return Err(Error::Integrity(
                    "資産の取り直しと取得の完了が一致しません".into(),
                ));
            }
            let request = LibraryId::from_slice(&request)
                .map_err(|_| Error::Integrity("資産の取り直しの要求 ID が不正です".into()))?;
            let manifest = manifest
                .ok_or_else(|| Error::Integrity("資産の取り直しの入力がありません".into()))?;
            let manifest = super::acquisitions::read_manifest(
                self,
                conn,
                Fingerprint(
                    manifest
                        .try_into()
                        .map_err(|_| Error::Integrity("資産の取り直しの指紋が不正です".into()))?,
                ),
            )?;
            let observed = manifest
                .asset_repair
                .ok_or_else(|| Error::Integrity("資産の取り直しの入力に状態がありません".into()))?;
            let snapshot = LibraryId::from_slice(&snapshot)
                .map_err(|_| Error::Integrity("資産の保存版が不正です".into()))?;
            let expected = fingerprint(conn, snapshot, &observed.expected_urls)?;
            let valid:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM asset_repair_state WHERE work_id=?1 AND request_id=?2 AND state_fingerprint=?3 AND ((?4 AND attempts=0) OR (NOT ?4 AND attempts>0)))",params![work,request.as_bytes().as_slice(),expected.0.as_slice(),observed.complete],|r|r.get(0))?;
            if !valid {
                return Err(Error::Integrity(
                    "資産の取り直しの保存状態が入力と一致しません".into(),
                ));
            }
        }
        Ok(())
    }
}

/// 試験で取得の観測を組むための URL 群。本番の観測は保存の対象から作る。
#[cfg(test)]
pub(crate) fn fanbox_expected_urls(data: &serde_json::Value) -> Vec<String> {
    let data = super::execution::metadata_post(data);
    let mut urls = Vec::new();
    if let Some(url) = data
        .get("coverImageUrl")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            data.pointer("/cover/url")
                .and_then(serde_json::Value::as_str)
        })
    {
        urls.push(url.into());
    }
    if let Some(body) = data.get("body") {
        for (field, url_field) in [
            ("imageMap", "originalUrl"),
            ("fileMap", "url"),
            ("images", "originalUrl"),
            ("files", "url"),
        ] {
            let items = body
                .get(field)
                .and_then(|v| {
                    if field.ends_with("Map") {
                        v.as_object().map(|m| m.values().collect::<Vec<_>>())
                    } else {
                        v.as_array().map(|a| a.iter().collect::<Vec<_>>())
                    }
                })
                .unwrap_or_default();
            for item in items {
                if let Some(url) = item.get(url_field).and_then(serde_json::Value::as_str) {
                    urls.push(url.into());
                }
            }
        }
    }
    urls.sort();
    urls.dedup();
    urls
}

#[cfg(test)]
#[path = "asset_repairs_tests.rs"]
mod tests;
