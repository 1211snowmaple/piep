//! サーバーが本棚を借りる先。
//!
//! 内蔵サーバーはデスクトップ版が開いている本棚を借りる。単独サーバーは自分で
//! 開き、送信の確定のときに世代を入れ替える。

use crate::database::Database;
use crate::AppState;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// 入れ替えのとき、読んでいる途中の要求が終わるのを待つ長さ。
const DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

pub enum DbLease {
    App(Arc<AppState>),
    Direct(Arc<Database>),
}

impl Deref for DbLease {
    type Target = Database;

    fn deref(&self) -> &Database {
        match self {
            Self::App(state) => &state.db,
            Self::Direct(db) => db,
        }
    }
}

#[derive(Clone)]
pub enum Library {
    App(Arc<AppState>),
    Swappable(Arc<SwappableDb>),
}

impl Library {
    /// 本棚がまだ無い、または入れ替えの途中なら `None`。
    pub fn lease(&self) -> Option<DbLease> {
        match self {
            Self::App(state) => Some(DbLease::App(Arc::clone(state))),
            Self::Swappable(slot) => slot.current().map(DbLease::Direct),
        }
    }
}

/// 確定のたびに開き直す本棚。
pub struct SwappableDb {
    slot: RwLock<Option<Arc<Database>>>,
    db_path: PathBuf,
    storage_dir: PathBuf,
}

impl SwappableDb {
    pub fn new(db_path: PathBuf, storage_dir: PathBuf) -> Self {
        Self {
            slot: RwLock::new(None),
            db_path,
            storage_dir,
        }
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn current(&self) -> Option<Arc<Database>> {
        self.slot.read().ok().and_then(|slot| slot.clone())
    }

    /// DB があれば開く。まだ一度も送られていなければ何もしない。
    pub fn open_existing(&self) -> Result<bool, String> {
        if !self.db_path.exists() {
            return Ok(false);
        }
        let db = Database::open(&self.db_path, &self.storage_dir)?;
        *self.slot.write().map_err(|_| "本棚の状態を開けません")? = Some(Arc::new(db));
        Ok(true)
    }

    /// 本棚を閉じて `install` を呼び、開き直す。配置か読み直しに失敗したら
    /// `rollback` でファイルを戻してから、元の本棚を開き直す。
    ///
    /// Windows では開いているファイルを置き換えられないので、読んでいる要求が
    /// 終わるのを待ってから閉じる。その間の新しい要求は 503 になる。
    pub fn replace_with<F, R>(&self, install: F, rollback: R) -> Result<(), String>
    where
        F: FnOnce() -> Result<(), String>,
        R: FnOnce() -> Result<(), String>,
    {
        self.replace_with_finalized(install, rollback, |_| Ok(()))
    }

    /// 開けることを確かめた新しい DB を公開する直前に、世代の記録を確定する。
    /// `finalize` の成功後には、失敗し得るファイル操作を行わない。
    pub fn replace_with_finalized<F, R, C>(
        &self,
        install: F,
        rollback: R,
        finalize: C,
    ) -> Result<(), String>
    where
        F: FnOnce() -> Result<(), String>,
        R: FnOnce() -> Result<(), String>,
        C: FnOnce(Option<&Database>) -> Result<(), String>,
    {
        let previous = self
            .slot
            .write()
            .map_err(|_| "本棚の状態を開けません")?
            .take();
        if let Some(previous) = previous {
            let started = Instant::now();
            while Arc::strong_count(&previous) > 1 {
                if started.elapsed() > DRAIN_TIMEOUT {
                    *self.slot.write().map_err(|_| "本棚の状態を開けません")? = Some(previous);
                    return Err("読んでいる要求が終わらないため、本棚を入れ替えられません".into());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            drop(previous);
        }
        crate::database::tantivy_index::release_runtime(&self.storage_dir);
        let result = (|| {
            install()?;
            let candidate = if self.db_path.exists() {
                Some(Arc::new(Database::open(&self.db_path, &self.storage_dir)?))
            } else {
                None
            };
            // 鍵の取得も commit より前に済ませる。公開前の DB をどこにも貸さないので、
            // finalize の失敗時には閉じてから巻き戻せる。
            let mut slot = self.slot.write().map_err(|_| "本棚の状態を開けません")?;
            finalize(candidate.as_deref())?;
            *slot = candidate;
            Ok(())
        })();
        if let Err(error) = result {
            crate::database::tantivy_index::release_runtime(&self.storage_dir);
            rollback()
                .and_then(|()| self.open_existing().map(|_| ()))
                .map_err(|recovery| format!("{error}。元の本棚にも戻せません: {recovery}"))?;
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replacement_that_cannot_be_opened_returns_to_the_previous_database() {
        let root = crate::server::tests::TestRoot::created("reopen_failure");
        let live = root.0.join("piep.db");
        let previous = root.0.join("previous.db");
        let storage = root.0.join("downloads");
        drop(Database::open(&live, &storage).unwrap());
        let slot = SwappableDb::new(live.clone(), storage);
        slot.open_existing().unwrap();
        let result = slot.replace_with(
            || {
                std::fs::rename(&live, &previous).map_err(|error| error.to_string())?;
                std::fs::write(&live, b"not a database").map_err(|error| error.to_string())
            },
            || std::fs::rename(&previous, &live).map_err(|error| error.to_string()),
        );
        assert!(result.is_err());
        assert_eq!(
            slot.current().unwrap().get_stats().unwrap().total_downloads,
            0
        );
        assert!(std::fs::read(&live)
            .unwrap()
            .starts_with(b"SQLite format 3"));
    }
}
