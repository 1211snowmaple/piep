//! デスクトップ版から棚の写しを受け取り、次の起動で配る棚を入れ替える（単独サーバー）。
//!
//! 送り手は台帳（DB）の写しと、まだ持っていない実体だけを送る。台帳も 4 MiB ずつの部分に分けて
//! 実体と同じ口で送り、前に受け取った台帳と同じ部分は送らない（写しは頁の並びを保つので、
//! 少し変えただけなら変わった部分だけになる）。受け取りは一度に一つで、途中で
//! 切れても同じ棚の受け取りなら続きから受ける。揃ったら、受け取った実体といま配っている棚の
//! 実体（ハードリンクで共有）から検証済みの棚を別の世代に組み、次の起動でその世代を選ぶ
//! （見張り役のいるサーバーは自分で立ち上げ直す）。起動のときは、送ったあとに携帯が前の世代に
//! 書いた読書位置とお気に入りを新しい世代へ写してから配る。
//!
//! 携帯が棚を読む許可（`shelf.readers`）は、管理者が端末ごとに付け外しする。

use super::generations::{activate_locked, generation_path, generations};
use super::{edit_config, load_config, ReaderShelfConfig, ServerConfig, ServerPaths};
use crate::library::{
    protocol::Id, ContentFile, FileChunk, Fingerprint, Library, LibraryId, ObjectStore,
};
use crate::server::durable::write_atomic;
use piep_storage::StoredEntry;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use toml_edit::{value, DocumentMut, Item, Table};

const INCOMING: &str = "shelf-incoming";
/// 前に受け取った台帳の部分（次の受け取りで同じ部分を送らずに済ませる）。
const DATABASE_PARTS: &str = "shelf-database-parts";
const CURRENT: &str = "current.json";
const PENDING: &str = "shelf-pending.json";
const RESULTS: &str = "shelf-shipment-results";
const DATABASE: &str = "library.db";
/// 一度に問い合わせる実体の数の上限。
pub const MAX_QUERY_CHUNKS: usize = 4096;
/// 台帳の写しの大きさの上限。
const MAX_DATABASE_BYTES: u64 = 64 * 1024 * 1024 * 1024;

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
    shipment_id: String,
    library_id: LibraryId,
    previous: ShelfSource,
    next_generation: Id,
    works: u64,
}

/// 同じ棚の ID を持つ写しどうしを、配信する世代で区別する。
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum ShelfSource {
    Absent,
    Local,
    Generation(Id),
}

impl ShelfSource {
    fn from_config(shelf: Option<&ReaderShelfConfig>) -> Self {
        match shelf.map(|settings| settings.generation) {
            None => Self::Absent,
            Some(None) => Self::Local,
            Some(Some(generation)) => Self::Generation(generation),
        }
    }

    fn root(&self, paths: &ServerPaths) -> Result<Option<PathBuf>, String> {
        match *self {
            Self::Absent => Ok(None),
            Self::Local => Ok(Some(paths.shelf())),
            Self::Generation(generation) => generation_path(paths, generation).map(Some),
        }
    }
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
    /// このサーバーが扱える棚の台帳の版。送り手はこれより新しい棚を送らない（組み立てで
    /// 断られ、送った分が無駄になる）。
    #[serde(default)]
    pub schema: Option<i64>,
    #[serde(default)]
    pub publication_by_id: bool,
}

