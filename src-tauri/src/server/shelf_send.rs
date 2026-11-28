//! デスクトップ版の側の、棚の送信。棚の写しを作り、サーバーが持っていない実体と
//! 台帳の部分だけを上げる。サーバーは受け取った写しを検査して組み、立ち上げ直して配る。
//!
//! 台帳は写しのたびに保管と同じ分け方（4 MiB ずつ）で分けて実体と同じ口で送る。写しは頁の並びを保つので、
//! 前に送ったときと同じ部分は送らない。

use super::standalone::shipment::{
    Opened, PublicationState, PublishedShelf, Rejection, ShipmentResult, ShipmentStatus, WireChunk,
    MAX_QUERY_CHUNKS,
};
use super::sync_send::{
    check_status, net_error, normalized_base, ClientCache, RemoteServer, SendHandle, CANCELED,
};
use crate::library::{protocol::Id, FileChunk, Library, ObjectStore};
use futures_util::{stream, TryStreamExt};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

const PARALLEL_UPLOADS: usize = 4;
const CHUNK_RETRIES: usize = 3;
const CHUNK_UPLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// 棚が公開されるまで、この間を置いて進み具合を尋ねる。
const STATUS_POLL: Duration = Duration::from_secs(2);
/// 公開待ちや再起動を待つ時間。組み立てにかかる時間は含まない。
const PUBLICATION_WAIT: Duration = Duration::from_secs(3 * 60);
const RESULT_TIMEOUT: Duration = Duration::from_secs(15);
const PUBLICATION_UNCONFIRMED: &str =
    "この送信の公開を確認できませんでした。サーバーの状態を確認してください";

/// 送り終えた結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShelfSent {
    /// 公開した棚の作品数と、サーバーがその棚の同期に使う世代。
    pub works: u64,
    pub sync_epoch: Id,
    /// 送った実体と台帳の部分の数。
    pub sent: u64,
    /// 送った台帳の変更の記録の最後の番号。サーバーの棚はこの番号から続けて数える。
    pub shipped_sequence: i64,
}

struct Shipper {
    client: reqwest::Client,
    base: String,
    token: String,
    handle: SendHandle,
}

struct SnapshotDirectory {
    path: PathBuf,
    removed: Arc<Notify>,
}

impl Drop for SnapshotDirectory {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.path) {
            log::warn!("棚の送信に使った写しを消せません: {error}");
        }
        self.removed.notify_one();
    }
}

impl Shipper {
    fn url(&self, path: &str) -> String {
        format!("{}/api/admin/shelf/shipments{path}", self.base)
    }

    async fn status(&self) -> Result<ShipmentStatus, String> {
        check_status(
            self.client
                .get(self.url(""))
                .bearer_auth(&self.token)
                .send()
                .await
                .map_err(net_error)?,
        )
        .await?
        .json()
        .await
        .map_err(net_error)
    }

    async fn result(&self, id: &str) -> Result<Option<ShipmentResult>, String> {
        let response = self
            .client
            .get(self.url(&format!("/{id}")))
            .timeout(RESULT_TIMEOUT)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(net_error)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        check_status(response)
            .await?
            .json()
            .await
            .map(Some)
            .map_err(net_error)
    }

    async fn put_chunk(
        &self,
        shipment: &str,
        chunk: &FileChunk,
        body: Vec<u8>,
    ) -> Result<(), String> {
        let mut last_error = String::new();
        for attempt in 0..CHUNK_RETRIES {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(2 << attempt)).await;
            }
            let sent = self
                .client
                .put(self.url(&format!("/{shipment}/chunks/{}", chunk.id)))
                .timeout(CHUNK_UPLOAD_TIMEOUT)
                .query(&[("length", chunk.length)])
                .bearer_auth(&self.token)
                .body(body.clone())
                .send()
                .await;
            match sent {
                Ok(response) if response.status().is_server_error() => {
                    last_error = format!("サーバーが処理できませんでした（{}）", response.status());
                }
                Ok(response) => {
                    check_status(response).await?;
                    return Ok(());
                }
                Err(error) => last_error = net_error(error),
            }
        }
        Err(last_error)
    }
}

