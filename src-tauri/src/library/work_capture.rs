//! 取得元の応答（作品）から保存の入力を組む。旧棚の版の移行と、アプリの保存の両方が同じ規則を使う
//! （移した作品と、切り替えた後に保存した作品が同じ形になるように）。
//!
//! 入力は取得元の応答の JSON と、手元に落とした画像・添付のファイル。応答の形は旧棚の保存と同じ
//! （pixiv は `detail` の下、FANBOX は投稿そのもの）。

use super::{
    cover_identity, image_identity, Capture, CaptureResult, CaptureSource, CapturedAttachment,
    CapturedMedia, EntityKind, Error, Library, LibraryId, ResourceReference, Result, SourceBody,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 手元に落とした画像・添付の一つ。`role` は旧棚の種類の名前（`cover`・`illustration`・`file`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedAsset {
    pub role: String,
    /// 保存したときのファイル名（挿絵は拡張子を除いた名前が本文の中の指定になる）。
    pub name: String,
    pub path: PathBuf,
    /// 取得元の URL。分からなければ None。
    pub url: Option<String>,
    pub mime_type: Option<String>,
}

/// 作品の項目・項目の状態・本文。
pub(crate) struct WorkFields {
    pub metadata: BTreeMap<String, Value>,
    pub field_states: BTreeMap<String, String>,
    pub body: Option<SourceBody>,
}

/// 画像・添付の種類と名前から、本文の中で指す枠を決める。
pub fn asset_reference(role: &str, name: &str) -> Result<ResourceReference> {
    Ok(if role == "cover" || role == "illustration" {
        let slot = if role == "cover" {
            "cover".into()
        } else {
            Path::new(name)
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| Error::Integrity("画像名が不正です".into()))?
                .to_owned()
        };
        ResourceReference::Media { slot }
    } else {
        ResourceReference::File {
            slot: format!("file:{name}"),
        }
    })
}

/// 応答から項目と本文を取り出す。
pub(crate) fn work_fields(provider: &str, data: &Value) -> WorkFields {
    let detail = data.get("detail").unwrap_or(data);
    let mut metadata = BTreeMap::new();
    match provider {
        "pixiv" => {
            metadata.insert("content_type".into(), json!("novel"));
        }
        "fanbox" => {
            if let Some(kind) = detail.get("type").and_then(Value::as_str) {
                metadata.insert("content_type".into(), json!(kind));
            }
        }
        _ => (),
    }
    for name in [
        "title",
        "caption",
        "create_date",
        "series_id",
        "series_title",
        "text_length",
    ] {
        if let Some(value) = detail.get(name) {
            metadata.insert(name.to_owned(), value.clone());
        }
    }
    if let Some(user) = detail.get("user") {
        if let Some(name) = user.get("name") {
            metadata.insert("author_name".into(), name.clone());
        }
        if let Some(id) = user.get("id") {
            metadata.insert("author_key".into(), id.clone());
        }
    }
    if provider == "fanbox" {
        if let Some(creator) = detail.get("creatorId").and_then(Value::as_str) {
            metadata.insert("author_key".into(), json!(creator));
        }
        if let Some(date) = detail.get("publishedDatetime").and_then(Value::as_str) {
            metadata.insert("create_date".into(), json!(date));
        }
        // 一覧の更新日とカードの抜粋。比較の対象ではないので、変わっても改稿にしない。
        if let Some(date) = detail.get("updatedDatetime").and_then(Value::as_str) {
            metadata.insert("update_date".into(), json!(date));
        }
        if let Some(excerpt) = detail.get("excerpt").and_then(Value::as_str) {
            metadata.insert("excerpt".into(), json!(excerpt));
        }
    }
    if let Some(tags) = detail.get("tags").and_then(Value::as_array) {
        let mut names: Vec<_> = tags
            .iter()
            .filter_map(|t| t.as_str().or_else(|| t.get("name").and_then(Value::as_str)))
            .map(str::to_owned)
            .collect();
        names.sort();
        names.dedup();
        metadata.insert("tags".into(), json!(names));
    }
    let body = if provider == "fanbox" {
        data.get("body")
            .filter(|body| !body.is_null())
            .cloned()
            .map(SourceBody::FanboxBody)
    } else {
        data.get("text")
            .or_else(|| detail.get("text"))
            .and_then(Value::as_str)
            .map(|text| {
                if provider == "pixiv" {
                    SourceBody::PixivText(text.into())
                } else {
                    SourceBody::PlainText(text.into())
                }
            })
    };
    let mut field_states = BTreeMap::new();
    // 版の画像と添付は、この取得で手元にあるものがすべて。前の版にあって消えた画像を引き継がない。
    field_states.insert("attachments".into(), "known".into());
    field_states.insert("media".into(), "known".into());
    if metadata
        .get("caption")
        .is_some_and(|v| v.as_str() == Some(""))
        && metadata
            .get("tags")
            .is_some_and(|v| v.as_array().is_some_and(Vec::is_empty))
    {
        field_states.insert("caption".into(), "unknown".into());
        field_states.insert("tags".into(), "unknown".into());
    }
    WorkFields {
        metadata,
        field_states,
        body,
    }
}

