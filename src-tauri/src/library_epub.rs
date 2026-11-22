//! 棚の作品を、EPUB の変換が読む形（取得元の応答と、旧棚と同じ並びの画像・添付）へ書き出す。
//!
//! EPUB の変換は、取得元の応答と `data_assets/`（`cover/`・`illustrations/`・`files/`）の実ファイルから
//! 本を組む。棚は応答を原応答として、画像・添付を元の名前とともに持っているので、一冊ぶんを一時の
//! 置き場へ書き出して同じ変換に渡す。変換の規則を二つ持たない。
//!
//! 反映した編集があれば、編集の基準版の応答と画像を使い、本文を編集で差し替える（取得元が
//! 書いていたのと同じ形で渡す）。編集で足した画像は、本へ加える画像として返す。

use crate::database::models::{AssetEntry, EditedSourceForm, WorkBlock};
use crate::database::queries::{
    blocks_to_fanbox_blocks, blocks_to_pixiv_text, blocks_to_plain_text, EDITOR_IMAGE_ASSET_TYPE,
};
use crate::library::{
    Error, Fingerprint, Library, LibraryId, ResourceReference, ResourceScope, Result,
};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 一冊ぶんの書き出し先。手放すと消える（本を組み終えるまで持っておく）。
pub struct ShelfEpubStaging {
    root: PathBuf,
}

impl ShelfEpubStaging {
    fn create() -> Result<Self> {
        let root = std::env::temp_dir().join(format!("piep-epub-{}", LibraryId::now_v7().simple()));
        std::fs::create_dir_all(root.join("data_assets"))?;
        Ok(Self { root })
    }

    pub fn assets_dir(&self) -> PathBuf {
        self.root.join("data_assets")
    }
}

impl Drop for ShelfEpubStaging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// EPUB の変換に渡すもの。
pub struct ShelfEpubSource {
    /// 取得元の応答（FANBOX は投稿の形へ寄せる前）。添付の本文と編集はまだ差し込んでいない。
    pub data: Value,
    pub provider: String,
    pub title: String,
    /// 書き出した添付（添付の中の本文を差し込むときに引く）。
    pub attachments: Vec<AssetEntry>,
    /// 反映した編集を取得元の形へ組み直したもの。
    pub edit: Option<EditedSourceForm>,
    /// 書き出した表紙。
    pub cover: Option<PathBuf>,
    pub staging: ShelfEpubStaging,
}

/// 書き出す名前。元の名前のファイル名の部分だけを使う（置き場の外へ出さない）。
fn file_name(name: Option<&str>, fallback: &str) -> String {
    let name = name
        .and_then(|value| Path::new(value).file_name())
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty() && *value != "." && *value != "..")
        .unwrap_or(fallback);
    piep_reader_render::sanitize_filename(name)
}