/// 台帳の写しの `offset` から一部分を読み、送る形に符号化する。
fn read_part(snapshot: &Path, offset: u64, chunk: &FileChunk) -> piep_storage::Result<Vec<u8>> {
    let mut file = fs::File::open(snapshot)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut original = Vec::with_capacity(chunk.length as usize);
    file.take(chunk.length).read_to_end(&mut original)?;
    let (encoded_as, encoded) = piep_storage::encode_chunk(&original)?;
    if encoded_as != *chunk {
        return Err(piep_storage::Error::Integrity(
            "台帳の写しが分けたときと変わりました".into(),
        ));
    }
    Ok(encoded)
}

fn wire(chunk: &FileChunk) -> WireChunk {
    WireChunk {
        id: chunk.id.to_string(),
        length: chunk.length,
    }
}

/// 棚の台帳の版を扱えて、送信 ID ごとに公開の結果を返すか。
fn receivable(status: &ShipmentStatus) -> Result<(), String> {
    if status.publication_by_id
        && status
            .schema
            .is_some_and(|schema| schema >= crate::library::schema::VERSION)
    {
        Ok(())
    } else {
        Err("サーバーがこの棚の送信に対応していません。サーバーを更新してから送ってください".into())
    }
}

/// 棚の写しを作って送り、その送信の棚が公開されるまで待つ。
pub(super) async fn send_shelf(
    clients: &ClientCache,
    remote: &RemoteServer,
    token: &str,
    library: Arc<Library>,
    outgoing: &Path,
    handle: &SendHandle,
) -> Result<ShelfSent, String> {
    if outgoing.exists() {
        fs::remove_dir_all(outgoing).map_err(|error| format!("前の写しを消せません: {error}"))?;
    }
    fs::create_dir_all(outgoing).map_err(|error| format!("写しの置き場を作れません: {error}"))?;
    let removed = Arc::new(Notify::new());
    let snapshot = Arc::new(SnapshotDirectory {
        path: outgoing.to_path_buf(),
        removed: Arc::clone(&removed),
    });
    let result = tokio::select! {
        biased;
        () = handle.canceled() => Err(CANCELED.into()),
        result = async move {
            let shipper = Shipper {
                client: clients.client(remote).await?,
                base: normalized_base(&remote.url),
                token: token.to_string(),
                handle: handle.clone(),
            };
            ship_shelf(&shipper, library, snapshot).await
        } => result,
    };
    // 送信を打ち切ったときも、始まった台帳の読み書きが終わってから写しを消す。
    removed.notified().await;
    result
}

