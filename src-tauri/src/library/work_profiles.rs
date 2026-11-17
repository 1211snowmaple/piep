//! 作品の応答から得た作者・シリーズの控え。直接取得したプロフィールと混同しない。
use super::{
    cover_identity, AcquisitionTicket, Capture, CaptureResult, CaptureSource, CapturedMedia,
    EntityKind, Error, Library, LibraryId, Result, StagedAsset,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(super) struct ProfileFields {
    pub metadata: BTreeMap<String, Value>,
    pub states: BTreeMap<String, String>,
}

/// 旧プロフィールと新しい控えで、既知・未確認の解釈を揃える。
pub(super) fn profile_fields(data: &Value) -> Result<ProfileFields> {
    let mut metadata = BTreeMap::new();
    let mut states = BTreeMap::from([
        ("media".into(), "known".into()),
        ("attachments".into(), "known".into()),
    ]);
    for (field, name) in [
        ("displayName", "display_name"),
        ("title", "title"),
        ("description", "description"),
        ("links", "links"),
        ("profileItems", "profile_items"),
        ("isConcluded", "is_concluded"),
        ("publishedContentCount", "published_content_count"),
        ("sampleNovelCount", "sample_novel_count"),
    ] {
        match data.get(field).filter(|v| !v.is_null()) {
            Some(value) => {
                let valid = match name {
                    "display_name" | "title" | "description" => value.is_string(),
                    "links" | "profile_items" => value.is_array(),
                    "is_concluded" => value.is_boolean(),
                    _ => value.as_u64().is_some(),
                };
                if !valid {
                    return Err(Error::Integrity(format!(
                        "プロフィールの {field} の型が不正です"
                    )));
                }
                metadata.insert(name.into(), value.clone());
            }
            None => {
                states.insert(name.into(), "unknown".into());
            }
        }
    }
    Ok(ProfileFields { metadata, states })
}

#[derive(Clone, Debug)]
pub(crate) struct WorkProfileSeed {
    pub provider: String,
    pub key: String,
    pub kind: EntityKind,
    pub data: Value,
}

fn string_at<'a>(data: &'a Value, paths: &[&str]) -> Option<&'a str> {
    paths.iter().find_map(|p| {
        data.pointer(p)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
    })
}
fn id_at(data: &Value, paths: &[&str]) -> Option<String> {
    paths.iter().find_map(|p| {
        let v = data.pointer(p)?;
        v.as_str()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| v.as_u64().map(|n| n.to_string()))
    })
}

/// 名前を知らない対象は作らない。作品の表紙をシリーズの表紙に流用しない。
pub(crate) fn work_profile_seeds(provider: &str, data: &Value) -> Result<Vec<WorkProfileSeed>> {
    if !matches!(provider, "pixiv" | "fanbox") {
        return Err(Error::Unsupported("作者の取得元を解釈できません".into()));
    }
    let mut seeds = Vec::new();
    let author = if provider == "fanbox" {
        id_at(data, &["/creatorId", "/detail/creatorId"])
    } else {
        id_at(data, &["/detail/user/id", "/user/id"])
    };
    if let (Some(key), Some(name)) = (
        author,
        string_at(data, &["/detail/user/name", "/user/name"]),
    ) {
        let icon = string_at(
            data,
            &[
                "/detail/user/profile_image_url",
                "/detail/user/profile_image_urls/medium",
                "/user/profile_image_urls/medium",
                "/user/iconUrl",
                "/user/icon_url",
            ],
        );
        let description = string_at(data, &["/detail/user/comment", "/user/comment"]);
        let link = if provider == "pixiv" {
            format!("https://www.pixiv.net/users/{key}")
        } else {
            format!("https://www.fanbox.cc/@{key}")
        };
        let normalized = json!({"source":provider,"sourceKey":key,"displayName":name,"iconUrl":icon,"coverUrl":null,"description":description,"links":[link]});
        seeds.push(WorkProfileSeed {
            provider: provider.into(),
            key,
            kind: EntityKind::Person,
            data: normalized,
        });
    }
    if provider == "pixiv" {
        if let (Some(key), Some(title)) = (
            id_at(
                data,
                &[
                    "/seriesId",
                    "/detail/seriesId",
                    "/series_id",
                    "/detail/series_id",
                ],
            ),
            string_at(
                data,
                &[
                    "/seriesTitle",
                    "/detail/seriesTitle",
                    "/series_title",
                    "/detail/series_title",
                    "/seriesNavigation/seriesTitle",
                    "/detail/seriesNavigation/seriesTitle",
                ],
            ),
        ) {
            let normalized = json!({"source":provider,"sourceKey":key,"title":title,"description":null,"coverUrl":null});
            seeds.push(WorkProfileSeed {
                provider: provider.into(),
                key,
                kind: EntityKind::Series,
                data: normalized,
            });
        }
    }
    Ok(seeds)
}

