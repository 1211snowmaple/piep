use super::LegacyImporter;
use crate::library::{store, Error, Library, Result};
use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::BTreeSet;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) const MANAGED_ROOTS: &[&str] = &[
    "downloads",
    "profiles",
    "series",
    "editor-assets",
    "collection-covers",
    "templates",
    "fonts",
];

#[derive(Default)]
pub(super) struct FileInventory {
    pub files: u64,
    pub unresolved: u64,
}

impl LegacyImporter {
    fn relative_file_key(&self, path: &Path) -> Result<String> {
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| Error::UnsafePath(path.into()))?;
        let mut names = Vec::new();
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err(Error::UnsafePath(path.into()));
            };
            names.push(
                name.to_str()
                    .ok_or_else(|| Error::UnsafePath(path.into()))?,
            );
        }
        if names.len() < 2 || !MANAGED_ROOTS.contains(&names[0]) {
            return Err(Error::UnsafePath(path.into()));
        }
        Ok(names.join("/"))
    }

    /// 所属の正規化と同じトランザクションで、実際に使ったパスと原本を結ぶ。
    /// 同じ内容の別ファイルがあることだけでは、そのパスの移行を完了としない。
    pub(super) fn mark_used_file(
        &self,
        tx: &Transaction<'_>,
        path: &Path,
        file: &[u8],
    ) -> Result<()> {
        let key = self.relative_file_key(path)?;
        let previous: Option<Vec<u8>> = tx.query_row("SELECT evidence_file_id FROM migration_records WHERE source_table='__used_files__' AND source_key=?1",[&key],|r|r.get(0)).optional()?.flatten();
        if previous.as_ref().is_some_and(|old| old != file) {
            return Err(Error::Integrity(format!(
                "移行中に元ファイルが変わりました: {key}"
            )));
        }
        tx.execute("INSERT OR IGNORE INTO migration_records(source_table,source_key,evidence_file_id,status) VALUES('__used_files__',?1,?2,'imported')",params![key,file])?;
        Ok(())
    }

    /// DB に載らないファイルも含め、管理領域全体を保管する。元の棚には書かない。
    pub(super) fn normalize_filesystem(
        &self,
        target: &Library,
        cancel: &AtomicBool,
        mut progress: impl FnMut(u64),
    ) -> Result<FileInventory> {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Canceled);
        }
        let mut directories = Vec::new();
        let mut inventory = FileInventory::default();
        for entry in std::fs::read_dir(&self.root)? {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::Canceled);
            }
            let entry = entry?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| Error::UnsafePath(entry.path()))?;
            if MANAGED_ROOTS.contains(&name) {
                store::regular_directory(&entry.path())?;
                directories.push(entry.path());
                continue;
            }
            // 認証情報を含む可能性のある管理領域外は、内容を推測してコピーしない。
            // 除外した理由を保存し、未対応の設定・端末状態や未知の項目は未解決にする。
            let (reason, resolved) = match name {
                "piep.db" | "piep.db-wal" | "piep.db-shm" | "piep.db-journal" => {
                    ("database_rows_preserved_separately", true)
                }
                ".piep-library.lock" => ("process_lock", true),
                "logs" => ("operational_logs", true),
                "models" | "NVIDIA" | "search" | ".search-snapshots" => ("rebuildable_cache", true),
                "browsing-profile" => ("browser_credentials_and_cache", true),
                "window-color-scheme" => ("machine_window_preference", true),
                "settings.json" | "server" => ("settings_or_device_state_not_yet_migrated", false),
                _ => ("unrecognized_root_entry_not_read", false),
            };
            let evidence = serde_json::to_vec(
                &serde_json::json!({"entry":name,"reason":reason,"contents_read":false}),
            )?;
            let file = target.store_original(&evidence.into())?;
            let conn = target.conn.lock().map_err(|_| Error::Busy)?;
            conn.execute("INSERT INTO migration_records(source_table,source_key,evidence_file_id,status) VALUES('__excluded_entries__',?1,?2,?3) ON CONFLICT(source_table,source_key) DO UPDATE SET evidence_file_id=excluded.evidence_file_id,status=excluded.status",params![name,file.id.0.as_slice(),if resolved {"imported"} else {"unresolved"}])?;
        }
        let mut seen = BTreeSet::new();
        while let Some(directory) = directories.pop() {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::Canceled);
            }
            store::regular_directory(&directory)?;
            for entry in std::fs::read_dir(&directory)? {
                if cancel.load(Ordering::Acquire) {
                    return Err(Error::Canceled);
                }
                let entry = entry?;
                let path = entry.path();
                let metadata = std::fs::symlink_metadata(&path)?;
                if metadata.is_dir() {
                    store::regular_directory(&path)?;
                    directories.push(path);
                    continue;
                }
                store::regular_file(&path)?;
                let key = self.relative_file_key(&path)?;
                let file = target.objects.put_path(&path)?;
                let prepared = target.prepare_file(&file)?;
                let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
                let tx = conn.transaction()?;
                Library::register_file(&tx, &prepared)?;
                // 複数回の走査で内容が変わっても、それぞれの原本を失わない。
                let copy_key = serde_json::to_string(&(&key, file.id.to_string()))?;
                tx.execute("INSERT OR IGNORE INTO migration_records(source_table,source_key,evidence_file_id,status) VALUES('__filesystem_copies__',?1,?2,'imported')",params![copy_key,file.id.0.as_slice()])?;
                let first: Option<Vec<u8>>=tx.query_row("SELECT evidence_file_id FROM migration_records WHERE source_table='__filesystem__' AND source_key=?1",[&key],|r|r.get(0)).optional()?.flatten();
                let used: Option<Vec<u8>>=tx.query_row("SELECT evidence_file_id FROM migration_records WHERE source_table='__used_files__' AND source_key=?1",[&key],|r|r.get(0)).optional()?.flatten();
                let known = used.as_deref() == Some(file.id.0.as_slice())
                    && first.as_ref().is_none_or(|f| f == &file.id.0);
                tx.execute("INSERT INTO migration_records(source_table,source_key,evidence_file_id,status) VALUES('__filesystem__',?1,?2,?3) ON CONFLICT(source_table,source_key) DO UPDATE SET evidence_file_id=coalesce(migration_records.evidence_file_id,excluded.evidence_file_id),status=excluded.status",params![key,file.id.0.as_slice(),if known {"imported"} else {"unresolved"}])?;
                tx.commit()?;
                drop(conn);
                seen.insert(key);
                inventory.files += 1;
                inventory.unresolved += u64::from(!known);
                progress(inventory.files);
            }
        }
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Canceled);
        }
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let missing = {
            let mut stmt=tx.prepare("SELECT source_key FROM migration_records WHERE source_table IN ('__filesystem__','__used_files__') GROUP BY source_key")?;
            let keys = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            keys.into_iter()
                .filter(|key| !seen.contains(key))
                .collect::<Vec<_>>()
        };
        for key in missing {
            tx.execute("INSERT INTO migration_records(source_table,source_key,status) VALUES('__filesystem__',?1,'unresolved') ON CONFLICT(source_table,source_key) DO UPDATE SET status='unresolved'",[key])?;
            inventory.unresolved += 1;
        }
        tx.commit()?;
        Ok(inventory)
    }
}

#[cfg(test)]
#[path = "filesystem_tests.rs"]
mod tests;