/// 同じ名前を二度書かない（先のものを残す）。
fn export_unique(
    library: &Library,
    work: LibraryId,
    scope: ResourceScope,
    reference: &ResourceReference,
    destination: PathBuf,
) -> Result<Option<PathBuf>> {
    if destination.exists() {
        return Ok(None);
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    library.export_resource(work, scope, reference, &destination)?;
    Ok(Some(destination))
}

impl Library {
    /// この作品の今読む版（反映した編集があればその基準版と編集）を、EPUB の変換が読む形で書き出す。
    pub fn epub_source(&self, work: LibraryId) -> Result<ShelfEpubSource> {
        let card = self.work_card(work)?;
        let edit = card.edit_id.map(|edit| self.edit(work, edit)).transpose()?;
        let base = edit.as_ref().map_or(card.snapshot_id, |edit| edit.base);
        let scope = ResourceScope::Snapshot(base);
        let manifest = self.resource_manifest(work, scope)?;

        // 原応答。同じ版に複数あれば新しい取得のもの（ID は時刻順）。
        let original = manifest
            .resources
            .iter()
            .filter(|r| r.file.is_some())
            .filter_map(|r| match &r.reference {
                ResourceReference::Original { observation } => Some(*observation),
                _ => None,
            })
            .max()
            .ok_or_else(|| Error::Unsupported("この作品には取得元の応答が残っていません".into()))?;
        let mut raw = Vec::new();
        self.read_resource(
            work,
            scope,
            &ResourceReference::Original {
                observation: original,
            },
            None,
            &mut raw,
        )?;
        let data: Value = serde_json::from_slice(&raw)?;

        let staging = ShelfEpubStaging::create()?;
        let assets = staging.assets_dir();
        let mut attachments = Vec::new();
        let mut cover = None;
        // 版の画像の指紋と書き出した場所（編集が同じ画像を指しているかを見る）。
        let mut by_file: HashMap<Fingerprint, (String, PathBuf)> = HashMap::new();
        for (index, resource) in manifest.resources.iter().enumerate() {
            let Some(file) = &resource.file else {
                continue;
            };
            let fallback = format!("asset-{index}");
            let name = file_name(resource.original_name.as_deref(), &fallback);
            let (dir, attachment) = match &resource.reference {
                ResourceReference::Media { .. } if resource.role == "cover" => ("cover", false),
                ResourceReference::Media { .. } => ("illustrations", false),
                ResourceReference::File { .. } => ("files", true),
                _ => continue,
            };
            let Some(path) = export_unique(
                self,
                work,
                scope,
                &resource.reference,
                assets.join(dir).join(&name),
            )?
            else {
                continue;
            };
            if attachment {
                attachments.push(AssetEntry {
                    id: attachments.len() as i64 + 1,
                    download_id: 0,
                    asset_type: "file".into(),
                    filename: name.clone(),
                    local_path: path.to_string_lossy().into_owned(),
                    original_url: None,
                    mime_type: resource.mime_type.clone(),
                    file_size_bytes: file.length as i64,
                });
            } else {
                if dir == "cover" && cover.is_none() {
                    cover = Some(path.clone());
                }
                by_file.entry(file.id).or_insert((name, path));
            }
        }

        let edit = match edit {
            None => None,
            Some(edit) => {
                let scope = ResourceScope::Edit(edit.id);
                let mut entries = Vec::new();
                let mut slots = HashMap::new();
                let mut added_images = Vec::new();
                for (index, media) in edit.content.media.iter().enumerate() {
                    let id = index as i64 + 1;
                    slots.insert(media.slot.clone(), id);
                    // 版にある画像を指すなら取得元の画像として、そうでなければ編集で足した画像として。
                    let entry = match by_file.get(&media.file) {
                        Some((name, path)) => AssetEntry {
                            id,
                            download_id: 0,
                            asset_type: "illustration".into(),
                            filename: name.clone(),
                            local_path: path.to_string_lossy().into_owned(),
                            original_url: None,
                            mime_type: media.mime_type.clone(),
                            file_size_bytes: 0,
                        },
                        None => {
                            // 名前が重なっても別の画像として本に入るよう、番号を先に付ける。
                            let name =
                                format!("{id}-{}", file_name(Some(&media.original_name), "image"));
                            let path = staging.root.join("edit-images").join(&name);
                            export_unique(
                                self,
                                work,
                                scope,
                                &ResourceReference::Media {
                                    slot: media.slot.clone(),
                                },
                                path.clone(),
                            )?;
                            let entry = AssetEntry {
                                id,
                                download_id: 0,
                                asset_type: EDITOR_IMAGE_ASSET_TYPE.into(),
                                filename: name,
                                local_path: path.to_string_lossy().into_owned(),
                                original_url: None,
                                mime_type: media.mime_type.clone(),
                                file_size_bytes: 0,
                            };
                            added_images.push(entry.clone());
                            entry
                        }
                    };
                    entries.push(entry);
                }
                let blocks: Vec<WorkBlock> = edit
                    .content
                    .blocks
                    .iter()
                    .enumerate()
                    .map(|(index, block)| WorkBlock {
                        id: index as i64 + 1,
                        edit_revision_id: 0,
                        order: block.ordinal,
                        block_type: block.kind.clone(),
                        text: block.text.clone(),
                        asset_id: block
                            .media_slot
                            .as_ref()
                            .and_then(|slot| slots.get(slot).copied()),
                        attrs_json: (!block.attrs.is_null()).then(|| block.attrs.to_string()),
                    })
                    .collect();
                // 本文が指していない足した画像は本に入れない。
                added_images
                    .retain(|image| blocks.iter().any(|block| block.asset_id == Some(image.id)));
                Some(EditedSourceForm {
                    pixiv_text: blocks_to_pixiv_text(&blocks, &entries),
                    fanbox_blocks: blocks_to_fanbox_blocks(&blocks, &entries),
                    plain_text: blocks_to_plain_text(&blocks),
                    added_images,
                })
            }
        };

        Ok(ShelfEpubSource {
            data,
            provider: card.provider,
            title: card.title,
            attachments,
            edit,
            cover,
            staging,
        })
    }
}

#[cfg(test)]
#[path = "library_epub_tests.rs"]
mod tests;
