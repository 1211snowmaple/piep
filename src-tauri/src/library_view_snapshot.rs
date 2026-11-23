//! 選んだ保存状態だけから、作品情報・本文・画像と添付を比較する入力を作る。
use crate::analysis::{revision_metadata, RevisionAsset, RevisionMetadata};
use crate::library::{
    BodyAssetReference, Error, Library, LibraryId, ResourceInfo, ResourceReference, ResourceScope,
    Result, SourceBody,
};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Serialize)]
pub struct RevisionSnapshotView {
    /// 未取得は None。確認して保存した空の本文は Some("")。
    pub text: Option<String>,
    pub metadata: RevisionMetadata,
}

fn known<'a>(
    metadata: &'a BTreeMap<String, Value>,
    states: &BTreeMap<String, String>,
    name: &str,
) -> Option<&'a Value> {
    if states.get(name).is_some_and(|state| state != "known") {
        None
    } else {
        metadata.get(name)
    }
}
fn text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    match value {
        Value::Null => Some(String::new()),
        Value::String(s) => Some(s.clone()),
        Value::Number(_) | Value::Bool(_) => Some(value.to_string()),
        _ => None,
    }
}

#[cfg(test)]
#[path = "library_view_snapshot_tests.rs"]
mod tests;
fn named(name: Option<String>, key: Option<String>) -> Option<String> {
    match (name, key) {
        (Some(name), Some(key)) if !name.is_empty() && !key.is_empty() => {
            Some(format!("{name}（{key}）"))
        }
        (Some(name), _) => Some(name),
        (None, key) => key,
    }
}
fn date(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Null => Some(String::new()),
        Value::Number(n) => {
            chrono::DateTime::from_timestamp_millis(n.as_i64()?).map(|d| d.to_rfc3339())
        }
        Value::String(s) if s.is_empty() => Some(String::new()),
        Value::String(s) => chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.to_rfc3339()),
        _ => None,
    }
}
fn reference_key(reference: &BodyAssetReference) -> String {
    match reference {
        BodyAssetReference::PixivUploaded { image_id } => format!("pixiv-uploaded:{image_id}"),
        BodyAssetReference::PixivIllustration {
            illustration_id,
            page,
        } => format!("pixiv-illustration:{illustration_id}:{page}"),
        BodyAssetReference::ArticleImage { key } => format!("article-image:{key}"),
        BodyAssetReference::ArticleFile { key } => format!("article-file:{key}"),
        BodyAssetReference::ArticleImageAt { index } => format!("article-image-at:{index}"),
        BodyAssetReference::ArticleFileAt { index } => format!("article-file-at:{index}"),
    }
}
fn asset(
    key: String,
    kind: &str,
    label: String,
    source: String,
    resource: Option<&ResourceInfo>,
    available: Option<bool>,
) -> RevisionAsset {
    let file = resource.and_then(|r| r.file.as_ref());
    RevisionAsset {
        key,
        kind: kind.into(),
        label,
        source,
        local_path: file.map(|f| format!("shelf:{}", f.id)),
        available,
        content_hash: file.map(|f| f.id.to_string()),
    }
}

