//! デスクトップ版から新しい棚の写しを受け取り、次の起動で配る棚を入れ替える（単独サーバー）。
//!
//! 送り手は台帳（DB）の写しと、まだ持っていない実体だけを送る。台帳も 4 MiB ずつの部分に分けて
//! 実体と同じ口で送り、前に受け取った台帳と同じ部分は送らない（写しは頁の並びを保つので、
//! 少し変えただけなら変わった部分だけになる）。受け取りは一度に一つで、途中で
//! 切れても同じ棚の受け取りなら続きから受ける。揃ったら、受け取った実体といま配っている棚の
//! 実体（ハードリンクで共有）から検証済みの棚を別の世代に組み、次の起動でその世代を選ぶ
//! （見張り役のいるサーバーは自分で立ち上げ直す）。起動のときは、送ったあとに携帯が前の世代に
//! 書いた読書位置を新しい世代へ写してから配る。
//!
//! 携帯が棚を読む許可（`library_v2.readers`）は、管理者が端末ごとに付け外しする。

use super::native::{activate_locked, generation_path, generations, write_config};
use super::{load_config, ServerConfig, ServerPaths};
use crate::library::{
    protocol::Id, ContentFile, FileChunk, Fingerprint, Library, LibraryId, ObjectStore,
};
use piep_storage::StoredEntry;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use toml_edit::{value, DocumentMut, Item, Table};

const INCOMING: &str = "library-v2-incoming";
/// 前に受け取った台帳の部分（次の受け取りで同じ部分を送らずに済ませる）。
const DATABASE_PARTS: &str = "library-v2-database-parts";
const CURRENT: &str = "current.json";
const PENDING: &str = "library-v2-pending.json";
const DATABASE: &str = "library.db";
/// 一度に問い合わせる実体の数の上限。
pub const MAX_QUERY_CHUNKS: usize = 4096;
/// 台帳の写しの大きさの上限。
const MAX_DATABASE_BYTES: u64 = 64 * 1024 * 1024 * 1024;
/// 組み終えてから立ち上げ直すまでの間。
const RESTART_DELAY: std::time::Duration = std::time::Duration::from_secs(3);

/// 受け取りの失敗。HTTP の応答へは呼び手が写す。
#[derive(Debug)]
pub enum ShipError {
    Invalid(&'static str),
    Conflict(&'static str),
    NotFound,
    Internal(String),
}

impl From<crate::library::Error> for ShipError {
    fn from(error: crate::library::Error) -> Self {
        Self::Internal(error.to_string())
    }
}
impl From<piep_storage::Error> for ShipError {
    fn from(error: piep_storage::Error) -> Self {
        Self::Internal(error.to_string())
    }
}
impl From<std::io::Error> for ShipError {
    fn from(error: std::io::Error) -> Self {
        Self::Internal(error.to_string())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Current {
    id: String,
    library_id: LibraryId,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pending {
    generation: Id,
}

/// 受け取りを始めた（続けた）ときの答え。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Opened {
    pub shipment_id: String,
    pub resumed: bool,
}

/// 受け取りの進み具合。`state` は `idle`・`receiving`・`assembling`・`ready`・`failed`。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShipmentStatus {
    pub state: String,
    pub shipment_id: Option<String>,
    pub generation: Option<Id>,
    pub works: Option<u64>,
    /// 組み終えて、立ち上げ直しを頼んだか（偽なら手で立ち上げ直すと切り替わる）。
    pub restarting: Option<bool>,
    pub error: Option<String>,
}

/// 問い合わせる実体（識別子は 16 進）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireChunk {
    pub id: String,
    pub length: u64,
}

impl WireChunk {
    pub fn parse(&self) -> Result<FileChunk, ShipError> {
        let id = Fingerprint::parse(&self.id)
            .map_err(|_| ShipError::Invalid("実体の識別子が正しくありません"))?;
        if id.to_string() != self.id
            || self.length == 0
            || self.length > piep_storage::MAX_CHUNK_ORIGINAL_BYTES
        {
            return Err(ShipError::Invalid("実体の識別子が正しくありません"));
        }
        Ok(FileChunk {
            id,
            length: self.length,
        })
    }
}

type Restart = Arc<dyn Fn() + Send + Sync>;

pub struct ShelfShipments {
    paths: ServerPaths,
    /// いま配っている棚（実体を共有し、送らずに済ませる）。
    served: Option<Arc<Library>>,
    /// 始める・台帳を書く・確定を一列に並べる。実体の受け取りは並べない。
    write: Mutex<()>,
    status: Arc<Mutex<ShipmentStatus>>,
    restart: OnceLock<Restart>,
}

fn incoming(paths: &ServerPaths) -> PathBuf {
    paths.root.join(INCOMING)
}

fn database_parts(paths: &ServerPaths) -> Result<ObjectStore, ShipError> {
    Ok(ObjectStore::open(&paths.root.join(DATABASE_PARTS))?)
}

fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|parsed| parsed.to_string() == id)
}

