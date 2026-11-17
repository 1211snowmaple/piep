//! 直接取得したプロフィールの保存。原応答の受信記録と確認日時を、保存版と一緒に確定する。
use super::{
    cover_identity, Capture, CaptureResult, CaptureSource, CapturedMedia, EntityKind, Error,
    Library, LibraryId, Result, StagedAsset,
};
use serde_json::Value;
use std::collections::BTreeMap;

pub const PROFILE_REFRESH_ROUTE: &str = "profile-refresh";

pub struct ProfileSave<'a> {
    pub request: LibraryId,
    pub provider: &'a str,
    pub key: &'a str,
    pub kind: EntityKind,
    /// 表示用に解釈した項目。取得元の原応答は record_acquisition_payload で先に保持する。
    pub data: &'a Value,
    pub media_complete: bool,
    pub images: Vec<StagedAsset>,
    pub checked_at: i64,
}

impl Library {
    /// 保存済み作品の現在版に載る作者・シリーズのうち、プロフィールを直接取得していない対象。
    /// 作品の原本や旧棚の番号は読まず、同じ対象を一度だけ返す。
    pub fn missing_profiles_for_works(
        &self,
        works: &[LibraryId],
    ) -> Result<Vec<(EntityKind, String, String)>> {
        let conn = self.reader()?;
        let mut statement=conn.prepare("SELECT x.kind,x.provider,x.external_key FROM entity_current c JOIN library_items li ON li.work_id=c.entity_id JOIN snapshot_people r ON r.snapshot_id=c.snapshot_id JOIN external_identities x ON x.entity_id=r.person_id LEFT JOIN source_entity_state st ON st.entity_id=r.person_id WHERE c.entity_id=?1 AND r.role IN ('author','creator') AND x.kind='person' AND x.provider IN ('pixiv','fanbox') AND st.last_fetched_at IS NULL UNION ALL SELECT x.kind,x.provider,x.external_key FROM entity_current c JOIN library_items li ON li.work_id=c.entity_id JOIN snapshot_series r ON r.snapshot_id=c.snapshot_id JOIN external_identities x ON x.entity_id=r.series_id LEFT JOIN source_entity_state st ON st.entity_id=r.series_id WHERE c.entity_id=?1 AND x.kind='series' AND x.provider='pixiv' AND st.last_fetched_at IS NULL")?;
        let mut keys = std::collections::BTreeSet::new();
        for work in works.iter().collect::<std::collections::BTreeSet<_>>() {
            for row in statement.query_map([work.as_bytes().as_slice()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })? {
                keys.insert(row?);
            }
        }
        keys.into_iter()
            .map(|(kind, provider, key)| {
                let kind = match kind.as_str() {
                    "person" => EntityKind::Person,
                    "series" => EntityKind::Series,
                    _ => return Err(Error::Integrity("プロフィールの対象が不正です".into())),
                };
                Ok((kind, provider, key))
            })
            .collect()
    }

    pub fn save_profile(&self, save: ProfileSave<'_>) -> Result<CaptureResult> {
        let request = save.request;
        let result = (|| {
            if !matches!(save.kind, EntityKind::Person | EntityKind::Series)
                || save.data.get("source").and_then(Value::as_str) != Some(save.provider)
                || save.data.get("sourceKey").and_then(Value::as_str) != Some(save.key)
            {
                return Err(Error::Integrity(
                    "プロフィールの取得元が一致しません".into(),
                ));
            }
            if self.acquisition_status(request)?.payloads.is_empty() {
                return Err(Error::Integrity("プロフィールの原応答がありません".into()));
            }
            let super::work_profiles::ProfileFields {
                metadata,
                mut states,
            } = super::work_profiles::profile_fields(save.data)?;
            if !save.media_complete {
                states.insert("media".into(), "unknown".into());
            }
            let mut images = BTreeMap::new();
            for image in save.images {
                if !matches!(image.role.as_str(), "icon" | "cover")
                    || (save.kind == EntityKind::Series && image.role == "icon")
                    || images.insert(image.role.clone(), image).is_some()
                {
                    return Err(Error::Integrity(
                        "プロフィールの画像の用途が不正・重複しています".into(),
                    ));
                }
            }
            let mut media = Vec::new();
            for (slot, field) in [("icon", "iconUrl"), ("cover", "coverUrl")] {
                if save.kind == EntityKind::Series && slot == "icon" {
                    continue;
                }
                let Some(url) = save
                    .data
                    .get(field)
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                else {
                    continue;
                };
                let image = images.remove(slot);
                if image
                    .as_ref()
                    .is_some_and(|i| i.url.as_deref() != Some(url))
                {
                    return Err(Error::Integrity(
                        "プロフィールの画像の取得元が一致しません".into(),
                    ));
                }
                let (name, mime, bytes) = image.map_or_else(
                    || (slot.into(), "application/octet-stream".into(), None),
                    |i| {
                        (
                            i.name,
                            i.mime_type
                                .unwrap_or_else(|| "application/octet-stream".into()),
                            Some(CaptureSource::File(i.path)),
                        )
                    },
                );
                media.push(CapturedMedia {
                    slot: slot.into(),
                    role: if slot == "cover" && save.kind == EntityKind::Person {
                        "banner"
                    } else {
                        slot
                    }
                    .into(),
                    source_key: Some(cover_identity(save.provider, url)),
                    source_url: Some(url.into()),
                    quality: "unknown".into(),
                    original_name: name,
                    mime_type: mime,
                    bytes,
                });
            }
            if !images.is_empty() {
                return Err(Error::Integrity(
                    "プロフィールの画像に取得元がありません".into(),
                ));
            }
            self.complete_profile_acquisition(
                request,
                Capture {
                    request_id: request,
                    acquired_at: None,
                    provider: save.provider.into(),
                    external_key: save.key.into(),
                    kind: save.kind,
                    route: PROFILE_REFRESH_ROUTE.into(),
                    raw: CaptureSource::Bytes(serde_json::to_vec_pretty(save.data)?),
                    metadata,
                    field_states: states,
                    body: None,
                    media,
                    attachments: vec![],
                },
                save.checked_at,
            )
        })();
        if result.is_err() {
            let _ = self.cancel_acquisition(request);
        }
        result
    }
}

#[cfg(test)]
#[path = "profile_capture_tests.rs"]
mod tests;