async fn ship_shelf(
    shipper: &Shipper,
    library: Arc<Library>,
    snapshot: Arc<SnapshotDirectory>,
) -> Result<ShelfSent, String> {
    // 写しを作る前に、サーバーがこの棚の版を扱えるかを確かめる。
    receivable(&shipper.status().await?)?;
    shipper
        .handle
        .update(|progress| progress.phase = "snapshot".into());
    let shelf = Arc::clone(&library);
    let writing = Arc::clone(&snapshot);
    let (objects, database, shipped_sequence) = tokio::task::spawn_blocking(move || {
        let written = writing.path.join("library.db");
        let objects = shelf
            .ship_snapshot(&written)
            .map_err(|error| format!("棚の写しを作れません: {error}"))?;
        let shipped_sequence = rusqlite::Connection::open_with_flags(
            &written,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .and_then(|conn| {
            conn.query_row("SELECT coalesce(max(sequence),0) FROM outbox", [], |row| {
                row.get::<_, i64>(0)
            })
        })
        .map_err(|error| format!("棚の写しを読めません: {error}"))?;
        // 分け方と識別子だけを求める。圧縮するのは、サーバーに求められた部分だけ。
        let database = fs::File::open(&written)
            .map_err(piep_storage::Error::from)
            .and_then(ObjectStore::plan)
            .map_err(|error| format!("棚の写しを分けられません: {error}"))?;
        Ok::<_, String>((objects, database, shipped_sequence))
    })
    .await
    .map_err(|_| "棚の写しを作れません".to_string())??;
    // 台帳の部分の、写しの中での位置。
    let mut offsets = HashMap::new();
    let mut at = 0u64;
    for chunk in &database.chunks {
        offsets.entry(chunk.id).or_insert(at);
        at += chunk.length;
    }
    let offsets = Arc::new(offsets);

    let opened: Opened = check_status(
        shipper
            .client
            .post(shipper.url(""))
            .bearer_auth(&shipper.token)
            .json(&serde_json::json!({ "libraryId": library.id }))
            .send()
            .await
            .map_err(net_error)?,
    )
    .await?
    .json()
    .await
    .map_err(net_error)?;
    let shipment = opened.shipment_id;

    shipper
        .handle
        .update(|progress| progress.phase = "manifest".into());
    let mut seen = HashSet::new();
    let all: Vec<FileChunk> = objects
        .iter()
        .chain(&database.chunks)
        .filter(|chunk| seen.insert(chunk.id))
        .cloned()
        .collect();
    let mut needed = Vec::new();
    for batch in all.chunks(MAX_QUERY_CHUNKS) {
        let wires: Vec<WireChunk> = batch.iter().map(wire).collect();
        let response: serde_json::Value = check_status(
            shipper
                .client
                .post(shipper.url(&format!("/{shipment}/needed")))
                .bearer_auth(&shipper.token)
                .json(&serde_json::json!({ "chunks": wires }))
                .send()
                .await
                .map_err(net_error)?,
        )
        .await?
        .json()
        .await
        .map_err(net_error)?;
        let asked: HashSet<&str> = response["needed"]
            .as_array()
            .ok_or("サーバーの応答を読み取れません")?
            .iter()
            .filter_map(|id| id.as_str())
            .collect();
        needed.extend(
            batch
                .iter()
                .filter(|chunk| asked.contains(chunk.id.to_string().as_str()))
                .cloned(),
        );
    }
    shipper.handle.update(|progress| {
        progress.phase = "upload".into();
        progress.files_total = needed.len() as u64;
        progress.files_done = 0;
        progress.bytes_total = needed.iter().map(|chunk| chunk.length).sum();
        progress.bytes_done = 0;
    });
    let upload_id = shipment.as_str();
    stream::iter(needed.iter().cloned().map(Ok::<_, String>))
        .try_for_each_concurrent(PARALLEL_UPLOADS, |chunk| {
            let library = Arc::clone(&library);
            // 送信が中断されても、始まった読み出しが終わるまで実体を守る。
            let objects = Arc::clone(&objects);
            let offset = offsets.get(&chunk.id).copied();
            let reading = offset.map(|offset| (Arc::clone(&snapshot), offset));
            async move {
                let read = chunk.clone();
                let body = tokio::task::spawn_blocking(move || {
                    let _pin = objects;
                    let sent = match reading {
                        // 台帳の部分は写しから読んで、その場で符号化する。
                        Some((snapshot, offset)) => {
                            read_part(&snapshot.path.join("library.db"), offset, &read)
                        }
                        None => {
                            let mut encoded = Vec::new();
                            library
                                .objects
                                .export_chunk(&read, &mut encoded)
                                .map(|()| encoded)
                        }
                    };
                    sent.map_err(|error| format!("送る実体を読めません: {error}"))
                })
                .await
                .map_err(|_| "送る実体を読めません".to_string())??;
                shipper.put_chunk(upload_id, &chunk, body).await?;
                shipper.handle.update(|progress| {
                    progress.files_done += 1;
                    progress.bytes_done += chunk.length;
                });
                Ok(())
            }
        })
        .await?;

    shipper
        .handle
        .update(|progress| progress.phase = "commit".into());
    check_status(
        shipper
            .client
            .post(shipper.url(&format!("/{shipment}/commit")))
            .bearer_auth(&shipper.token)
            .json(&serde_json::json!({ "database": database }))
            .send()
            .await
            .map_err(net_error)?,
    )
    .await?;
    drop(snapshot);

    // 公開を待つ。中止しても、サーバーは組み続けて次の起動で配る。
    shipper
        .handle
        .update(|progress| progress.phase = "assemble".into());
    let sent = needed.len() as u64;
    let mut waiting_since = tokio::time::Instant::now();
    loop {
        tokio::time::sleep(STATUS_POLL).await;
        match shipper.result(&shipment).await {
            Ok(Some(status)) => {
                let pending = match status.state {
                    PublicationState::Pending { restarting } => Some(restarting),
                    _ => None,
                };
                if let Some(published) = publication_result(status, &shipment)? {
                    return Ok(ShelfSent {
                        works: published.works,
                        sync_epoch: published.sync_epoch,
                        sent,
                        shipped_sequence,
                    });
                }
                if let Some(restarting) = pending {
                    shipper.handle.update(|progress| {
                        progress.message = Some(if restarting {
                            "サーバーでの公開を待っています".into()
                        } else {
                            "サーバーを再起動してください".into()
                        });
                    });
                } else {
                    // 組み立ての進行中は、再起動や応答を待つ時間に数えない。
                    waiting_since = tokio::time::Instant::now();
                    continue;
                }
            }
            Ok(None) => return Err(PUBLICATION_UNCONFIRMED.into()),
            // サーバーが再起動している間は応答が途切れる。
            Err(_) => {}
        }
        if waiting_since.elapsed() >= PUBLICATION_WAIT {
            return Err(PUBLICATION_UNCONFIRMED.into());
        }
    }
}

fn publication_result(
    result: ShipmentResult,
    shipment: &str,
) -> Result<Option<PublishedShelf>, String> {
    if result.shipment_id != shipment {
        return Err(PUBLICATION_UNCONFIRMED.into());
    }
    match result.state {
        PublicationState::Assembling | PublicationState::Pending { .. } => Ok(None),
        PublicationState::Published(shelf) => Ok(Some(shelf)),
        PublicationState::Receiving => Err(PUBLICATION_UNCONFIRMED.into()),
        PublicationState::Rejected { reason } => Err(match reason {
            Rejection::AssemblyFailed => {
                "サーバーで棚を組めませんでした。サーバーの記録を確認してください"
            }
            Rejection::Interrupted => {
                "サーバーで棚の組み立てが中断されました。もう一度送ってください"
            }
            Rejection::Superseded => {
                "サーバーで配信する棚が変わりました。サーバーの状態を確認してください"
            }
        }
        .into()),
    }
}

#[cfg(test)]
mod schema_tests {
    use super::*;

    #[test]
    fn another_shipment_publication_is_not_success() {
        let result = ShipmentResult {
            shipment_id: "previous".into(),
            state: PublicationState::Published(PublishedShelf {
                sync_epoch: Id::new(uuid::Uuid::now_v7()).unwrap(),
                works: 7,
            }),
        };
        assert!(publication_result(result, "current").is_err());
    }

    #[test]
    fn only_publication_completes_the_requested_shipment() {
        let result = |state| ShipmentResult {
            shipment_id: "current".into(),
            state,
        };
        assert_eq!(
            publication_result(
                result(PublicationState::Pending { restarting: false }),
                "current"
            ),
            Ok(None)
        );
        assert!(publication_result(
            result(PublicationState::Rejected {
                reason: Rejection::AssemblyFailed
            }),
            "current"
        )
        .is_err());
        let published = PublishedShelf {
            sync_epoch: Id::new(uuid::Uuid::now_v7()).unwrap(),
            works: 7,
        };
        assert_eq!(
            publication_result(
                result(PublicationState::Published(published.clone())),
                "current"
            ),
            Ok(Some(published))
        );
    }

    /// 版を返さない古いサーバーや、版の古いサーバーには送らない。同じか新しければ送る。
    #[test]
    fn a_shelf_is_sent_only_to_a_server_that_can_assemble_it() {
        let status = |schema| ShipmentStatus {
            schema,
            publication_by_id: true,
            ..Default::default()
        };
        let current = crate::library::schema::VERSION;
        assert!(receivable(&status(None)).is_err());
        assert!(receivable(&status(Some(current - 1))).is_err());
        assert!(receivable(&status(Some(current))).is_ok());
        assert!(receivable(&status(Some(current + 1))).is_ok());
        assert!(receivable(&ShipmentStatus {
            publication_by_id: false,
            ..status(Some(current))
        })
        .is_err());
    }
}
