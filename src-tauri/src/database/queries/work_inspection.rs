//! On-demand inspection of one work; list screens never read its JSON or image bytes.
use super::{active_edit_revision_locked, assets_for_version_path, reader_version_path, Database};
use crate::database::AssetEntry;
use crate::downloader::asset_downloader::{asset_source_identity, extract_download_targets};
use regex::Regex;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Read, path::Path, sync::LazyLock};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionAsset {
    key: String,
    kind: String,
    label: String,
    source: String,
    local_path: Option<String>,
    available: Option<bool>,
    content_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionMetadata {
    title: Option<String>,
    caption: Option<String>,
    author: Option<String>,
    series: Option<String>,
    body_structure: Option<String>,
    tags: Option<Vec<String>>,
    published_at: Option<String>,
    updated_at: Option<String>,
    assets: Vec<RevisionAsset>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionSnapshot {
    pub text: String,
    pub metadata: RevisionMetadata,
    /// 編集画面で足した絵。どの保存版にも属さないので、編集版にしか無い。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub added_images: Vec<AssetEntry>,
}

fn text_at(value: &Value, paths: &[&str]) -> Option<String> {
    paths.iter().find_map(|path| {
        value
            .pointer(path)
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

fn file_digest(path: &str) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = file.read(&mut buffer).ok()?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Some(
        digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

fn asset_key(folder: &str, filename: &str) -> String {
    static LEGACY_PAGE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(\d+)-\d+(_p\d+)$").unwrap());
    if folder == "cover" {
        return "cover".to_string();
    }
    let stem = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);
    format!("{folder}:{}", LEGACY_PAGE.replace(stem, "$1$2"))
}

pub(crate) fn revision_metadata(
    value: &Value,
    source: &str,
    assets: Option<&[AssetEntry]>,
) -> RevisionMetadata {
    let value = if source == "fanbox" {
        crate::fanbox_api::payload::post_or_self(value)
    } else {
        value
    };
    let mut target_value = value.clone();
    if source == "pixiv" && target_value.get("cover_url").is_none() {
        if let Some(cover) = text_at(value, &["/coverUrl", "/detail/coverUrl"]) {
            target_value["cover_url"] = Value::String(cover);
        }
    }
    let mut targets = Vec::new();
    extract_download_targets(&target_value, source == "fanbox", &mut targets);
    let mut items = BTreeMap::new();
    for target in targets {
        let key = asset_key(target.sub_folder, &target.filename);
        let saved = assets.and_then(|list| {
            list.iter().find(|asset| {
                let folder = if asset.asset_type == "cover" {
                    "cover"
                } else if asset.asset_type == "file" {
                    "files"
                } else {
                    target.sub_folder
                };
                asset_key(folder, &asset.filename) == key
            })
        });
        let available = saved.is_some_and(|asset| {
            std::fs::metadata(&asset.local_path).is_ok_and(|m| m.is_file() && m.len() > 0)
        });
        items.insert(
            key.clone(),
            RevisionAsset {
                key,
                kind: target.sub_folder.to_string(),
                label: target.filename,
                source: asset_source_identity(&target.url),
                local_path: saved
                    .filter(|_| available)
                    .map(|asset| asset.local_path.clone()),
                available: assets.map(|_| available),
                content_hash: saved
                    .filter(|_| available)
                    .and_then(|asset| file_digest(&asset.local_path)),
            },
        );
    }
    if source == "pixiv" {
        static REFERENCE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"\[(uploadedimage|pixivimage):([^\]]+)\]").unwrap());
        static SPEC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+)(?:-(\d+))?$").unwrap());
        let body = crate::commands::downloader::fetched_plain_text(value, source);
        for capture in REFERENCE.captures_iter(&body) {
            let reference = capture.get(0).unwrap().as_str();
            let key = if &capture[1] == "uploadedimage" {
                format!("illustrations:inline_{}", &capture[2])
            } else if let Some(spec) = SPEC.captures(&capture[2]) {
                let page = spec
                    .get(2)
                    .map_or(Some(1), |v| v.as_str().parse::<u64>().ok());
                match page.and_then(|page| page.checked_sub(1)) {
                    Some(page) => format!("illustrations:{}_p{page}", &spec[1]),
                    None => format!("reference:{reference}"),
                }
            } else {
                format!("reference:{reference}")
            };
            items.entry(key.clone()).or_insert_with(|| RevisionAsset {
                key,
                kind: "illustrations".to_string(),
                label: reference.to_string(),
                source: String::new(),
                local_path: None,
                available: assets.map(|_| false),
                content_hash: None,
            });
        }
    }
    let tags = value
        .get("tags")
        .or_else(|| value.pointer("/detail/tags"))
        .and_then(|tags| {
            tags.as_array()
                .or_else(|| tags.get("tags").and_then(Value::as_array))
        })
        .map(|tags| {
            tags.iter()
                .filter_map(|tag| {
                    tag.as_str()
                        .or_else(|| tag.get("name").and_then(Value::as_str))
                        .map(str::to_string)
                })
                .collect()
        });
    let body_structure = value
        .pointer("/body/blocks")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .enumerate()
                .map(|(index, block)| {
                    let kind = block.get("type").and_then(Value::as_str).unwrap_or("");
                    let mut line = format!(
                        "{}: {}",
                        index + 1,
                        match kind {
                            "p" => "段落",
                            "header" => "見出し",
                            "image" => "画像",
                            "file" => "添付ファイル",
                            "url_embed" => "リンク",
                            _ => kind,
                        }
                    );
                    for key in ["imageId", "fileId"] {
                        if let Some(id) = block.get(key).and_then(Value::as_str) {
                            line.push_str(&format!(" {id}"));
                        }
                    }
                    if let Some(id) = block.get("urlEmbedId").and_then(Value::as_str) {
                        if let Some(embed) = value
                            .pointer("/body/urlEmbedMap")
                            .and_then(|map| map.get(id))
                        {
                            line.push_str(&format!(
                                " {}",
                                text_at(embed, &["/url", "/html"]).unwrap_or_default()
                            ));
                        }
                    }
                    if let Some(links) = block.get("links").and_then(Value::as_array) {
                        for link in links {
                            line.push_str(&format!(
                                " · リンク {}（位置 {}、長さ {}）",
                                link.get("url").and_then(Value::as_str).unwrap_or(""),
                                link.get("offset").unwrap_or(&Value::Null),
                                link.get("length").unwrap_or(&Value::Null)
                            ));
                        }
                    }
                    if let Some(styles) = block.get("styles").and_then(Value::as_array) {
                        for style in styles {
                            line.push_str(&format!(
                                " · {}（位置 {}、長さ {}）",
                                if style.get("type").and_then(Value::as_str) == Some("bold") {
                                    "太字"
                                } else {
                                    "書式"
                                },
                                style.get("offset").unwrap_or(&Value::Null),
                                style.get("length").unwrap_or(&Value::Null)
                            ));
                        }
                    }
                    line
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .or_else(|| {
            let body = value.get("body")?;
            let mut order = Vec::new();
            for (field, label) in [("images", "画像"), ("files", "添付ファイル")] {
                if let Some(items) = body.get(field).and_then(Value::as_array) {
                    for (index, item) in items.iter().enumerate() {
                        order.push(format!(
                            "{}: {} {}",
                            index + 1,
                            label,
                            item.get("name")
                                .or_else(|| item.get("id"))
                                .and_then(Value::as_str)
                                .unwrap_or("")
                        ));
                    }
                }
            }
            (!order.is_empty()).then(|| order.join("\n"))
        });
    RevisionMetadata {
        author: text_at(
            value,
            &[
                "/user/name",
                "/authorName",
                "/detail/user/name",
                "/detail/authorName",
            ],
        ),
        series: text_at(
            value,
            &["/seriesTitle", "/detail/seriesTitle", "/series/title"],
        )
        .or_else(|| text_at(value, &["/seriesId", "/detail/seriesId"])),
        body_structure,
        title: text_at(value, &["/title", "/detail/title"]),
        caption: text_at(value, &["/caption", "/description", "/detail/caption"]),
        tags,
        published_at: text_at(
            value,
            &[
                "/create_date",
                "/createDate",
                "/publishedDatetime",
                "/cdate",
                "/detail/createDate",
                "/detail/cdate",
            ],
        ),
        updated_at: text_at(
            value,
            &[
                "/uploadDate",
                "/updateDate",
                "/updatedDatetime",
                "/detail/uploadDate",
            ],
        ),
        assets: items.into_values().collect(),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MissingImageLocation {
    reference: String,
    page: usize,
    occurrence: usize,
    excerpt: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkAcquisitionDetails {
    kind: String,
    status: String,
    reason: String,
    recorded_at: String,
    pub version: i64,
    missing_images: Vec<MissingImageLocation>,
    /// Pixiv detail fields saved while the source masked them.
    missing_metadata: Vec<&'static str>,
    location_error: Option<String>,
}

impl Database {
    /// 反映中の編集を、保存版と同じ形で読む。履歴で、もとにした版と比べるため。
    ///
    /// 編集は取得元の版を増やさない。版の上に重ねた層である。それを履歴から
    /// 外したままにすると、編集したのに版が一つしか無いように見える。
    pub fn get_active_edit_snapshot(
        &self,
        download_id: i64,
    ) -> Result<Option<RevisionSnapshot>, String> {
        let conn = self.read_conn()?;
        let revision = active_edit_revision_locked(&conn, download_id)?;
        drop(conn);
        let Some(revision) = revision else {
            return Ok(None);
        };
        let Some(edit) = self.active_edit_source_form(download_id)? else {
            return Ok(None);
        };
        let download = self.get_download(download_id)?;
        let versions = self.get_versions(download_id)?;
        let mut snapshot = self.get_revision_snapshot(download_id, revision.base_version)?;
        let raw =
            self.read_download_json_for_version(&download, &versions, revision.base_version)?;
        let mut value: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        // 保存版と同じ道筋で平文にする。差分に出るのは利用者が変えたところだけで、
        // 組み方の違いは出ない。
        if download.source == "fanbox" {
            let pointer = if value.get("body").is_some() {
                "/body"
            } else {
                "/post/body"
            };
            if let Some(body) = value.pointer_mut(pointer).and_then(Value::as_object_mut) {
                body.insert("blocks".into(), Value::Array(edit.fanbox_blocks));
                body.insert("text".into(), Value::String(edit.plain_text.clone()));
            }
        } else {
            value["text"] = Value::String(edit.pixiv_text.clone());
            if let Some(detail) = value.get_mut("detail").filter(|detail| detail.is_object()) {
                detail["text"] = Value::String(edit.pixiv_text.clone());
            }
        }
        snapshot.text = crate::commands::downloader::fetched_plain_text(&value, &download.source);
        if let Some(title) = revision.title.filter(|title| !title.trim().is_empty()) {
            snapshot.metadata.title = Some(title);
        }
        // 編集で消えた参照は、もう読む本文に無い。欠けた挿絵として並べない。
        let body = if download.source == "fanbox" {
            edit.plain_text
        } else {
            edit.pixiv_text
        };
        snapshot
            .metadata
            .assets
            .retain(|item| !item.label.starts_with('[') || body.contains(item.label.as_str()));
        for image in &edit.added_images {
            let available = std::fs::metadata(&image.local_path)
                .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0);
            snapshot.metadata.assets.push(RevisionAsset {
                key: format!("edit:{}", image.filename),
                kind: "illustrations".to_string(),
                label: image.filename.clone(),
                source: "編集で追加".to_string(),
                content_hash: available.then(|| file_digest(&image.local_path)).flatten(),
                local_path: available.then(|| image.local_path.clone()),
                available: Some(available),
            });
        }
        snapshot.added_images = edit.added_images;
        Ok(Some(snapshot))
    }

    pub fn get_revision_snapshot(
        &self,
        download_id: i64,
        version: i64,
    ) -> Result<RevisionSnapshot, String> {
        let download = self.get_download(download_id)?;
        let versions = self.get_versions(download_id)?;
        if version != download.current_version && !versions.iter().any(|v| v.version == version) {
            return Err("指定された版が見つかりません".to_string());
        }
        let raw = self.read_download_json_for_version(&download, &versions, version)?;
        let value: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        let assets = assets_for_version_path(
            &self.get_assets(download_id)?,
            &reader_version_path(&download, &versions, version),
        );
        let mut metadata = revision_metadata(&value, &download.source, Some(&assets));
        // Cover repair preserves provider JSON. A current cover can therefore
        // differ from the old R-18 placeholder still recorded in that JSON.
        // Historical JSON may refer to a retained file without an asset row.
        if let Some(cover) = metadata.assets.iter_mut().find(|item| item.kind == "cover") {
            let source_path = reader_version_path(&download, &versions, version);
            let retained = source_path
                .parent()
                .map(|dir| dir.join("data_assets").join("cover").join(&cover.label));
            let current = (version == download.current_version)
                .then_some(download.cover_path.as_deref())
                .flatten()
                .map(std::path::PathBuf::from);
            if let Some(path) = current
                .or(retained)
                .filter(|path| std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() > 0))
            {
                let path = path.to_string_lossy().to_string();
                if let Some(asset) = assets.iter().find(|asset| {
                    super::comparable_library_path(Path::new(&asset.local_path))
                        == super::comparable_library_path(Path::new(&path))
                }) {
                    if let Some(url) = &asset.original_url {
                        cover.source = asset_source_identity(url);
                    }
                }
                cover.label = Path::new(&path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                cover.content_hash = file_digest(&path);
                cover.local_path = Some(path);
                cover.available = Some(true);
            }
        }
        Ok(RevisionSnapshot {
            text: crate::commands::downloader::fetched_plain_text(&value, &download.source),
            metadata,
            added_images: Vec::new(),
        })
    }

    pub fn get_work_acquisition_details(
        &self,
        download_id: i64,
    ) -> Result<Option<WorkAcquisitionDetails>, String> {
        let download = self.get_download(download_id)?;
        let candidate = self.read_conn()?.query_row(
            "SELECT status, payload_json, updated_at FROM update_candidates WHERE source=?1 AND source_id=?2 AND status IN ('held','dismissed')",
            params![download.source, download.source_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)),
        ).optional().map_err(|e| e.to_string())?;
        let Some((status, payload, recorded_at)) = candidate else {
            return Ok(None);
        };
        let payload: Value = serde_json::from_str(&payload).unwrap_or(Value::Null);
        let content = self.get_cached_reader_content(download_id, Some(download.current_version));
        let location_error = content.as_ref().err().cloned();
        static MARKER: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(
                r#"<[^>]*data-image-reference="([^"]*)"[^>]*data-image-missing="true"[^>]*>"#,
            )
            .unwrap()
        });
        let mut missing_images = Vec::new();
        for (page, html) in content
            .as_ref()
            .ok()
            .into_iter()
            .flat_map(|content| content.pages.iter())
            .enumerate()
        {
            let mut occurrences = BTreeMap::new();
            for capture in MARKER.captures_iter(html) {
                let reference = super::plain_text_from_reader_html(&capture[1]);
                let occurrence = occurrences.entry(reference.clone()).or_insert(0);
                let start = capture.get(0).unwrap().start();
                // Bound context work per occurrence. Re-stripping a whole long
                // page for every missing image becomes quadratic.
                let before_html = html[..start]
                    .chars()
                    .rev()
                    .take(512)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<String>();
                let after_html = html[start..].chars().take(1024).collect::<String>();
                let before = super::plain_text_from_reader_html(&before_html);
                let after = super::plain_text_from_reader_html(&after_html);
                let prefix: String = before
                    .chars()
                    .rev()
                    .take(36)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                let suffix: String = after.chars().take(80).collect();
                missing_images.push(MissingImageLocation {
                    reference,
                    page: page + 1,
                    occurrence: *occurrence,
                    excerpt: format!("{prefix}{suffix}"),
                });
                *occurrence += 1;
            }
        }
        // Read the saved copy rather than the hold reason: a later save may
        // already have filled what the recorded reason still mentions.
        let missing_metadata = if download.source == "pixiv" {
            std::fs::read_to_string(&download.json_path)
                .ok()
                .and_then(|body| serde_json::from_str::<Value>(&body).ok())
                .map(|value| super::masked_pixiv_metadata(&value))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        Ok(Some(WorkAcquisitionDetails {
            kind: payload
                .get("holdKind")
                .and_then(Value::as_str)
                .unwrap_or("held")
                .to_string(),
            status,
            reason: payload
                .get("holdReason")
                .and_then(Value::as_str)
                .unwrap_or("確認を保留しています")
                .to_string(),
            recorded_at,
            version: download.current_version,
            missing_images,
            missing_metadata,
            location_error,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn comparison_keeps_cover_metadata_and_missing_references_without_fetching_images() {
        let value = json!({"detail": {"title":"題名", "caption":"説明", "coverUrl":"https://i.pximg.net/cover.jpg?token=old&width=100", "tags":["一"], "cdate":"2026-10-01"}, "text":"[pixivimage:42-2][pixivimage:99-0][pixivimage:id=1]"});
        let metadata = revision_metadata(&value, "pixiv", Some(&[]));
        assert_eq!(metadata.title.as_deref(), Some("題名"));
        assert_eq!(metadata.caption.as_deref(), Some("説明"));
        assert_eq!(metadata.published_at.as_deref(), Some("2026-10-01"));
        assert!(metadata
            .assets
            .iter()
            .any(|item| item.key == "illustrations:42_p1" && item.available == Some(false)));
        assert!(metadata
            .assets
            .iter()
            .any(|item| item.key == "reference:[pixivimage:99-0]"));
        assert!(metadata
            .assets
            .iter()
            .any(|item| item.key == "reference:[pixivimage:id=1]"));
        let cover = metadata
            .assets
            .iter()
            .find(|item| item.kind == "cover")
            .unwrap();
        assert!(!cover.source.contains("token="));
        assert!(cover.source.contains("width=100"));
        assert!(revision_metadata(&json!({"text":"本文"}), "pixiv", None)
            .title
            .is_none());
    }

    #[test]
    fn fanbox_link_and_style_changes_survive_plain_text_comparison() {
        let value = json!({"body":{"id":"42", "title":"題名", "body":{"blocks":[{"type":"p", "text":"同じ本文", "links":[{"url":"https://example.com/new", "offset":0,"length":2}],"styles":[{"type":"bold","offset":0,"length":2}]}]}}});
        let metadata = revision_metadata(&value, "fanbox", None);
        let structure = metadata.body_structure.unwrap();
        assert!(structure.contains("https://example.com/new"));
        assert!(structure.contains("太字"));
    }

    #[test]
    fn comparison_uses_asset_bytes_and_keeps_pending_availability_unknown() {
        let file = std::env::temp_dir().join(format!(
            "piep-revision-hash-{:016x}.jpg",
            rand::random::<u64>()
        ));
        std::fs::write(&file, b"saved bytes").unwrap();
        let asset: AssetEntry = serde_json::from_value(json!({"id":1,"downloadId":1,"assetType":"cover","filename":"front.jpg","localPath":file.to_string_lossy(),"originalUrl":null,"mimeType":"image/jpeg","fileSizeBytes":11})).unwrap();
        let value = json!({"cover_url":"https://i.pximg.net/front.jpg"});
        let saved = revision_metadata(&value, "pixiv", Some(&[asset]));
        assert!(saved.assets[0].content_hash.is_some());
        assert_eq!(saved.assets[0].available, Some(true));
        assert_eq!(
            revision_metadata(&value, "pixiv", None).assets[0].available,
            None
        );
        std::fs::remove_file(file).unwrap();
    }
}
