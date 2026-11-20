//! 取得元の本文の指定と、取得した画像・添付の名前・URL を照合する共通の規則。
use super::{BodyAssetReference, SourceBody};
use serde_json::Value;
use std::path::Path;

pub(crate) fn entry<'a>(body: &'a SourceBody, reference: &BodyAssetReference) -> Option<&'a Value> {
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

fn sanitized(name: &str) -> String {
    name.replace("..", "__")
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect::<String>()
        .trim()
        .to_owned()
}

pub(crate) fn source_matches(
    body: &SourceBody,
    reference: &BodyAssetReference,
    name: &str,
    held_url: Option<&str>,
) -> bool {
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
    if url.is_some_and(|url| held_url == Some(url)) {
        return true;
    }
    if url.is_some() && held_url.is_some() {
        return false;
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
            let id = data.and_then(|v| v.get("id")).and_then(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
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
    let stem = Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    stems.iter().any(|name| name == stem)
}