/// 送信 ID ごとの公開の状態。確定した結果は棚の世代を片付けても残る。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ShipmentResult {
    pub shipment_id: String,
    #[serde(flatten)]
    pub state: PublicationState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PublicationState {
    Receiving,
    Assembling,
    Pending { restarting: bool },
    Published(PublishedShelf),
    Rejected { reason: Rejection },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublishedShelf {
    pub sync_epoch: Id,
    pub works: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Rejection {
    AssemblyFailed,
    Interrupted,
    Superseded,
    Canceled,
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
        // 空のファイルは長さ 0 の実体を一つ持つ（保存の決まり）ので、0 は断らない。
        if id.to_string() != self.id || self.length > piep_storage::MAX_CHUNK_ORIGINAL_BYTES {
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
    write: Arc<Mutex<()>>,
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
        let generation = load_config(&paths)
            .ok()
            .and_then(|config| config.shelf)
            .and_then(|settings| settings.generation);
        let current = read_current(&paths).filter(|current| {
            read_result(&paths, &current.id).is_ok_and(|record| record.is_none())
        });
        let status = match (read_pending(&paths).ok().flatten(), current) {
            (Some(pending), _) => ShipmentStatus {
                state: "ready".into(),
                shipment_id: Some(pending.shipment_id),
                generation: Some(pending.next_generation),
                works: Some(pending.works),
                restarting: Some(false),
                ..Default::default()
            },
            (None, Some(current)) => ShipmentStatus {
                state: "receiving".into(),
                shipment_id: Some(current.id),
                generation,
                ..Default::default()
            },
            (None, None) => ShipmentStatus {
                state: "idle".into(),
                generation,
                ..Default::default()
            },
        };
        Self {
            paths,
            served,
            write: Arc::new(Mutex::new(())),
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
        let mut status = self
            .status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default();
        status.schema = Some(crate::library::schema::VERSION);
        status.publication_by_id = true;
        status
    }

    pub fn result(&self, id: &str) -> Result<ShipmentResult, ShipError> {
        if !valid_id(id) {
            return Err(ShipError::NotFound);
        }
        let _write = self
            .write
            .lock()
            .map_err(|_| ShipError::Conflict("受け取りの途中です"))?;
        let record = read_result(&self.paths, id)?;
        let status = self.status();
        let state = match record {
            Some(state @ (PublicationState::Published(_) | PublicationState::Rejected { .. })) => {
                state
            }
            _ if read_pending(&self.paths)
                .map_err(ShipError::Internal)?
                .is_some_and(|pending| pending.shipment_id == id) =>
            {
                PublicationState::Pending {
                    restarting: status.shipment_id.as_deref() == Some(id)
                        && status.restarting == Some(true),
                }
            }
            Some(PublicationState::Assembling)
                if status.shipment_id.as_deref() == Some(id) && status.state == "assembling" =>
            {
                PublicationState::Assembling
            }
            Some(PublicationState::Assembling) => {
                // 起動時の復旧を完了できなかった。別の写しにこの番号を渡さない。
                PublicationState::Rejected {
                    reason: Rejection::Interrupted,
                }
            }
            None if read_current(&self.paths).is_some_and(|current| current.id == id) => {
                PublicationState::Receiving
            }
            _ => return Err(ShipError::NotFound),
        };
        Ok(ShipmentResult {
            shipment_id: id.into(),
            state,
        })
    }

    /// 受け取りか組み立ての途中か。自己更新はこれが済むのを待つ。
    pub fn busy(&self) -> bool {
        matches!(self.status().state.as_str(), "receiving" | "assembling")
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
            Some(current)
                if current.id == id && valid_id(id) && read_result(&self.paths, id)?.is_none() =>
            {
                Ok(current)
            }
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
        if read_pending(&self.paths)
            .map_err(ShipError::Internal)?
            .is_some()
        {
            return Err(ShipError::Conflict(
                "前に送られた棚の公開を待っています。サーバーの状態を確認してください",
            ));
        }
        let config = load_config(&self.paths).map_err(ShipError::Internal)?;
        if config
            .shelf
            .as_ref()
            .is_some_and(|settings| settings.library_id.get() != library_id)
        {
            return Err(ShipError::Conflict("このサーバーは別の棚を配っています"));
        }
        if let Some(current) = read_current(&self.paths) {
            let record = read_result(&self.paths, &current.id)?;
            if record.is_none()
                && current.library_id == library_id
                && self.dir(&current.id).is_dir()
            {
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
            match record {
                None => reject(&self.paths, &current.id, Rejection::Superseded)?,
                Some(PublicationState::Assembling) => {
                    reject(&self.paths, &current.id, Rejection::Interrupted)?;
                }
                _ => {}
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
        )
        .map_err(ShipError::Internal)?;
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
        if self.assembling() {
            return Err(ShipError::Conflict("前に送られた棚を組んでいます"));
        }
        if self.pending() {
            return Err(ShipError::Conflict("前に送られた棚の公開を待っています"));
        }
        let current = self.require(id)?;
        if database.length > MAX_DATABASE_BYTES || database.validate_layout().is_err() {
            return Err(ShipError::Invalid("台帳の部分の並びが正しくありません"));
        }
        let config = load_config(&self.paths).map_err(ShipError::Internal)?;
        let previous = ShelfSource::from_config(config.shelf.as_ref());
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
        // 確定を受けた ID は別の写しに使い回さない。
        write_result(&self.paths, id, &PublicationState::Assembling)?;
        self.set_status(assembling.clone());
        let paths = self.paths.clone();
        let served = self.served.clone();
        let status = Arc::clone(&self.status);
        let write = Arc::clone(&self.write);
        let restart_cb = self.restart.get().cloned();
        let restart = restart_cb.is_some();
        let shipment = id.to_owned();
        let assemble_paths = paths.clone();
        let assemble_id = shipment.clone();
        let run = move || -> Result<Pending, String> {
            let paths = &assemble_paths;
            let parent = generations(paths);
            fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
            let generation = Id::new(uuid::Uuid::now_v7()).map_err(|error| error.to_string())?;
            let destination = generation_path(paths, generation)?;
            let received = ObjectStore::open(&incoming(paths).join(&assemble_id).join("objects"))
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
            Ok(Pending {
                shipment_id: assemble_id,
                library_id: current.library_id,
                previous,
                next_generation: generation,
                works,
            })
        };
        if let Err(error) = std::thread::Builder::new()
            .name("piep-shelf-assembly".into())
            .spawn(move || {
                let result = run();
                let Ok(_write) = write.lock() else {
                    return;
                };
                let result = result.and_then(|pending| {
                    let bytes = serde_json::to_vec(&pending).map_err(|error| error.to_string())?;
                    write_atomic(&paths.root.join(PENDING), &bytes)?;
                    // 予約が残ることを確かめてから受信の置き場を片付ける。
                    let _ = fs::remove_dir_all(incoming(&paths));
                    if let Err(error) = prune_parts(&paths, &keep) {
                        log::warn!("前の台帳の部分を片付けられません: {error:?}");
                    }
                    Ok(pending)
                });
                let next = match result {
                    Ok(Pending {
                        next_generation: generation,
                        works,
                        ..
                    }) => {
                        log::info!("送られた棚を組みました（{works} 作品）。次の起動で配ります");
                        ShipmentStatus {
                            state: "ready".into(),
                            shipment_id: Some(shipment),
                            generation: Some(generation),
                            works: Some(works),
                            restarting: Some(restart),
                            ..Default::default()
                        }
                    }
                    Err(error) => {
                        log::error!("送られた棚を組めません: {error}");
                        if let Err(error) = reject(&paths, &shipment, Rejection::AssemblyFailed) {
                            log::error!("送られた棚の結果を記録できません: {error:?}");
                        }
                        ShipmentStatus {
                            state: "failed".into(),
                            shipment_id: Some(shipment),
                            error: Some("サーバーで棚を組めませんでした".into()),
                            ..Default::default()
                        }
                    }
                };
                let ready = next.state == "ready";
                if let Ok(mut status) = status.lock() {
                    *status = next;
                }
                drop(_write);
                if ready {
                    if let Some(restart) = restart_cb {
                        restart();
                    }
                }
            })
        {
            self.set_status(ShipmentStatus {
                state: "failed".into(),
                shipment_id: Some(id.into()),
                ..Default::default()
            });
            reject(&self.paths, id, Rejection::AssemblyFailed)?;
            return Err(ShipError::Internal(error.to_string()));
        }
        Ok(assembling)
    }

    /// 受け取りをやめて、受け取ったものを捨てる。
    pub fn abort(&self, id: &str) -> Result<(), ShipError> {
        let _write = self
            .write
            .lock()
            .map_err(|_| ShipError::Conflict("受け取りの途中です"))?;
        if self.assembling() {
            return Err(ShipError::Conflict("前に送られた棚を組んでいます"));
        }
        self.require(id)?;
        reject(&self.paths, id, Rejection::Canceled)?;
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
    let current: Current = serde_json::from_slice(&bytes).ok()?;
    valid_id(&current.id).then_some(current)
}

fn result_path(paths: &ServerPaths, id: &str) -> PathBuf {
    paths.root.join(RESULTS).join(format!("{id}.json"))
}

fn read_result(paths: &ServerPaths, id: &str) -> Result<Option<PublicationState>, ShipError> {
    match fs::read(result_path(paths, id)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| ShipError::Internal(error.to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn write_result(paths: &ServerPaths, id: &str, state: &PublicationState) -> Result<(), ShipError> {
    let path = result_path(paths, id);
    let bytes =
        serde_json::to_vec(state).map_err(|error| ShipError::Internal(error.to_string()))?;
    fs::create_dir_all(paths.root.join(RESULTS))?;
    write_atomic(&path, &bytes).map_err(ShipError::Internal)?;
    piep_storage::sync_directory(&paths.root)?;
    Ok(())
}

fn sync_results(paths: &ServerPaths) -> Result<(), ShipError> {
    piep_storage::sync_directory(&paths.root.join(RESULTS))?;
    piep_storage::sync_directory(&paths.root)?;
    Ok(())
}

fn reject(paths: &ServerPaths, id: &str, reason: Rejection) -> Result<(), ShipError> {
    write_result(paths, id, &PublicationState::Rejected { reason })
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

/// 起動のとき、置き場の錠を持ったまま呼ぶ。引き継ぎを済ませてから棚と同期の世代を切り替える。
pub fn apply_pending(paths: &ServerPaths) {
    record_outcome(paths, apply_pending_with(paths, |_| {}));
}

/// 起動時の切り替えの結果を、操作の記録に残す。
fn record_outcome(paths: &ServerPaths, outcome: Result<Option<Publication>, String>) {
    let store = super::super::store::ServerStore::open(&paths.store());
    let audit = |action: &str, ok: bool, detail: &str| match &store {
        Ok(store) => store.audit("library", action, "サーバー", None, ok, Some(detail)),
        Err(error) => log::warn!("棚の公開を記録できません: {error}"),
    };
    match outcome {
        Ok(None) => {}
        Ok(Some(Publication::Published { shipment_id, works })) => {
            audit(
                "棚を公開",
                true,
                &format!("送信 ID {shipment_id}・{works} 作品"),
            );
        }
        Ok(Some(Publication::Rejected {
            shipment_id,
            reason,
        })) => {
            audit(
                "棚の公開を拒否",
                false,
                &format!("送信 ID {shipment_id}・{}", reason.text()),
            );
        }
        Err(error) => {
            log::error!("送られた棚の切り替え処理を完了できません。予約を残します: {error}");
            // 中身のエラーにはパスが入りうるので、記録には決まった文だけを残す。
            audit(
                "棚の公開に失敗",
                false,
                "棚の切り替え処理を完了できませんでした。次の起動で再試行します",
            );
        }
    }
}

/// 起動時の切り替えの結果。前に記録した結果をもう一度片付けただけなら返さない。
#[derive(Debug, PartialEq, Eq)]
enum Publication {
    Published {
        shipment_id: String,
        works: u64,
    },
    Rejected {
        shipment_id: String,
        reason: Rejection,
    },
}

impl Rejection {
    /// 記録に載せる短い理由。
    fn text(self) -> &'static str {
        match self {
            Rejection::AssemblyFailed => "棚を組めませんでした",
            Rejection::Interrupted => "受け取りの途中で止まりました",
            Rejection::Superseded => "引き継ぎ元の棚が変わりました",
            Rejection::Canceled => "受け取りを中止しました",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SwapStage {
    BeforeAdoption,
    AfterAdoption,
    AfterActivation,
}

fn read_pending(paths: &ServerPaths) -> Result<Option<Pending>, String> {
    match fs::read(paths.root.join(PENDING)) {
        Ok(bytes) => {
            let pending: Pending = serde_json::from_slice(&bytes)
                .map_err(|error| format!("棚の切り替えの予約を読めません: {error}"))?;
            if !valid_id(&pending.shipment_id) {
                return Err("棚の切り替えの予約にある送信 ID が正しくありません".into());
            }
            Ok(Some(pending))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("棚の切り替えの予約を読めません: {error}")),
    }
}

/// 予約した世代は、携帯の変更を引き継ぐ起動時の処理で選ぶ。
pub(super) fn check_manual_activation(paths: &ServerPaths, generation: Id) -> Result<(), String> {
    if read_pending(paths)?.is_some_and(|pending| pending.next_generation == generation) {
        return Err(
            "受け取った棚の切り替えが予約されています。サーバーを起動して切り替えてください".into(),
        );
    }
    Ok(())
}

fn apply_pending_with(
    paths: &ServerPaths,
    mut reached: impl FnMut(SwapStage),
) -> Result<Option<Publication>, String> {
    let result_error = |error| format!("棚の公開結果を記録できません: {error:?}");
    let Some(pending) = read_pending(paths)? else {
        if let Some(current) = read_current(paths) {
            if read_result(paths, &current.id).map_err(result_error)?
                == Some(PublicationState::Assembling)
            {
                reject(paths, &current.id, Rejection::Interrupted).map_err(result_error)?;
                return Ok(Some(Publication::Rejected {
                    shipment_id: current.id,
                    reason: Rejection::Interrupted,
                }));
            }
        }
        return Ok(None);
    };
    let config = load_config(paths)?;
    let current = ShelfSource::from_config(config.shelf.as_ref());
    let target = ShelfSource::Generation(pending.next_generation);
    let record = read_result(paths, &pending.shipment_id).map_err(result_error)?;
    if matches!(record, Some(PublicationState::Rejected { .. })) {
        sync_results(paths).map_err(result_error)?;
        return clear_pending(paths).map(|()| None);
    }
    if matches!(record, Some(PublicationState::Published(_))) {
        sync_results(paths).map_err(result_error)?;
        if current == target {
            let previous = pending.previous.root(paths)?;
            prune_generations(paths, pending.next_generation, previous.as_deref());
        }
        return clear_pending(paths).map(|()| None);
    }
    if (current != target && current != pending.previous)
        || config
            .shelf
            .is_some_and(|settings| settings.library_id.get() != pending.library_id)
    {
        reject(paths, &pending.shipment_id, Rejection::Superseded).map_err(result_error)?;
        clear_pending(paths)?;
        return Ok(Some(Publication::Rejected {
            shipment_id: pending.shipment_id,
            reason: Rejection::Superseded,
        }));
    }
    let previous = pending.previous.root(paths)?;
    activate_locked(paths, pending.next_generation, |settings, new| {
        if new.id != pending.library_id {
            return Err("予約した棚の ID が一致しません".into());
        }
        let current = ShelfSource::from_config(settings);
        if current == ShelfSource::Generation(pending.next_generation) {
            // 配信先を書く前に引き継いだので、公開結果の記録から続ける。
            return Ok(());
        }
        if current != pending.previous {
            return Err("予約した引き継ぎ元と配信中の棚が一致しません".into());
        }
        reached(SwapStage::BeforeAdoption);
        if let Some(previous) = &previous {
            let old = Library::open_matching(previous, pending.library_id)
                .map_err(|error| format!("前の棚を開けません: {error}"))?;
            let (positions, favorites) = new
                .adopt_from_previous(&old)
                .map_err(|error| format!("読書位置とお気に入りを写せません: {error}"))?;
            log::info!(
                "前の棚から読書位置を {positions} 件、お気に入りを {favorites} 件写しました"
            );
        }
        reached(SwapStage::AfterAdoption);
        Ok(())
    })?;
    reached(SwapStage::AfterActivation);
    let published = load_config(paths)?
        .shelf
        .filter(|settings| {
            settings.generation == Some(pending.next_generation)
                && settings.library_id.get() == pending.library_id
        })
        .ok_or("配信する棚の設定を確認できません")?;
    write_result(
        paths,
        &pending.shipment_id,
        &PublicationState::Published(PublishedShelf {
            sync_epoch: published.sync_epoch,
            works: pending.works,
        }),
    )
    .map_err(result_error)?;
    prune_generations(paths, pending.next_generation, previous.as_deref());
    clear_pending(paths)?;
    Ok(Some(Publication::Published {
        shipment_id: pending.shipment_id,
        works: pending.works,
    }))
}

fn clear_pending(paths: &ServerPaths) -> Result<(), String> {
    fs::remove_file(paths.root.join(PENDING))
        .map_err(|error| format!("棚の切り替えの予約を片付けられません: {error}"))
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

/// 端末に棚を読む許可を付け外しする（設定の `shelf.readers` に書く）。読む利用者は棚の
/// 持ち主（デスクトップと同じ利用者）。
pub fn set_reader(paths: &ServerPaths, device: &str, read: bool) -> Result<Id, String> {
    edit_config(paths, |path| {
        let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
        let config: ServerConfig = toml::from_str(&text).map_err(|error| error.to_string())?;
        let owner = config
            .shelf
            .as_ref()
            .map(|settings| settings.library_id)
            .ok_or("このサーバーはまだ棚を配っていません")?;
        let mut document: DocumentMut = text
            .parse()
            .map_err(|error: toml_edit::TomlError| error.to_string())?;
        let settings = document
            .entry("shelf")
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
        write_atomic(path, written.as_bytes())?;
        Ok(owner)
    })
}

#[cfg(test)]
#[path = "standalone_shipment_tests.rs"]
mod tests;
