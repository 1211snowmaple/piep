//! 単独サーバーの小さな状態ファイルを、内容を同期してから差し替える。

use piep_storage::sync_directory;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use uuid::Uuid;

fn parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

pub(crate) fn write_atomic(path: &Path, content: &[u8]) -> Result<(), String> {
    StagedWrite::new(path, content)?.commit()
}

/// 差し替える前に複数のファイルを同期しておける。確定できなかった一時ファイルは片付ける。
pub(super) struct StagedWrite<'a> {
    target: &'a Path,
    temporary: PathBuf,
}

impl<'a> StagedWrite<'a> {
    pub(super) fn new(target: &'a Path, content: &[u8]) -> Result<Self, String> {
        let temporary = parent(target).join(format!(".{}.part", Uuid::now_v7()));
        let write_error = |error| format!("{} を書けません: {error}", target.display());
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(write_error)?;
        let staged = Self { target, temporary };
        let result = file.write_all(content).and_then(|()| file.sync_all());
        drop(file);
        result.map_err(write_error)?;
        Ok(staged)
    }

    pub(super) fn commit(mut self) -> Result<(), String> {
        fs::rename(&self.temporary, self.target)
            .map_err(|error| format!("{} を置けません: {error}", self.target.display()))?;
        self.temporary.clear();
        let directory = parent(self.target);
        sync_directory(directory)
            .map_err(|error| format!("{} を同期できません: {error}", directory.display()))
    }
}

impl Drop for StagedWrite<'_> {
    fn drop(&mut self) {
        if !self.temporary.as_os_str().is_empty() {
            let _ = fs::remove_file(&self.temporary);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::TestRoot;

    #[test]
    fn staged_write_keeps_old_content_until_commit_and_cleans_up() {
        let root = TestRoot::created("durable");
        let target = root.0.join("state");
        write_atomic(&target, b"old").unwrap();

        // 電源断の代わりに、確定前に一時ファイルを破棄する。
        let staged = StagedWrite::new(&target, b"new").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"old");
        drop(staged);
        assert_eq!(fs::read(&target).unwrap(), b"old");
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 1);

        write_atomic(&target, b"new").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 1);
    }
}