impl ShelfShipments {
    pub fn new(paths: ServerPaths, served: Option<Arc<Library>>) -> Self {
        // いま配っている世代も返す。送り手は、組み終えた知らせを立ち上げ直しで聞き逃しても、
        // 世代が変わったことで届いたと分かる。
        let generation = load_config(&paths)
            .ok()
            .and_then(|config| config.library_v2)
            .and_then(|settings| settings.generation);
        let status = match read_current(&paths) {
            Some(current) => ShipmentStatus {
                state: "receiving".into(),
                shipment_id: Some(current.id),
                generation,
                ..Default::default()
            },
            None => ShipmentStatus {
                state: "idle".into(),
                generation,
                ..Default::default()
            },
        };
        Self {
            paths,
            served,
            write: Mutex::new(()),
            status: Arc::new(Mutex::new(status)),
            restart: OnceLock::new(),
        }
    }

    /// 組み終えたあとに立ち上げ直す口（見張り役のいるサーバーだけ）。
    pub fn set_restart(&self, restart: Restart) {
        let _ = self.restart.set(restart);
    }

    /// 組み終えた棚が、次の起動で配られるのを待っているか。
    pub fn pending(&self) -> bool {
        self.paths.root.join(PENDING).is_file()
    }

    pub fn status(&self) -> ShipmentStatus {
        self.status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default()
    }

    fn assembling(&self) -> bool {
        self.status().state == "assembling"
    }

    fn dir(&self, id: &str) -> PathBuf {
        incoming(&self.paths).join(id)
    }

    fn received(&self, id: &str) -> Result<ObjectStore, ShipError> {
        Ok(ObjectStore::open(&self.dir(id).join("objects"))?)
    }

    fn require(&self, id: &str) -> Result<Current, ShipError> {
        match read_current(&self.paths) {
            Some(current) if current.id == id && valid_id(id) => Ok(current),
            _ => Err(ShipError::NotFound),
        }
    }

    /// 受け取りを始める。同じ棚の受け取りが残っていれば続ける。別の棚を配っていれば断る。
    pub fn open(&self, library_id: LibraryId) -> Result<Opened, ShipError> {
        let _write = self
            .write
            .lock()
            .map_err(|_| ShipError::Conflict("受け取りの途中です"))?;
        if self.assembling() {
            return Err(ShipError::Conflict("前に送られた棚を組んでいます"));
        }
        let config = load_config(&self.paths).map_err(ShipError::Internal)?;
        if config
            .library_v2
            .as_ref()
            .is_some_and(|settings| settings.library_id.get() != library_id)
        {
            return Err(ShipError::Conflict("このサーバーは別の棚を配っています"));
        }
        if let Some(current) = read_current(&self.paths) {
            if current.library_id == library_id && self.dir(&current.id).is_dir() {
                self.set_status(ShipmentStatus {
                    state: "receiving".into(),
                    shipment_id: Some(current.id.clone()),
                    ..Default::default()
                });
                return Ok(Opened {
                    shipment_id: current.id,
                    resumed: true,
                });
            }
        }
        let root = incoming(&self.paths);
        if root.exists() {
            fs::remove_dir_all(&root)?;
        }
        let id = uuid::Uuid::now_v7().to_string();
        fs::create_dir_all(self.dir(&id).join("objects"))?;
        write_atomic(
            &root.join(CURRENT),
            &serde_json::to_vec(&Current {
                id: id.clone(),
                library_id,
            })
            .map_err(|error| ShipError::Internal(error.to_string()))?,
        )?;
        self.set_status(ShipmentStatus {
            state: "receiving".into(),
            shipment_id: Some(id.clone()),
            ..Default::default()
        });
        Ok(Opened {
            shipment_id: id,
            resumed: false,
        })
    }

