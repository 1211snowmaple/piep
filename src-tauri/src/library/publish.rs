use super::{
    store::{regular_directory, sync_directory},
    Error, Library, LibraryId, Result,
};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) const BACKUP_STAGE_PREFIX: &str = ".piep-backup-";
pub(super) const PART_STAGE_PREFIX: &str = ".piep-part-";
pub(super) const SHIPMENT_STAGE_PREFIX: &str = ".piep-shipment-";
const STAGE_SUFFIX: &str = ".part";
const COPY_DIR: &str = "copy";

pub(super) fn is_stage_name(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|name| name.strip_suffix(STAGE_SUFFIX))
        .is_some_and(|id| {
            LibraryId::parse_str(id)
                .is_ok_and(|uuid| uuid.get_version_num() == 7 && id == uuid.to_string())
        })
}

/// 作業フォルダーの内側にある写しは、最終名の名札があっても公開前と扱う。
pub(super) fn is_staged_copy(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(COPY_DIR))
        && path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                [
                    BACKUP_STAGE_PREFIX,
                    PART_STAGE_PREFIX,
                    SHIPMENT_STAGE_PREFIX,
                ]
                .into_iter()
                .any(|prefix| is_stage_name(name, prefix))
            })
}

pub(super) struct StagedCopy<'a> {
    destination: &'a Path,
    parent: &'a Path,
    directory: PathBuf,
}

impl<'a> StagedCopy<'a> {
    pub(super) fn new(destination: &'a Path, prefix: &str) -> Result<Self> {
        let parent = destination
            .parent()
            .ok_or_else(|| Error::UnsafePath(destination.into()))?;
        regular_directory(parent)?;
        let name = destination
            .file_name()
            .ok_or_else(|| Error::UnsafePath(destination.into()))?;
        if destination.try_exists()? || is_staged_copy(&parent.canonicalize()?.join(name)) {
            return Err(Error::UnsafePath(destination.into()));
        }
        let directory = parent.join(format!("{prefix}{}{STAGE_SUFFIX}", LibraryId::now_v7()));
        fs::create_dir(&directory)?;
        Ok(Self {
            destination,
            parent,
            directory,
        })
    }

    pub(super) fn path(&self) -> PathBuf {
        self.directory.join(COPY_DIR)
    }

    /// 検査済みの写しに名札を書き、名札ごと一度の改名で公開する。
    pub(super) fn publish(self, id: LibraryId, marker: &str) -> Result<()> {
        let copy = self.path();
        Library::write_format_marker(&copy.join(marker), id)?;
        sync_directory(&copy)?;
        if self.destination.try_exists()? {
            return Err(Error::UnsafePath(self.destination.into()));
        }
        #[cfg(test)]
        tests::before_rename(&copy, self.destination)?;
        fs::rename(&copy, self.destination)?;
        let parent = self.parent;
        drop(self);
        sync_directory(parent)?;
        Ok(())
    }
}

impl Drop for StagedCopy<'_> {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.directory) {
            log::warn!("写しの作業フォルダーを片付けられません: {error}");
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::super::{Error, Library, Result};
    use std::cell::Cell;
    use std::path::Path;

    thread_local! {
        static INTERRUPT: Cell<Option<&'static str>> = const { Cell::new(None) };
    }

    pub(in crate::library) fn interrupt_next_publish(marker: &'static str) {
        INTERRUPT.set(Some(marker));
    }

    pub(in crate::library) fn before_rename(stage: &Path, destination: &Path) -> Result<()> {
        if let Some(marker) = INTERRUPT.take() {
            assert!(!destination.exists());
            assert!(stage.join(marker).is_file(), "公開前に {marker} が必要です");
            assert!(
                Library::open(stage).is_err(),
                "作業中の棚を開けてはいけません"
            );
            assert!(!Library::is_part(stage));
            assert!(Library::open_part(stage).is_err());
            return Err(Error::Canceled);
        }
        Ok(())
    }
}
