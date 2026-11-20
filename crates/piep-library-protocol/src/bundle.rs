//! 読書 bundle。一つの保存状態（または編集）を読むのに要る文書と、全資産の file 記述を
//! 一回の応答で渡す。部分実体は今までどおり一つずつ取る（`File` の chunk の一覧で指定する）。
use crate::{
    Error, File, Id, ResourceReference, ResourceScope, Result, MAX_BODY_BYTES, MAX_FILE_BYTES,
    MAX_FILE_CHUNKS, MAX_MANIFEST_BYTES,
};
use serde::{Deserialize, Serialize};

pub const BUNDLE_FORMAT: &str = "piep-reader-bundle-v1";
/// 一つの bundle に載せる file の上限（本文＋本文から指せる画像・添付の上限）。
pub const MAX_BUNDLE_FILES: usize = 4097;

/// bundle の一つの file。`scope` は file を取るときの view（編集の表紙は基準の保存版から取る）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BundleFile {
    pub scope: ResourceScope,
    pub resource: ResourceReference,
    pub file: File,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReaderBundle {
    pub format: String,
    pub library_id: Id,
    pub entity_id: Id,
    pub scope: ResourceScope,
    /// `piep-reader-v1` の読書用文書（本文・対応表・資産の一覧）。
    pub document: serde_json::Value,
    pub files: Vec<BundleFile>,
}

impl ReaderBundle {
    pub fn validate(&self) -> Result<()> {
        let fail = |message: &str| Err(Error::Integrity(message.into()));
        if self.format != BUNDLE_FORMAT {
            return fail("読書 bundle の形式が一致しません");
        }
        if self.document.get("format").and_then(|v| v.as_str()) != Some("piep-reader-v1") {
            return fail("読書用文書の形式が一致しません");
        }
        if self.files.len() > MAX_BUNDLE_FILES {
            return fail("読書 bundle の file が上限を超えています");
        }
        let mut seen = std::collections::BTreeSet::new();
        for item in &self.files {
            if !seen.insert((
                serde_json::to_string(&item.scope).unwrap_or_default(),
                serde_json::to_string(&item.resource).unwrap_or_default(),
            )) {
                return fail("読書 bundle に同じ資産が二度あります");
            }
            match &item.resource {
                ResourceReference::Original { .. } => {
                    return fail("読書 bundle に原応答は入れません")
                }
                ResourceReference::Body if item.file.length.get() > MAX_BODY_BYTES => {
                    return fail("本文ファイルが転送上限を超えています")
                }
                ResourceReference::Media { slot } | ResourceReference::File { slot }
                    if slot.is_empty()
                        || slot.len() > 4096
                        || slot.chars().any(char::is_control) =>
                {
                    return fail("資産の識別が不正です")
                }
                _ => {}
            }
            if item.file.length.get() > MAX_FILE_BYTES || item.file.chunks.len() > MAX_FILE_CHUNKS {
                return fail("ファイルが転送上限を超えています");
            }
            item.file.to_native().validate_layout()?;
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|e| Error::Integrity(e.to_string()))?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(Error::Integrity("読書 bundle が上限を超えています".into()));
        }
        Ok(bytes)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(Error::Integrity("読書 bundle が上限を超えています".into()));
        }
        let bundle: Self =
            serde_json::from_slice(bytes).map_err(|e| Error::Integrity(e.to_string()))?;
        bundle.validate()?;
        Ok(bundle)
    }
}