    /// 送ってほしい実体（受け取り済みでも、配っている棚にも無いもの）。
    pub fn needed(&self, id: &str, chunks: &[WireChunk]) -> Result<Vec<String>, ShipError> {
        self.require(id)?;
        if chunks.len() > MAX_QUERY_CHUNKS {
            return Err(ShipError::Invalid("一度に問い合わせる実体が多すぎます"));
        }
        let received = self.received(id)?;
        let parts = database_parts(&self.paths)?;
        let mut needed = Vec::new();
        for wire in chunks {
            let chunk = wire.parse()?;
            let served = match &self.served {
                Some(library) => library.objects.has_object(chunk.id)?,
                None => false,
            };
            if !served && !received.has_object(chunk.id)? && !parts.has_object(chunk.id)? {
                needed.push(wire.id.clone());
            }
        }
        Ok(needed)
    }

    /// 実体を一つ受け取る（圧縮したまま）。中身は受け取るときに確かめる。
    pub fn put_chunk(&self, id: &str, wire: &WireChunk, bytes: &[u8]) -> Result<(), ShipError> {
        self.require(id)?;
        let chunk = wire.parse()?;
        self.received(id)?
            .import_chunk(&chunk, std::io::Cursor::new(bytes))
            .map_err(|error| match error {
                piep_storage::Error::Integrity(_) => {
                    ShipError::Invalid("受け取った実体が識別子と一致しません")
                }
                other => ShipError::Internal(other.to_string()),
            })
    }

    /// 受け取りを確定し、別のスレッドで棚を組む。進み具合は `status` で見る。
    pub fn commit(&self, id: &str, database: &ContentFile) -> Result<ShipmentStatus, ShipError> {
        let _write = self
            .write
            .lock()
            .map_err(|_| ShipError::Conflict("受け取りの途中です"))?;
        let current = self.require(id)?;
        if self.assembling() {
            return Err(ShipError::Conflict("前に送られた棚を組んでいます"));
        }
        if database.length > MAX_DATABASE_BYTES || database.validate_layout().is_err() {
            return Err(ShipError::Invalid("台帳の部分の並びが正しくありません"));
        }
        // 台帳の部分を、次の受け取りでも使えるよう台帳の部分の置き場へ移してから組み立てる。
        let context = |step: &'static str| {
            move |error: piep_storage::Error| ShipError::Internal(format!("{step}: {error}"))
        };
        let parts = database_parts(&self.paths)?;
        let received = self.received(id)?;
        for chunk in &database.chunks {
            if !parts
                .has_object(chunk.id)
                .map_err(context("台帳の部分を探せません"))?
            {
                if !received
                    .has_object(chunk.id)
                    .map_err(context("受け取った部分を探せません"))?
                {
                    return Err(ShipError::Invalid("台帳の部分が届いていません"));
                }
                parts
                    .adopt_chunk(&received, chunk)
                    .map_err(context("台帳の部分を移せません"))?;
            }
        }
        let database_path = self.dir(id).join(DATABASE);
        if database_path.try_exists()? {
            fs::remove_file(&database_path)?;
        }
        parts
            .export_new(database, &database_path)
            .map_err(|error| match error {
                piep_storage::Error::Integrity(_) => {
                    ShipError::Invalid("受け取った台帳がハッシュと一致しません")
                }
                other => ShipError::Internal(format!("台帳を組み直せません: {other}")),
            })?;
        let keep: HashSet<Fingerprint> = database.chunks.iter().map(|chunk| chunk.id).collect();
        let database = database_path;
        let assembling = ShipmentStatus {
            state: "assembling".into(),
            shipment_id: Some(id.to_owned()),
            ..Default::default()
        };
        self.set_status(assembling.clone());
        let paths = self.paths.clone();
        let served = self.served.clone();
        let status = Arc::clone(&self.status);
        let restart_cb = self.restart.get().cloned();
        let restart = restart_cb.is_some();
        let shipment = id.to_owned();
        let run = move || -> Result<(Id, u64), String> {
            let parent = generations(&paths);
            fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
            let generation = Id::new(uuid::Uuid::now_v7()).map_err(|error| error.to_string())?;
            let destination = generation_path(&paths, generation)?;
            let received = ObjectStore::open(&incoming(&paths).join(&shipment).join("objects"))
                .map_err(|error| error.to_string())?;
            let works = Library::assemble_shipment(
                &destination,
                &database,
                &received,
                served.as_deref(),
                current.library_id,
            )
            .map_err(|error| error.to_string())?
            .works;
            write_atomic(
                &paths.root.join(PENDING),
                &serde_json::to_vec(&Pending { generation }).map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
            // 受け取ったものは組んだ棚へ共有した。置き場ごと片付ける。
            let _ = fs::remove_dir_all(incoming(&paths));
            // 台帳の部分は、いま組んだ台帳のものだけを残す。
            if let Err(error) = prune_parts(&paths, &keep) {
                log::warn!("前の台帳の部分を片付けられません: {error:?}");
            }
            Ok((generation, works))
        };
        let this_status = status;
        std::thread::Builder::new()
            .name("piep-shelf-assembly".into())
            .spawn(move || {
                let result = run();
                let next = match result {
                    Ok((generation, works)) => {
                        log::info!("送られた棚を組みました（{works} 作品）。次の起動で配ります");
                        ShipmentStatus {
                            state: "ready".into(),
                            shipment_id: None,
                            generation: Some(generation),
                            works: Some(works),
                            restarting: Some(restart),
                            error: None,
                        }
                    }
                    Err(error) => {
                        log::error!("送られた棚を組めません: {error}");
                        ShipmentStatus {
                            state: "failed".into(),
                            shipment_id: None,
                            error: Some(error),
                            ..Default::default()
                        }
                    }
                };
                let ready = next.state == "ready";
                if let Ok(mut status) = this_status.lock() {
                    *status = next;
                }
                if ready {
                    if let Some(restart) = restart_cb {
                        // 送り手が組み終えた知らせを読めるよう、少し待ってから立ち上げ直す。
                        std::thread::sleep(RESTART_DELAY);
                        restart();
                    }
                }
            })
            .map_err(|error| ShipError::Internal(error.to_string()))?;
        Ok(assembling)
    }

