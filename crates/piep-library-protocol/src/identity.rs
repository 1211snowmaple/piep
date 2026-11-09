use crate::{Decimal, Error, Id, Result, MAX_BODY_BYTES, MAX_FILE_BYTES, MAX_MANIFEST_BYTES};
use serde::{Deserialize, Serialize};

pub const MAX_IDENTITY_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Capabilities {
    pub resource_transfer: bool,
    pub reader_bundle: bool,
    pub mutations: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LibraryIdentity {
    pub format: String,
    pub api_version: u32,
    pub storage_version: u32,
    pub transfer_format: String,
    pub library_id: Id,
    pub subject_id: Id,
    pub sync_epoch: Id,
    pub capabilities: Capabilities,
    pub max_file_bytes: Decimal,
    pub max_body_bytes: Decimal,
    pub max_manifest_bytes: Decimal,
}

impl LibraryIdentity {
    pub fn new(library_id: Id, subject_id: Id, sync_epoch: Id) -> Self {
        Self {
            format: "piep-library-api-v2".into(),
            api_version: 2,
            storage_version: 1,
            transfer_format: "piep-resource-transfer-v1".into(),
            library_id,
            subject_id,
            sync_epoch,
            capabilities: Capabilities {
                resource_transfer: true,
                reader_bundle: false,
                mutations: false,
            },
            max_file_bytes: Decimal::new(MAX_FILE_BYTES),
            max_body_bytes: Decimal::new(MAX_BODY_BYTES),
            max_manifest_bytes: Decimal::new(MAX_MANIFEST_BYTES as u64),
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.format != "piep-library-api-v2"
            || self.api_version != 2
            || self.storage_version != 1
            || self.transfer_format != "piep-resource-transfer-v1"
            || !self.capabilities.resource_transfer
            || self.max_file_bytes.get() == 0
            || self.max_file_bytes.get() > MAX_FILE_BYTES
            || self.max_body_bytes.get() == 0
            || self.max_body_bytes.get() > MAX_BODY_BYTES
            || self.max_manifest_bytes.get() == 0
            || self.max_manifest_bytes.get() > MAX_MANIFEST_BYTES as u64
        {
            return Err(Error::Integrity(
                "新ライブラリの対応形式または上限が一致しません".into(),
            ));
        }
        Ok(())
    }
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_IDENTITY_BYTES {
            return Err(Error::Integrity("接続情報が上限を超えています".into()));
        }
        let result: Self =
            serde_json::from_slice(bytes).map_err(|e| Error::Integrity(e.to_string()))?;
        result.validate()?;
        Ok(result)
    }
}