impl Library {
    /// 編集と現在版へ読み替えず、指定した作品の公開済み snapshot だけを比較する。
    pub fn work_revision_snapshot(
        &self,
        work: LibraryId,
        snapshot: LibraryId,
    ) -> Result<RevisionSnapshotView> {
        let (metadata, states, provider): (String, String, String) = {
            let conn = self.reader()?;
            conn.query_row("SELECT s.metadata_json,s.field_states_json,(SELECT provider FROM external_identities WHERE entity_id=s.entity_id AND kind='work' ORDER BY provider,external_key LIMIT 1) FROM revision_snapshots s JOIN works w ON w.id=s.entity_id WHERE s.entity_id=?1 AND s.id=?2 AND s.published=1",params![work.as_bytes().as_slice(),snapshot.as_bytes().as_slice()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?.ok_or(Error::ResourceUnavailable)?
        };
        let metadata: BTreeMap<String, Value> = serde_json::from_str(&metadata)?;
        let states: BTreeMap<String, String> = serde_json::from_str(&states)?;
        let value = |key: &str| known(&metadata, &states, key);
        let body = self.source_body(work, snapshot)?;
        let scope = ResourceScope::Snapshot(snapshot);
        let resources = self.resource_manifest(work, scope)?.resources;
        let originals = resources.iter().filter(|r| {
            matches!(r.reference, ResourceReference::Original { .. }) && r.file.is_some()
        });
        let raw = if let Some(original) = originals.min_by_key(|r| r.role != "source_response") {
            let mut bytes = Vec::new();
            self.read_resource(work, scope, &original.reference, None, &mut bytes)?;
            serde_json::from_slice::<Value>(&bytes).ok()
        } else {
            None
        };
        let date_value = |key: &str, paths: &[&str]| {
            if metadata.contains_key(key) || states.contains_key(key) {
                return date(value(key));
            }
            paths.iter().find_map(|p| date(raw.as_ref()?.pointer(p)))
        };
        let chosen_body = match &body {
            Some(SourceBody::FanboxBody(b)) => json!({"body":b}),
            _ => json!({}),
        };
        let mut view = revision_metadata(&chosen_body, &provider, None);
        view.title = text(value("title"));
        view.caption = text(value("caption"));
        view.author = named(text(value("author_name")), text(value("author_key")));
        view.series = named(text(value("series_title")), text(value("series_id")));
        view.tags = match value("tags") {
            Some(Value::Null) => Some(Vec::new()),
            Some(Value::Array(tags)) => Some(
                tags.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
            ),
            _ => None,
        };
        view.published_at = date_value(
            "create_date",
            &[
                "/create_date",
                "/createDate",
                "/publishedDatetime",
                "/detail/create_date",
                "/detail/createDate",
            ],
        );
        view.updated_at = date_value(
            "update_date",
            &[
                "/uploadDate",
                "/updateDate",
                "/updatedDatetime",
                "/detail/uploadDate",
                "/detail/update_date",
            ],
        );
        let required = body
            .as_ref()
            .map(SourceBody::asset_references)
            .transpose()?
            .unwrap_or_default();
        let plan = if body.is_some() {
            match self.reader_bindings(work, snapshot) {
                Ok(plan) => Some(plan),
                Err(Error::ResourceUnavailable) => None,
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        let assets_known = body.is_some() && (required.is_empty() || plan.is_some());
        let selected = resources
            .iter()
            .map(|r| (&r.reference, r))
            .collect::<BTreeMap<_, _>>();
        let mut used = BTreeSet::new();
        let mut assets = Vec::new();
        for reference in required {
            let binding = plan
                .as_ref()
                .and_then(|p| p.bindings.iter().find(|b| b.reference == reference));
            let resource = match binding.and_then(|b| b.resource.as_ref()) {
                Some(reference) => {
                    used.insert(reference.clone());
                    Some(*selected.get(reference).ok_or(Error::ResourceUnavailable)?)
                }
                None => None,
            };
            let key = reference_key(&reference);
            let kind = if reference.is_image() {
                "illustrations"
            } else {
                "files"
            };
            let source = body
                .as_ref()
                .and_then(|b| crate::library::source_bindings::entry(b, &reference))
                .and_then(|v| {
                    v.get(if reference.is_image() {
                        "originalUrl"
                    } else {
                        "url"
                    })
                })
                .and_then(Value::as_str)
                .map(crate::downloader::asset_downloader::asset_source_identity)
                .or_else(|| resource.and_then(|r| r.source_identity.clone()))
                .unwrap_or_default();
            let label = resource
                .and_then(|r| r.original_name.clone())
                .unwrap_or_else(|| match &reference {
                    BodyAssetReference::PixivUploaded { image_id } => {
                        format!("[uploadedimage:{image_id}]")
                    }
                    BodyAssetReference::PixivIllustration {
                        illustration_id,
                        page,
                    } => format!("[pixivimage:{illustration_id}-{page}]"),
                    _ => key.clone(),
                });
            assets.push(asset(
                key,
                kind,
                label,
                source,
                resource,
                assets_known.then_some(resource.is_some_and(|r| r.file.is_some())),
            ));
        }
        for resource in &resources {
            let (key, kind) = match &resource.reference {
                ResourceReference::Media { slot } => (
                    format!("media:{slot}"),
                    if resource.role == "cover" {
                        "cover"
                    } else {
                        "illustrations"
                    },
                ),
                ResourceReference::File { slot } => (format!("file:{slot}"), "files"),
                _ => continue,
            };
            if used.contains(&resource.reference) {
                continue;
            }
            assets.push(asset(
                key,
                kind,
                resource
                    .original_name
                    .clone()
                    .unwrap_or_else(|| resource.role.clone()),
                resource.source_identity.clone().unwrap_or_default(),
                Some(resource),
                Some(resource.file.is_some()),
            ));
        }
        assets.sort_by(|a, b| a.key.cmp(&b.key));
        view.assets = assets;
        view.assets_known = Some(assets_known);
        Ok(RevisionSnapshotView {
            text: body.map(|b| b.text_content()),
            metadata: view,
        })
    }
}