/// 画像を pixiv の品質の印から分ける（縮小版・原寸・不明）。
fn quality(url: Option<&str>) -> &'static str {
    if url.is_some_and(|u| u.contains("/c/")) {
        "thumbnail"
    } else if url.is_some_and(|u| u.contains("-original/")) {
        "original"
    } else {
        "unknown"
    }
}

/// 手元の画像・添付を、保存の画像と添付へ。返す枠の名前は、渡した順に並ぶ。
pub(crate) fn staged_assets(
    provider: &str,
    data: &Value,
    assets: Vec<StagedAsset>,
) -> Result<(Vec<CapturedMedia>, Vec<CapturedAttachment>, Vec<String>)> {
    let detail = data.get("detail").unwrap_or(data);
    let mut media = Vec::new();
    let mut attachments = Vec::new();
    let mut slots = Vec::new();
    for asset in assets {
        let StagedAsset {
            role,
            name,
            path,
            url,
            mime_type,
        } = asset;
        match asset_reference(&role, &name)? {
            ResourceReference::Media { slot } => {
                let media_key = if role == "cover" {
                    url.as_deref()
                        .or_else(|| data.get("cover_url").and_then(Value::as_str))
                        .or_else(|| detail.get("cover_url").and_then(Value::as_str))
                        .or_else(|| data.get("coverImageUrl").and_then(Value::as_str))
                        .map(|url| cover_identity(provider, url))
                } else {
                    url.as_deref().map(|url| image_identity(provider, url))
                };
                slots.push(slot.clone());
                media.push(CapturedMedia {
                    slot,
                    role: if role == "cover" {
                        "cover"
                    } else {
                        "illustration"
                    }
                    .into(),
                    source_key: media_key,
                    quality: quality(url.as_deref()).into(),
                    source_url: url,
                    original_name: name,
                    mime_type: mime_type.unwrap_or_else(|| "application/octet-stream".into()),
                    bytes: Some(CaptureSource::File(path)),
                });
            }
            ResourceReference::File { slot } => {
                slots.push(slot.clone());
                attachments.push(CapturedAttachment {
                    slot,
                    original_name: name,
                    mime_type,
                    bytes: CaptureSource::File(path),
                });
            }
            ResourceReference::Body | ResourceReference::Original { .. } => {
                unreachable!("asset_reference は画像か添付の枠だけを返す")
            }
        }
    }
    Ok((media, attachments, slots))
}

/// アプリの保存で、作品を一つ棚へ保存する。
pub struct WorkSave<'a> {
    /// 送り直しで同じになる要求の ID。
    pub request: LibraryId,
    pub provider: &'a str,
    pub key: &'a str,
    /// 何から保存したか（`save`・`update` など）。
    pub route: &'a str,
    /// 取得元の応答（旧棚の保存と同じ形）。原本としてそのまま残す。
    pub data: &'a Value,
    pub assets: Vec<StagedAsset>,
}

impl Library {
    /// 取得を始めた記録を作ってから、応答と手元の画像・添付を保存する。中身が今の版と同じなら
    /// 版を増やさない（`created_revision` が偽）。
    pub fn save_work(&self, save: WorkSave<'_>) -> Result<CaptureResult> {
        self.save_work_inner(save, None)
    }
    pub(crate) fn save_work_with_asset_repair(
        &self,
        save: WorkSave<'_>,
        observation: super::asset_repairs::AssetRepairObservation,
    ) -> Result<CaptureResult> {
        self.save_work_inner(save, Some(observation))
    }
    fn save_work_inner(
        &self,
        save: WorkSave<'_>,
        observation: Option<super::asset_repairs::AssetRepairObservation>,
    ) -> Result<CaptureResult> {
        let WorkSave {
            request,
            provider,
            key,
            route,
            data,
            assets,
        } = save;
        self.begin_acquisition(request, provider, EntityKind::Work, key, route)?;
        let raw = serde_json::to_vec_pretty(data)?;
        let fields = work_fields(provider, data);
        let (media, attachments, _) = staged_assets(provider, data, assets)?;
        let capture = Capture {
            request_id: request,
            acquired_at: None,
            provider: provider.into(),
            external_key: key.into(),
            kind: EntityKind::Work,
            route: route.into(),
            raw: CaptureSource::Bytes(raw),
            metadata: fields.metadata,
            field_states: fields.field_states,
            body: fields.body,
            media,
            attachments,
        };
        let result = match observation {
            Some(observation) => {
                self.complete_acquisition_with_asset_repair(request, capture, observation)
            }
            None => self.complete_acquisition(request, capture),
        };
        if result.is_err() {
            let _ = self.cancel_acquisition(request);
        }
        result
    }
}

#[cfg(test)]
#[path = "work_capture_tests.rs"]
mod tests;
