//! 所属指定で転送を組み立てる。HTTP の認証・利用者 ACL は呼び手で別途必要。
use super::{
    Error, Fingerprint, Library, LibraryId, ResourceInfo, ResourceReference, ResourceScope, Result,
};
use piep_library_protocol::{
    BundleFile, File as WireFile, Id, ReaderBundle, ResourceReference as WireReference,
    ResourceScope as WireScope, TransferManifest, TransferTarget, BUNDLE_FORMAT,
};
use std::io::Write;

fn wire_scope(scope: ResourceScope) -> Result<WireScope> {
    Ok(match scope {
        ResourceScope::Snapshot(id) => WireScope::Snapshot(Id::new(id)?),
        ResourceScope::Edit(id) => WireScope::Edit(Id::new(id)?),
    })
}

fn wire_reference(reference: &ResourceReference) -> Result<WireReference> {
    Ok(match reference {
        ResourceReference::Body => WireReference::Body,
        ResourceReference::Original { observation } => WireReference::Original {
            observation: Id::new(*observation)?,
        },
        ResourceReference::Media { slot } => WireReference::Media { slot: slot.clone() },
        ResourceReference::File { slot } => WireReference::File { slot: slot.clone() },
    })
}

/// 読書の側（携帯・ほかの端末）へ渡してよい資産か。原応答は内部の情報を含むことがあるので
/// 渡さない。添付は読書に使うもの（添付・文書）だけで、保管用の書庫などは渡さない。
pub fn reader_may_read(info: &ResourceInfo) -> bool {
    match info.reference {
        ResourceReference::Original { .. } => false,
        ResourceReference::File { .. } => matches!(info.role.as_str(), "attachment" | "document"),
        ResourceReference::Body | ResourceReference::Media { .. } => true,
    }
}

impl Library {
    /// 一つの保存状態（または編集）を読むのに要る文書と、渡してよい全資産の file 記述。
    /// 編集の基準の保存版の資産は、その保存版の view で取る形で載せる。未保存の資産は載せない。
    pub fn reader_bundle(&self, entity: LibraryId, scope: ResourceScope) -> Result<ReaderBundle> {
        let document = self.reader_document(entity, scope)?;
        let mut files = Vec::new();
        let manifests = std::iter::once(&document.resources).chain(document.base_resources.iter());
        for manifest in manifests {
            for info in &manifest.resources {
                if info.file.is_none() || !reader_may_read(info) {
                    continue;
                }
                let file = self.resource_file(entity, manifest.scope, &info.reference)?;
                files.push(BundleFile {
                    scope: wire_scope(manifest.scope)?,
                    resource: wire_reference(&info.reference)?,
                    file: WireFile::from_native(&file),
                });
            }
        }
        let bundle = ReaderBundle {
            format: BUNDLE_FORMAT.into(),
            library_id: Id::new(self.id)?,
            entity_id: Id::new(entity)?,
            scope: wire_scope(scope)?,
            document: serde_json::to_value(&document)?,
            files,
        };
        bundle.validate()?;
        Ok(bundle)
    }

    /// 不変な保存状態の file を指定する。現在版への追従や未保存資産の代用をしない。
    /// descriptor の構造を検査するが、全実体の存在・バイト列の検証は表さない。
    pub fn resource_transfer_manifest(
        &self,
        entity: LibraryId,
        scope: ResourceScope,
        reference: &ResourceReference,
    ) -> Result<TransferManifest> {
        let file = self.resource_file(entity, scope, reference)?;
        Ok(TransferManifest::new(
            TransferTarget {
                library_id: Id::new(self.id)?,
                entity_id: Id::new(entity)?,
                scope: wire_scope(scope)?,
                resource: wire_reference(reference)?,
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