    /// 受け取りをやめて、受け取ったものを捨てる。
    pub fn abort(&self, id: &str) -> Result<(), ShipError> {
        let _write = self
            .write
            .lock()
            .map_err(|_| ShipError::Conflict("受け取りの途中です"))?;
        self.require(id)?;
        if self.assembling() {
            return Err(ShipError::Conflict("前に送られた棚を組んでいます"));
        }
        fs::remove_dir_all(incoming(&self.paths))?;
        self.set_status(ShipmentStatus {
            state: "idle".into(),
            ..Default::default()
        });
        Ok(())
    }

    fn set_status(&self, next: ShipmentStatus) {
        if let Ok(mut status) = self.status.lock() {
            *status = next;
        }
    }
}

fn read_current(paths: &ServerPaths) -> Option<Current> {
    let bytes = fs::read(incoming(paths).join(CURRENT)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_atomic(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".{}.part", uuid::Uuid::now_v7()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    let written = file.write_all(content).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = written.and_then(|()| fs::rename(&temporary, path)) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

/// 台帳の部分の置き場は受け取りだけが使う（棚の実体ではない）。隔離を挟まずに消してよい。
fn prune_parts(paths: &ServerPaths, keep: &HashSet<Fingerprint>) -> Result<(), ShipError> {
    let parts = database_parts(paths)?;
    for listing in parts.listing()? {
        let stale = match listing.entry {
            StoredEntry::Object(id) => !keep.contains(&id),
            StoredEntry::Part | StoredEntry::Damaged => true,
            StoredEntry::Quarantined { .. } => false,
        };
        if stale {
            fs::remove_file(&listing.path)?;
        }
    }
    Ok(())
}

/// 起動のとき（置き場の錠を持ったまま、棚を開く前に）呼ぶ。組み終えた世代があれば配る棚に
/// 選び、送ったあとに携帯が前の世代へ書いた読書位置を写し、古い世代を片付ける。選べなければ
/// 前の棚のまま起動する。
pub fn apply_pending(paths: &ServerPaths) {
    let pending_path = paths.root.join(PENDING);
    let Ok(bytes) = fs::read(&pending_path) else {
        return;
    };
    let result = (|| -> Result<(), String> {
        let pending: Pending = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        let before = load_config(paths)?.library_v2;
        let previous = match &before {
            Some(settings) => Some(match settings.generation {
                Some(generation) => generation_path(paths, generation)?,
                None => paths.native_library(),
            }),
            None => None,
        };
        activate_locked(paths, pending.generation, false)?;
        let next = generation_path(paths, pending.generation)?;
        // ここからは切り替えたあと。読書位置を写せなくても新しい棚で起動する（前の世代は残る）。
        if let (Some(previous), Some(settings)) = (&previous, &before) {
            if previous != &next && previous.is_dir() {
                let moved = (|| -> Result<(usize, usize), String> {
                    let old = Library::open_matching(previous, settings.library_id.get())
                        .map_err(|error| format!("前の棚を開けません: {error}"))?;
                    let new = Library::open_matching(&next, settings.library_id.get())
                        .map_err(|error| format!("新しい棚を開けません: {error}"))?;
                    new.adopt_from_previous(&old)
                        .map_err(|error| format!("読書位置とお気に入りを写せません: {error}"))
                })();
                match moved {
                    Ok((positions, favorites)) => log::info!(
                        "前の棚から読書位置を {positions} 件、お気に入りを {favorites} 件写しました"
                    ),
                    Err(error) => log::error!(
                        "送られた棚に切り替えましたが、前の棚の読書位置を写せませんでした: {error}"
                    ),
                }
            }
        }
        prune_generations(paths, pending.generation, previous.as_deref());
        Ok(())
    })();
    if let Err(error) = result {
        log::error!("送られた棚に切り替えられません。前の棚のまま起動します: {error}");
    }
    // 成否によらず一度きり（失敗した世代を起動のたびに試さない）。
    let _ = fs::remove_file(pending_path);
}

/// 今の世代と一つ前の世代だけを残す（実体はハードリンクで共有しているので、消えるのは
/// どちらにも使われていない実体と台帳だけ）。
fn prune_generations(paths: &ServerPaths, current: Id, previous: Option<&Path>) {
    let Ok(entries) = fs::read_dir(generations(paths)) else {
        return;
    };
    let current = current.get().to_string();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == current || previous.is_some_and(|previous| previous == path) {
            continue;
        }
        if let Err(error) = fs::remove_dir_all(&path) {
            log::warn!("古い棚の世代を消せません（{name}）: {error}");
        }
    }
}

/// 端末に棚を読む許可を付け外しする（設定の `library_v2.readers` に書く）。読む利用者は棚の
/// 持ち主（デスクトップと同じ利用者）。
pub fn set_reader(paths: &ServerPaths, device: &str, read: bool) -> Result<Id, String> {
    static EDIT: Mutex<()> = Mutex::new(());
    let _edit = EDIT.lock().map_err(|_| "設定を書き換えられません")?;
    let path = paths.config();
    let text = fs::read_to_string(&path).map_err(|error| error.to_string())?;
    let config: ServerConfig = toml::from_str(&text).map_err(|error| error.to_string())?;
    let owner = config
        .library_v2
        .as_ref()
        .map(|settings| settings.library_id)
        .ok_or("このサーバーはまだ新しい棚を配っていません")?;
    let mut document: DocumentMut = text
        .parse()
        .map_err(|error: toml_edit::TomlError| error.to_string())?;
    let settings = document
        .entry("library_v2")
        .or_insert(Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or("棚の配信設定が不正です")?;
    let readers = settings
        .entry("readers")
        .or_insert(Item::Table(Table::new()))
        .as_table_like_mut()
        .ok_or("棚の配信設定が不正です")?;
    if read {
        readers.insert(device, value(owner.get().to_string()));
    } else {
        readers.remove(device);
    }
    let written = document.to_string();
    let next: ServerConfig = toml::from_str(&written).map_err(|error| error.to_string())?;
    next.validate()?;
    write_config(paths, written.as_bytes())?;
    Ok(owner)
}

#[cfg(test)]
#[path = "standalone_shipment_tests.rs"]
mod tests;
