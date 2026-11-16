//! 候補の正本を選ぶ。切り替え後の判断と原入力は棚へ、実行中の項目だけは既存のキューへ渡す。

use crate::database::{
    UpdateCandidateInput, UpdateCandidateRow, UpdateJobItemInput, UpdateWorkMetadata,
};
use crate::library::{Discovery, DiscoveryInput, Library, LibraryId};
use crate::AppState;
use std::sync::Arc;

pub(crate) struct Candidates<'a> {
    state: &'a AppState,
    native: Option<Arc<Library>>,
}

#[cfg(test)]
#[path = "candidates_tests.rs"]
mod tests;
impl AppState {
    pub(crate) fn candidates(&self) -> Candidates<'_> {
        Candidates {
            state: self,
            native: self.library_v2.switched(),
        }
    }
}
fn date(value: i64) -> Result<String, String> {
    chrono::DateTime::from_timestamp_millis(value)
        .map(|v| v.to_rfc3339())
        .ok_or("候補の日時が不正です".into())
}
impl Candidates<'_> {
    pub(crate) fn generation(&self, source: &str, key: &str) -> Result<Option<i64>, String> {
        self.native
            .as_ref()
            .map(|library| {
                library
                    .discovery(source, key)
                    .map(|v| v.map(|v| v.generation))
                    .map_err(|e| e.to_string())
            })
            .transpose()
            .map(Option::flatten)
    }
    pub(crate) fn recheck_available(
        &self,
        input: &UpdateCandidateInput,
        expected: Option<i64>,
    ) -> Result<(), String> {
        if let Some(library) = &self.native {
            let expected = expected
                .ok_or("再確認した候補の判断の世代がありません。再確認をやり直してください")?;
            return library
                .recheck_discovery(
                    LibraryId::now_v7(),
                    DiscoveryInput {
                        provider: &input.source,
                        key: &input.source_id,
                        kind: &input.kind,
                        title: &input.title,
                        payload: &input.payload_json,
                        target_type: input.target_type.as_deref(),
                    },
                    expected,
                )
                .map_err(|e| e.to_string());
        }
        self.state.db.upsert_update_candidate(input)?;
        self.state
            .db
            .set_update_candidate_status(&input.source, &input.source_id, "pending")
    }
    pub(crate) fn saved(&self, source: &str, key: &str) -> Result<bool, String> {
        if let Some(library) = &self.native {
            return super::update_jobs::shelf::saved_card(library, source, key)
                .map(|v| v.is_some());
        }
        self.state
            .db
            .get_download_by_source(source, key)
            .map(|v| v.is_some())
    }
    fn rows(
        &self,
        status: Option<&str>,
        after: Option<(&str, &str)>,
        limit: usize,
    ) -> Result<Vec<Discovery>, String> {
        self.native
            .as_ref()
            .ok_or("新しい棚がありません")?
            .discoveries(status, after, limit)
            .map_err(|e| e.to_string())
    }
    fn all(&self, status: Option<&str>) -> Result<Vec<Discovery>, String> {
        let mut cursor = None;
        let mut rows = Vec::new();
        loop {
            let page = self.rows(
                status,
                cursor
                    .as_ref()
                    .map(|(p, k): &(String, String)| (p.as_str(), k.as_str())),
                500,
            )?;
            let Some(last) = page.last() else {
                break;
            };
            cursor = Some((last.provider.clone(), last.key.clone()));
            rows.extend(page);
        }
        Ok(rows)
    }
    fn convert(&self, entry: Discovery, display: bool) -> Result<UpdateCandidateRow, String> {
        let library = self.native.as_ref().ok_or("新しい棚がありません")?;
        let payload = if display {
            entry
                .summary
                .as_ref()
                .ok_or("候補の表示情報を準備できません")?
                .to_string()
        } else {
            library
                .discovery_payload(entry.id)
                .map_err(|e| e.to_string())?
        };
        let value: serde_json::Value = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
        let mut metadata = UpdateWorkMetadata::from_payload(&value);
        let mut work_id = None;
        if display {
            if let Some(card) =
                super::update_jobs::shelf::saved_card(library, &entry.provider, &entry.key)?
            {
                work_id = Some(card.id);
                metadata.author_name = metadata.author_name.or(Some(card.author_name));
                metadata.published_at = metadata
                    .published_at
                    .or(card.published_at.map(date).transpose()?);
                metadata.updated_at = metadata
                    .updated_at
                    .or(card.updated_at.map(date).transpose()?);
                metadata.saved_at = Some(date(card.saved_at)?);
            }
        }
        Ok(UpdateCandidateRow {
            work_id,
            metadata,
            source: entry.provider,
            source_id: entry.key,
            kind: value
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .filter(|v| *v == "sequel")
                .unwrap_or(&entry.kind)
                .into(),
            title: entry.title,
            payload_json: if display {
                serde_json::json!({"holdKind":value.get("holdKind"),"holdReason":value.get("holdReason")}).to_string()
            } else {
                payload
            },
            target_type: entry.target_type,
            status: entry.decision,
            first_seen_at: date(entry.first_seen_at)?,
            updated_at: date(entry.updated_at)?,
            download_id: None,
        })
    }
    pub(crate) fn upsert_update_candidate(
        &self,
        input: &UpdateCandidateInput,
    ) -> Result<(), String> {
        self.record(input, false)
    }
    pub(crate) fn hold(&self, input: &UpdateCandidateInput) -> Result<(), String> {
        self.record(input, true)
    }
    fn record(&self, input: &UpdateCandidateInput, held: bool) -> Result<(), String> {
        if let Some(library) = &self.native {
            return library
                .record_discovery(
                    LibraryId::now_v7(),
                    DiscoveryInput {
                        provider: &input.source,
                        key: &input.source_id,
                        kind: &input.kind,
                        title: &input.title,
                        payload: &input.payload_json,
                        target_type: input.target_type.as_deref(),
                    },
                    held,
                )
                .map_err(|e| e.to_string());
        }
        self.state.db.upsert_update_candidate(input)?;
        if held
            && self
                .state
                .db
                .update_candidate_status(&input.source, &input.source_id)?
                .as_deref()
                != Some("dismissed")
        {
            self.state
                .db
                .set_update_candidate_status(&input.source, &input.source_id, "held")?;
        }
        Ok(())
    }
    pub(crate) fn update_candidate_hold(
        &self,
        source: &str,
        key: &str,
    ) -> Result<Option<(String, Option<String>)>, String> {
        if let Some(library) = &self.native {
            let Some(row) = library.discovery(source, key).map_err(|e| e.to_string())? else {
                return Ok(None);
            };
            let value: serde_json::Value = serde_json::from_str(
                &library
                    .discovery_payload(row.id)
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
            return Ok(Some((
                row.decision,
                value
                    .get("holdKind")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            )));
        }
        self.state.db.update_candidate_hold(source, key)
    }
    pub(crate) fn update_candidate_status(
        &self,
        source: &str,
        key: &str,
    ) -> Result<Option<String>, String> {
        if let Some(library) = &self.native {
            return library
                .discovery(source, key)
                .map(|v| v.map(|v| v.decision))
                .map_err(|e| e.to_string());
        }
        self.state.db.update_candidate_status(source, key)
    }
    pub(crate) fn set_update_candidate_status(
        &self,
        source: &str,
        key: &str,
        status: &str,
    ) -> Result<(), String> {
        if let Some(library) = &self.native {
            if status == "held" {
                let entry = library
                    .discovery(source, key)
                    .map_err(|e| e.to_string())?
                    .ok_or("候補がありません")?;
                return self.hold(&UpdateCandidateInput {
                    source: source.into(),
                    source_id: key.into(),
                    kind: entry.kind,
                    title: entry.title,
                    payload_json: library
                        .discovery_payload(entry.id)
                        .map_err(|e| e.to_string())?,
                    target_type: entry.target_type,
                });
            }
            if !matches!(status, "pending" | "dismissed") {
                return Err("候補の判断が不正です".into());
            }
            return library
                .decide_discovery(
                    LibraryId::now_v7(),
                    source,
                    key,
                    None,
                    status == "dismissed",
                )
                .map_err(|e| e.to_string());
        }
        self.state
            .db
            .set_update_candidate_status(source, key, status)
    }
    pub(crate) fn clear_update_candidate(&self, source: &str, key: &str) -> Result<(), String> {
        if let Some(library) = &self.native {
            return library
                .clear_discovery(LibraryId::now_v7(), source, key, false)
                .map_err(|e| e.to_string());
        }
        self.state.db.clear_update_candidate(source, key)
    }
    pub(crate) fn list_pending_update_candidates_after(
        &self,
        cursor: Option<(&str, &str)>,
        limit: i64,
    ) -> Result<Vec<UpdateCandidateRow>, String> {
        if self.native.is_some() {
            return self
                .rows(Some("pending"), cursor, limit.clamp(1, 1000) as usize)?
                .into_iter()
                .map(|v| self.convert(v, false))
                .collect();
        }
        self.state
            .db
            .list_pending_update_candidates_after(cursor, limit)
    }
    pub(crate) fn list_update_candidates_by_status(
        &self,
        status: &str,
        limit: i64,
    ) -> Result<Vec<UpdateCandidateRow>, String> {
        self.list(status, limit, false)
    }
    pub(crate) fn list_deferred_update_candidate_summaries(
        &self,
        limit: i64,
    ) -> Result<Vec<UpdateCandidateRow>, String> {
        self.list("deferred", limit, true)
    }
    fn list(
        &self,
        status: &str,
        limit: i64,
        display: bool,
    ) -> Result<Vec<UpdateCandidateRow>, String> {
        if let Some(library) = &self.native {
            library
                .prepare_discovery_summaries()
                .map_err(|e| e.to_string())?;
            let mut rows = self.all(Some(status))?;
            rows.sort_by_key(|row| std::cmp::Reverse((row.updated_at, row.id)));
            if limit >= 0 {
                rows.truncate(limit as usize);
            }
            return rows.into_iter().map(|v| self.convert(v, display)).collect();
        }
        if display {
            self.state
                .db
                .list_deferred_update_candidate_summaries(limit)
        } else {
            self.state
                .db
                .list_update_candidates_by_status(status, limit)
        }
    }
    pub(crate) fn held_update_candidate_keys(
        &self,
    ) -> Result<std::collections::HashSet<(String, String)>, String> {
        if self.native.is_some() {
            return Ok(self
                .all(Some("held"))?
                .into_iter()
                .map(|v| (v.provider, v.key))
                .collect());
        }
        self.state.db.held_update_candidate_keys()
    }
    pub(crate) fn count_saved_deferred_update_candidates(&self) -> Result<i64, String> {
        if self.native.is_some() {
            return self.all(Some("held"))?.into_iter().try_fold(0, |count, v| {
                Ok(count + i64::from(self.saved(&v.provider, &v.key)?))
            });
        }
        self.state.db.count_saved_deferred_update_candidates()
    }
    pub(crate) fn count_dismissed_update_candidates(&self) -> Result<i64, String> {
        if self.native.is_some() {
            return Ok(self.all(Some("dismissed"))?.len() as i64);
        }
        self.state.db.count_dismissed_update_candidates()
    }
    pub(crate) fn restore_dismissed_update_candidates(&self) -> Result<usize, String> {
        if self.native.is_some() {
            let rows = self.all(Some("dismissed"))?;
            let count = rows.len();
            for row in rows {
                self.set_update_candidate_status(&row.provider, &row.key, "pending")?;
            }
            return Ok(count);
        }
        self.state.db.restore_dismissed_update_candidates()
    }
    pub(crate) fn has_incomplete_save_candidate(
        &self,
        source: &str,
        key: &str,
    ) -> Result<bool, String> {
        if self.native.is_some() {
            return Ok(self.update_candidate_hold(source, key)?
                == Some(("held".into(), Some("incomplete".into())))
                && self.saved(source, key)?);
        }
        self.state.db.has_incomplete_save_candidate(source, key)
    }
    pub(crate) fn candidate_acquisition_blocked(
        &self,
        source: &str,
        key: &str,
        repair: bool,
    ) -> Result<bool, String> {
        if self.native.is_some() {
            return Ok(matches!(
                self.update_candidate_status(source, key)?.as_deref(),
                Some("held" | "dismissed")
            ) && !(repair && self.has_incomplete_save_candidate(source, key)?));
        }
        self.state
            .db
            .candidate_acquisition_blocked(source, key, repair)
    }
    pub(crate) fn remember_job_candidate(&self, source: &str, key: &str) -> Result<(), String> {
        if self.native.is_none() {
            return self.state.db.remember_job_candidate(source, key);
        }
        if self.update_candidate_status(source, key)?.is_some() {
            return Ok(());
        }
        let (title, payload) = self.state.db.latest_job_candidate(source, key)?;
        let mut value: serde_json::Value =
            serde_json::from_str(&payload).map_err(|e| e.to_string())?;
        let kind = if self.saved(source, key)? {
            "revision"
        } else {
            "new"
        };
        value["kind"] = serde_json::json!(kind);
        self.upsert_update_candidate(&UpdateCandidateInput {
            source: source.into(),
            source_id: key.into(),
            kind: kind.into(),
            title,
            payload_json: value.to_string(),
            target_type: Some("work".into()),
        })
    }
    pub(crate) fn clear_resolved_unavailable_candidates(
        &self,
        source: &str,
        keys: &[String],
    ) -> Result<(), String> {
        let Some(library) = &self.native else {
            return self
                .state
                .db
                .clear_resolved_unavailable_candidates(source, keys);
        };
        for key in keys {
            library
                .clear_discovery(LibraryId::now_v7(), source, key, true)
                .map_err(|e| e.to_string())?;
        }
        self.state.db.resolve_unavailable_job_items(source, keys)
    }
    pub(crate) fn insert_update_job_unavailable_item(
        &self,
        job: &str,
        item: &UpdateJobItemInput,
        original: &serde_json::Value,
        reason: &str,
    ) -> Result<bool, String> {
        if self.native.is_none() {
            return self
                .state
                .db
                .insert_update_job_unavailable_item(job, item, original, reason);
        }
        let source = item.source.as_deref().ok_or("取得元がありません")?;
        let key = item.source_id.as_deref().ok_or("作品 ID がありません")?;
        let known = self
            .update_candidate_hold(source, key)?
            .is_some_and(|(status, kind)| {
                matches!(status.as_str(), "held" | "dismissed")
                    && kind.as_deref() == Some("unavailable")
            });
        self.hold(&UpdateCandidateInput {source:source.into(),source_id:key.into(),kind:"revision".into(),title:item.title.clone(),payload_json:serde_json::json!({"source":source,"sourceId":key,"title":item.title,"kind":"revision","holdKind":"unavailable","holdReason":reason,"originalData":original}).to_string(),target_type:Some("work".into())})?;
        self.state
            .db
            .insert_unavailable_queue_item(job, item, reason, known)
    }
}
