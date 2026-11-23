//! On-demand inspection of one work; list screens never read its JSON or image bytes.
use crate::downloader::asset_downloader::{asset_source_identity, extract_download_targets};
use crate::models::AssetEntry;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Read, path::Path, sync::LazyLock};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionAsset {
    pub(crate) key: String,
    pub(crate) kind: String,
    pub(crate) label: String,
    pub(crate) source: String,
    pub(crate) local_path: Option<String>,
    pub(crate) available: Option<bool>,
    pub(crate) content_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionMetadata {
    pub(crate) title: Option<String>,
    pub(crate) caption: Option<String>,
    pub(crate) author: Option<String>,
    pub(crate) series: Option<String>,
    pub(crate) body_structure: Option<String>,
    pub(crate) tags: Option<Vec<String>>,
    pub(crate) published_at: Option<String>,
    pub(crate) updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) assets_known: Option<bool>,
    pub(crate) assets: Vec<RevisionAsset>,
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
        assets_known: None,
        assets: items.into_values().collect(),
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
        let asset: AssetEntry = serde_json::from_value(json!({"id":1,"workId":1,"assetType":"cover","filename":"front.jpg","localPath":file.to_string_lossy(),"originalUrl":null,"mimeType":"image/jpeg","fileSizeBytes":11})).unwrap();
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
