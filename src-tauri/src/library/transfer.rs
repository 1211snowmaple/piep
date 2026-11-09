//! 所属指定で転送を組み立てる。HTTP の認証・利用者 ACL は呼び手で別途必要。
use super::{Error, Fingerprint, Library, LibraryId, ResourceReference, ResourceScope, Result};
use piep_library_protocol::{
    Id, ResourceReference as WireReference, ResourceScope as WireScope, TransferManifest,
    TransferTarget,
};
use std::io::Write;

impl Library {
    /// 不変な保存状態の file を指定する。現在版への追従や未保存資産の代用をしない。
    /// descriptor の構造を検査するが、全実体の存在・バイト列の検証は表さない。
    pub fn resource_transfer_manifest(
        &self,
        entity: LibraryId,
        scope: ResourceScope,
        reference: &ResourceReference,
    ) -> Result<TransferManifest> {
        let file = self.resource_file(entity, scope, reference)?;
        let scope = match scope {
            ResourceScope::Snapshot(id) => WireScope::Snapshot(Id::new(id)?),
            ResourceScope::Edit(id) => WireScope::Edit(Id::new(id)?),
        };
        let resource = match reference {
            ResourceReference::Body => WireReference::Body,
            ResourceReference::Original { observation } => WireReference::Original {
                observation: Id::new(*observation)?,
            },
            ResourceReference::Media { slot } => WireReference::Media { slot: slot.clone() },
            ResourceReference::File { slot } => WireReference::File { slot: slot.clone() },
        };
        Ok(TransferManifest::new(
            TransferTarget {
                library_id: Id::new(self.id)?,
                entity_id: Id::new(entity)?,
                scope,
                resource,
            },
            &file,
        )?)
    }

    /// 所属を毎回照合してから、指定 file の指定部分だけを圧縮したまま渡す。
    /// ハッシュだけを認可に使わず、I/O の間は DB の錠を保持しない。
    pub fn export_resource_chunk(
        &self,
        entity: LibraryId,
        scope: ResourceScope,
        reference: &ResourceReference,
        expected_file: Fingerprint,
        ordinal: usize,
        destination: impl Write,
    ) -> Result<()> {
        let file = self.resource_file(entity, scope, reference)?;
        if file.id != expected_file {
            return Err(Error::ResourceUnavailable);
        }
        let chunk = file.chunks.get(ordinal).ok_or(Error::InvalidRange)?;
        self.objects
            .export_chunk(chunk, destination)
            .map_err(Into::into)
    }
}
