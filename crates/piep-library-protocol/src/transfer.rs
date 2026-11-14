use crate::{Decimal, Hash, Id};
use piep_storage::{ContentFile, Error, FileChunk, Fingerprint, Result};
use serde::{
    de::{Error as _, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};

pub const MAX_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_BODY_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_FILE_CHUNKS: usize = 128;
const FORMAT: &str = "piep-resource-transfer-v1";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    content = "id",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ResourceScope {
    Snapshot(Id),
    Edit(Id),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceReference {
    Body,
    Original { observation: Id },
    Media { slot: String },
    File { slot: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferTarget {
    pub library_id: Id,
    pub entity_id: Id,
    pub scope: ResourceScope,
    pub resource: ResourceReference,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
    pub id: Hash,
    pub length: Decimal,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct File {
    pub id: Hash,
    pub length: Decimal,
    #[serde(deserialize_with = "bounded_chunks")]
    pub chunks: Vec<Chunk>,
}

fn bounded_chunks<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Vec<Chunk>, D::Error> {
    struct Chunks;
    impl<'de> Visitor<'de> for Chunks {
        type Value = Vec<Chunk>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("最大 128 個の chunk")
        }
        fn visit_seq<A: SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut chunks = Vec::new();
            while let Some(chunk) = seq.next_element()? {
                if chunks.len() == MAX_FILE_CHUNKS {
                    return Err(A::Error::custom("chunk の個数が上限を超えています"));
                }
                chunks.push(chunk);
            }
            Ok(chunks)
        }
    }
    d.deserialize_seq(Chunks)
}

impl File {
    pub fn from_native(file: &ContentFile) -> Self {
        Self {
            id: Hash::new(file.id),
            length: Decimal::new(file.length),
            chunks: file
                .chunks
                .iter()
                .map(|chunk| Chunk {
                    id: Hash::new(chunk.id),
                    length: Decimal::new(chunk.length),
                })
                .collect(),
        }
    }
    pub fn to_native(&self) -> ContentFile {
        ContentFile {
            id: self.id.get(),
            length: self.length.get(),
            chunks: self
                .chunks
                .iter()
                .map(|chunk| FileChunk {
                    id: chunk.id.get(),
                    length: chunk.length.get(),
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TransferManifest {
    pub format: String,
    pub target: TransferTarget,
    pub file: File,
}

impl TransferManifest {
    pub fn new(target: TransferTarget, file: &ContentFile) -> Result<Self> {
        let result = Self {
            format: FORMAT.into(),
            target,
            file: File::from_native(file),
        };
        result.validate()?;
        Ok(result)
    }
    pub fn validate(&self) -> Result<()> {
        if self.format != FORMAT
            || self.file.length.get() > MAX_FILE_BYTES
            || self.file.chunks.len() > MAX_FILE_CHUNKS
        {
            return Err(Error::Integrity(
                "転送形式またはファイルの上限が不正です".into(),
            ));
        }
        if matches!(self.target.resource, ResourceReference::Body)
            && self.file.length.get() > MAX_BODY_BYTES
        {
            return Err(Error::Integrity(
                "本文ファイルが転送上限を超えています".into(),
            ));
        }
        match &self.target.resource {
            ResourceReference::Media { slot } | ResourceReference::File { slot }
                if slot.is_empty() || slot.len() > 4096 || slot.chars().any(char::is_control) =>
            {
                return Err(Error::Integrity("資産の識別が不正です".into()))
            }
            _ => (),
        }
        self.file.to_native().validate_layout()
    }
    /// 同じ struct のエンコードは一定。受信時の ID は元の JSON バイト列で計算する。
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|e| Error::Integrity(e.to_string()))?;
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(Error::Integrity("転送記録が上限を超えています".into()));
        }
        Ok(bytes)
    }
}

/// 構造の検証済み記録。file の存在・バイト列・認可・bundle の完成は表さない。
pub struct ValidatedTransfer {
    bytes: Vec<u8>,
    id: Fingerprint,
    manifest: TransferManifest,
    file: ContentFile,
}
impl ValidatedTransfer {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_MANIFEST_BYTES {
            return Err(Error::Integrity("転送記録が上限を超えています".into()));
        }
        let manifest: TransferManifest =
            serde_json::from_slice(bytes).map_err(|e| Error::Integrity(e.to_string()))?;
        manifest.validate()?;
        let file = manifest.file.to_native();
        Ok(Self {
            bytes: bytes.to_vec(),
            id: Fingerprint::of(bytes),
            manifest,
            file,
        })
    }
    pub fn id(&self) -> Fingerprint {
        self.id
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn manifest(&self) -> &TransferManifest {
        &self.manifest
    }
    pub fn file(&self) -> &ContentFile {
        &self.file
    }
}
