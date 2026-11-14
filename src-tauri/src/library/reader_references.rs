use super::{Error, Result, SourceBody};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, sync::LazyLock};

pub(super) const MAX_BODY_ASSETS: usize = 10_000;
fn insert(set: &mut BTreeSet<BodyAssetReference>, reference: BodyAssetReference) -> Result<()> {
    set.insert(reference);
    if set.len() > MAX_BODY_ASSETS {
        return Err(Error::Unsupported(
            "本文の画像・添付指定が読み込み上限を超えています".into(),
        ));
    }
    Ok(())
}

/// 本文に書かれた指定。取得 URL・保存名・OS のパスを識別子に使わない。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BodyAssetReference {
    PixivUploaded {
        image_id: String,
    },
    /// 本文のページ番号は 1 始まりのまま保持する。
    PixivIllustration {
        illustration_id: String,
        page: u32,
    },
    ArticleImage {
        key: String,
    },
    ArticleFile {
        key: String,
    },
    ArticleImageAt {
        index: u32,
    },
    ArticleFileAt {
        index: u32,
    },
}
impl BodyAssetReference {
    pub(super) fn is_image(&self) -> bool {
        !matches!(self, Self::ArticleFile { .. } | Self::ArticleFileAt { .. })
    }
}
static PIXIV_IMAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(uploadedimage|pixivimage):([0-9]+)(?:-([0-9]+))?\]").unwrap());

impl SourceBody {
    pub fn asset_references(&self) -> Result<Vec<BodyAssetReference>> {
        Ok(required(self)?.into_iter().collect())
    }
}

fn key(value: &Value) -> Result<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_u64().map(|id| id.to_string()))
        .filter(|key| !key.is_empty() && key.len() <= 1024)
        .ok_or_else(|| Error::Unsupported("本文中の画像・添付の識別子を解釈できません".into()))
}

pub(super) fn required(body: &SourceBody) -> Result<BTreeSet<BodyAssetReference>> {
    let mut references = BTreeSet::new();
    match body {
        SourceBody::PlainText(_) => {}
        SourceBody::PixivText(text) => {
            for caps in PIXIV_IMAGE.captures_iter(text) {
                let id = &caps[2];
                if id.len() > 1024 {
                    return Err(Error::Unsupported("本文の画像指定が長すぎます".into()));
                }
                let reference = if &caps[1] == "uploadedimage" {
                    if caps.get(3).is_some() {
                        continue;
                    }
                    BodyAssetReference::PixivUploaded {
                        image_id: id.into(),
                    }
                } else {
                    let page = match caps.get(3) {
                        Some(page) => match page.as_str().parse::<u32>() {
                            Ok(page) if page > 0 => page,
                            _ => continue,
                        },
                        None => 1,
                    };
                    BodyAssetReference::PixivIllustration {
                        illustration_id: id.into(),
                        page,
                    }
                };
                insert(&mut references, reference)?;
            }
        }
        SourceBody::FanboxBody(body) => {
            for (name, image) in [("imageMap", true), ("fileMap", false)] {
                if let Some(map) = body.get(name).filter(|v| !v.is_null()) {
                    let map = map.as_object().ok_or_else(|| {
                        Error::Unsupported("本文の画像・添付一覧を解釈できません".into())
                    })?;
                    for name in map.keys() {
                        let key = key(&Value::String(name.clone()))?;
                        insert(
                            &mut references,
                            if image {
                                BodyAssetReference::ArticleImage { key }
                            } else {
                                BodyAssetReference::ArticleFile { key }
                            },
                        )?;
                    }
                }
            }
            for (name, image) in [("images", true), ("files", false)] {
                if let Some(array) = body.get(name).filter(|v| !v.is_null()) {
                    let array = array.as_array().ok_or_else(|| {
                        Error::Unsupported("本文の画像・添付配列を解釈できません".into())
                    })?;
                    for index in 0..array.len() {
                        let index = u32::try_from(index).map_err(|_| {
                            Error::Unsupported("本文の画像・添付が多すぎます".into())
                        })?;
                        insert(
                            &mut references,
                            if image {
                                BodyAssetReference::ArticleImageAt { index }
                            } else {
                                BodyAssetReference::ArticleFileAt { index }
                            },
                        )?;
                    }
                }
            }
            if let Some(blocks) = body.get("blocks").filter(|v| !v.is_null()) {
                let blocks = blocks
                    .as_array()
                    .ok_or_else(|| Error::Unsupported("本文のブロックを解釈できません".into()))?;
                for block in blocks {
                    let reference = match block.get("type").and_then(Value::as_str) {
                        Some("image") => Some(BodyAssetReference::ArticleImage {
                            key: key(&block["imageId"])?,
                        }),
                        Some("file") => Some(BodyAssetReference::ArticleFile {
                            key: key(&block["fileId"])?,
                        }),
                        _ => None,
                    };
                    if let Some(reference) = reference {
                        insert(&mut references, reference)?;
                    }
                }
            }
        }
    }
    Ok(references)
}
