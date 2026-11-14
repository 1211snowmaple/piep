use super::{rows::mark_row, LegacyImporter};
use crate::library::{
    BodyAssetReference, Error, Library, LibraryId, ReaderBinding, ResourceReference, Result,
    SourceBody,
};
use rusqlite::OptionalExtension;
use serde_json::Value;
use std::{
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

struct Asset {
    reference: ResourceReference,
    name: String,
    paths: Vec<PathBuf>,
    url: Option<String>,
}
pub(super) fn asset_reference(role: &str, name: &str) -> Result<ResourceReference> {
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
fn ordinary(path: &Path) -> PathBuf {
    #[cfg(windows)]
    if let Some(path) = path.to_str() {
        if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = path.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path.to_path_buf()
}
fn spelling(base: &Path, stored: &str) -> Result<PathBuf> {
    let stored = Path::new(stored);
    if stored
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(Error::UnsafePath(stored.into()));
    }
    let path = ordinary(&if stored.is_absolute() {
        stored.to_path_buf()
    } else {
        base.join(stored)
    });
    let path: PathBuf = path
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    // 本文内のパス自体は開かず、所属を照合済みの資産記録とだけ比較する。
    Ok(path)
}
fn sanitized(name: &str) -> String {
    let name = name.replace("..", "__");
    name.chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}
fn entry<'a>(body: &'a SourceBody, reference: &BodyAssetReference) -> Option<&'a Value> {
    let SourceBody::FanboxBody(body) = body else {
        return None;
    };
    match reference {
        BodyAssetReference::ArticleImage { key } => body.get("imageMap")?.get(key),
        BodyAssetReference::ArticleFile { key } => body.get("fileMap")?.get(key),
        BodyAssetReference::ArticleImageAt { index } => body.get("images")?.get(*index as usize),
        BodyAssetReference::ArticleFileAt { index } => body.get("files")?.get(*index as usize),
        _ => None,
    }
}
fn choose(
    root: &Path,
    json: &Path,
    body: &SourceBody,
    reference: &BodyAssetReference,
    assets: &[Asset],
) -> Result<Option<ResourceReference>> {
    let data = entry(body, reference);
    let url = data
        .and_then(|v| {
            v.get(if reference.is_image() {
                "originalUrl"
            } else {
                "url"
            })
        })
        .and_then(Value::as_str);
    let local = data
        .and_then(|v| v.get("localPath"))
        .and_then(Value::as_str)
        .map(|p| {
            spelling(
                json.parent()
                    .ok_or_else(|| Error::UnsafePath(json.into()))?,
                p,
            )
        })
        .transpose()?;
    if let Some(local) = &local {
        // 元記録の表記も、実在する同一資産として検査したものだけを許可する。
        // Windows の大小文字・短い名前・verbatim 表記を一律に潰して別ファイルを混ぜない。
        if !local.starts_with(ordinary(root)) && !assets.iter().any(|a| a.paths.contains(local)) {
            return Err(Error::UnsafePath(local.clone()));
        }
    }
    let mut stems = Vec::new();
    match reference {
        BodyAssetReference::PixivUploaded { image_id } => stems.extend([
            format!("inline_{image_id}"),
            image_id.clone(),
            format!("illust_inline_{image_id}"),
        ]),
        BodyAssetReference::PixivIllustration {
            illustration_id,
            page,
        } => stems.extend([
            format!("{illustration_id}_p{}", page - 1),
            format!("illust_{illustration_id}_p{}", page - 1),
            format!("{illustration_id}-{page}_p{}", page - 1),
        ]),
        _ => {
            let id = data.and_then(|v| v.get("id")).and_then(|id| {
                id.as_str()
                    .map(str::to_owned)
                    .or_else(|| id.as_i64().map(|id| id.to_string()))
            });
            if let Some(id) = id {
                if reference.is_image() {
                    stems.push(sanitized(&id));
                } else if let Some(name) = data.and_then(|v| v.get("name")).and_then(Value::as_str)
                {
                    stems.push(sanitized(&format!("{name}_{id}")));
                }
            }
        }
    }
    let mut candidates = Vec::new();
    for asset in assets {
        if matches!(asset.reference, ResourceReference::Media { .. }) != reference.is_image() {
            continue;
        }
        let exact_url = url.is_some_and(|url| asset.url.as_deref() == Some(url));
        let exact_path = local
            .as_ref()
            .is_some_and(|local| asset.paths.contains(local));
        let stem = Path::new(&asset.name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        // 取得元が別だと分かる画像を、似た名前だけで代用しない。
        let compatible = url.is_none() || asset.url.is_none() || exact_url;
        if exact_path && !compatible {
            return Err(Error::Integrity(
                "本文の保存先と取得 URL の記録が食い違います".into(),
            ));
        }
        let named = compatible && stems.iter().any(|name| name == stem);
        if exact_url || exact_path || named {
            candidates.push(asset.reference.clone());
        }
    }
    candidates.sort();
    candidates.dedup();
    match candidates.len() {
        0 => Ok(None),
        1 => Ok(candidates.pop()),
        _ => Err(Error::Integrity(
            "本文の画像・添付を一意に対応付けられません".into(),
        )),
    }
}

impl LegacyImporter {
    pub(super) fn normalize_reader(&self, target: &Library, cancel: &AtomicBool) -> Result<()> {
        let mut stmt=self.conn.prepare("SELECT id,download_id,version,coalesce(original_json_path,json_path) FROM download_versions ORDER BY download_id,version,id")?;
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })? {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::Canceled);
            }
            let (old, work, version, json) = row?;
            let migration_key = format!("{work}:{old}:{version}");
            let (entity, snapshot) = {
                let conn = target.conn.lock().map_err(|_| Error::Busy)?;
                let row:Option<(Vec<u8>,Vec<u8>)>=conn.query_row("SELECT s.entity_id,s.id FROM migration_records m JOIN revision_snapshots s ON s.id=m.target_id WHERE m.source_table='download_versions' AND m.source_key=?1 AND m.status='imported'",[&migration_key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
                let (entity, snapshot) = row.ok_or_else(|| {
                    Error::Integrity("本文を読む対象の保存版を移行できていません".into())
                })?;
                (
                    LibraryId::from_slice(&entity)
                        .map_err(|_| Error::Integrity("読書対象が不正です".into()))?,
                    LibraryId::from_slice(&snapshot)
                        .map_err(|_| Error::Integrity("読書対象の保存版が不正です".into()))?,
                )
            };
            let Some(body) = target.source_body(entity, snapshot)? else {
                let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
                let tx = conn.transaction()?;
                mark_row(&tx, "reader_body_unavailable", &migration_key, snapshot)?;
                tx.commit()?;
                continue;
            };
            let path = self.managed_path(&json)?;
            let mut assets = Vec::new();
            self.each_row_matching("assets", "download_id", work, |row| {
                let role = row.text("asset_type")?;
                if role == "cover"
                    || role == "editor_image"
                    || !self.asset_in_version(work, row.text("local_path")?, &json)?
                {
                    return Ok(());
                }
                let name = row.text("filename")?.to_owned();
                let reference = asset_reference(role, &name)?;
                // FK と元の所属版・容量・ハッシュの照合は先の資産移行で確定済み。
                target.resource_info(
                    entity,
                    crate::library::ResourceScope::Snapshot(snapshot),
                    &reference,
                )?;
                assets.push(Asset {
                    reference,
                    name,
                    paths: vec![
                        ordinary(&self.managed_path(row.text("local_path")?)?),
                        spelling(&self.root, row.text("local_path")?)?,
                    ],
                    url: row.optional_text("original_url")?.map(str::to_owned),
                });
                Ok(())
            })?;
            let bindings = body
                .asset_references()?
                .into_iter()
                .map(|reference| {
                    Ok(ReaderBinding {
                        resource: choose(&self.root, &path, &body, &reference, &assets)?,
                        reference,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            target.publish_reader_bindings(entity, snapshot, bindings)?;
            let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
            let tx = conn.transaction()?;
            mark_row(&tx, "reader_bindings", &migration_key, snapshot)?;
            tx.commit()?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "reader_tests.rs"]
mod tests;