const ROUTE: &str = "work-profile-snapshot";
impl Library {
    /// 既存の版には触れない。画像の通信前に始め、後から取得された詳細にも勝たせない。
    pub(crate) fn begin_work_profile_seed(
        &self,
        request: LibraryId,
        seed: &WorkProfileSeed,
        work: &Value,
    ) -> Result<Option<AcquisitionTicket>> {
        let info = match self.begin_initial_profile_acquisition(
            request,
            &seed.provider,
            seed.kind,
            &seed.key,
            ROUTE,
        ) {
            Ok(info) => info,
            Err(Error::Superseded) => return Ok(None),
            Err(error) => return Err(error),
        };
        if let Err(error) = self.record_acquisition_payload(
            request,
            "source-work",
            &CaptureSource::Bytes(serde_json::to_vec_pretty(work)?),
        ) {
            let _ = self.cancel_acquisition(request);
            return Err(error);
        }
        Ok(Some(info.ticket))
    }

    pub(crate) fn complete_work_profile_seed(
        &self,
        ticket: &AcquisitionTicket,
        seed: &WorkProfileSeed,
        images: Vec<StagedAsset>,
    ) -> Result<CaptureResult> {
        let result = (|| {
            if ticket.kind != seed.kind
                || ticket.provider != seed.provider
                || ticket.external_key != seed.key
                || ticket.route != ROUTE
                || ticket.base_snapshot.is_some()
                || seed.data.get("source").and_then(Value::as_str) != Some(&seed.provider)
                || seed.data.get("sourceKey").and_then(Value::as_str) != Some(&seed.key)
            {
                return Err(Error::Integrity(
                    "プロフィールの控えと取得開始が一致しません".into(),
                ));
            }
            let ProfileFields {
                metadata,
                mut states,
            } = profile_fields(&seed.data)?;
            // 作品に載らなかった画像の有無は分からない。
            states.insert("media".into(), "unknown".into());
            let mut by_role = BTreeMap::new();
            for image in images {
                if by_role.insert(image.role.clone(), image).is_some() {
                    return Err(Error::Integrity(
                        "プロフィールの画像の用途が重複しています".into(),
                    ));
                }
            }
            let mut images = by_role;
            if images.keys().any(|role| role != "icon")
                || (seed.kind != EntityKind::Person && !images.is_empty())
            {
                return Err(Error::Integrity(
                    "プロフィールの控えの画像の用途が不正です".into(),
                ));
            }
            let mut media = Vec::new();
            if let Some(url) = seed
                .data
                .get("iconUrl")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                let image = images.remove("icon");
                if image
                    .as_ref()
                    .is_some_and(|a| a.url.as_deref() != Some(url))
                {
                    return Err(Error::Integrity(
                        "プロフィールの画像の取得元が一致しません".into(),
                    ));
                }
                let (name, mime, bytes) = image.map_or_else(
                    || ("icon".into(), "application/octet-stream".into(), None),
                    |a| {
                        (
                            a.name,
                            a.mime_type
                                .unwrap_or_else(|| "application/octet-stream".into()),
                            Some(CaptureSource::File(a.path)),
                        )
                    },
                );
                media.push(CapturedMedia {
                    slot: "icon".into(),
                    role: "icon".into(),
                    source_key: Some(cover_identity(&seed.provider, url)),
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
            self.complete_acquisition(
                ticket.id,
                Capture {
                    request_id: ticket.id,
                    acquired_at: None,
                    provider: seed.provider.clone(),
                    external_key: seed.key.clone(),
                    kind: seed.kind,
                    route: ROUTE.into(),
                    raw: CaptureSource::Bytes(serde_json::to_vec_pretty(&seed.data)?),
                    metadata,
                    field_states: states,
                    body: None,
                    media,
                    attachments: vec![],
                },
            )
        })();
        if result.is_err() {
            let _ = self.cancel_acquisition(ticket.id);
        }
        result
    }
}

#[cfg(test)]
#[path = "work_profiles_tests.rs"]
mod tests;
