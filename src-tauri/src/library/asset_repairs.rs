//! FANBOX の欠けた資産の取り直し。回数は作品の保存と一緒に `repair_attempts` へ確定し、
//! 同じ状態では有限回で止める。
use super::{repair_attempts, CaptureResult, Error, Fingerprint, Library, LibraryId, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

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

pub(super) fn record(
    conn: &Connection,
    result: &CaptureResult,
    request: LibraryId,
    observation: &AssetRepairObservation,
    at: i64,
) -> Result<()> {
    let next =
        repair_attempts::gap_fingerprint(conn, result.snapshot_id, &observation.expected_urls)?;
    repair_attempts::record(
        conn,
        result.entity_id,
        result.snapshot_id,
        next,
        observation.complete,
        Some(request),
        at,
    )?;
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
        let hash = repair_attempts::gap_fingerprint(&conn, snapshot, expected_urls)?;
        let attempts = repair_attempts::attempts(&conn, work, snapshot, hash)?;
        Ok(AssetRepairPlan {
            fingerprint: hash,
            attempts,
        })
    }

    pub(super) fn verify_asset_repairs(&self, conn: &Connection) -> Result<()> {
        let mut stmt=conn.prepare("SELECT s.snapshot_id,s.work_id,a.id,a.state,a.result_snapshot_id,a.input_manifest_file_id FROM repair_attempts s LEFT JOIN acquisition_attempts a ON a.id=s.request_id WHERE s.request_id IS NOT NULL")?;
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
            let expected =
                repair_attempts::gap_fingerprint(conn, snapshot, &observed.expected_urls)?;
            let valid:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM repair_attempts WHERE work_id=?1 AND request_id=?2 AND gap_fingerprint=?3 AND ((?4 AND attempts=0) OR (NOT ?4 AND attempts>0)))",params![work,request.as_bytes().as_slice(),expected.0.as_slice(),observed.complete],|r|r.get(0))?;
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
