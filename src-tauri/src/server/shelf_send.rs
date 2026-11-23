//! デスクトップ版の側の、棚の送信。棚の写しを作り、サーバーが持っていない実体と
//! 台帳の部分だけを上げる。サーバーは受け取った写しを検査して組み、立ち上げ直して配る。
//!
//! 台帳は写しのたびに保管と同じ分け方（4 MiB ずつ）で分けて実体と同じ口で送る。写しは頁の並びを保つので、
//! 前に送ったときと同じ部分は送らない。

use super::standalone::shipment::{Opened, ShipmentStatus, WireChunk, MAX_QUERY_CHUNKS};
use super::sync_send::{
    build_client, check_status, net_error, normalized_base, RemoteServer, SendHandle,
};
use crate::library::{FileChunk, Library, ObjectStore};
use futures_util::{stream, StreamExt};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const PARALLEL_UPLOADS: usize = 4;
const CHUNK_RETRIES: usize = 3;
/// 組み終えるまで、この間を置いて進み具合を尋ねる。
const STATUS_POLL: Duration = Duration::from_secs(2);
/// 組み終えたあとの立ち上げ直しで返事が無い間、待つ回数（約 3 分）。
const RESTART_WAIT_POLLS: usize = 90;

/// 送り終えた結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShelfSent {
    /// サーバーで組んだ棚の作品数（立ち上げ直しで聞き逃したら分からない）。
    pub works: Option<u64>,
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

    async fn put_chunk(
        &self,
        shipment: &str,
        chunk: &FileChunk,
        body: Vec<u8>,
    ) -> Result<(), String> {
        let mut last_error = String::new();
        for attempt in 0..CHUNK_RETRIES {
            self.handle.check_cancel()?;
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(2 << attempt)).await;
            }
            let sent = self
                .client
                .put(self.url(&format!("/{shipment}/chunks/{}", chunk.id)))
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

/// 棚の写しを作って送り、サーバーが組み終えるまで待つ。`outgoing` は送り手の作業場所
/// （中身は毎回作り直す）。
pub async fn send_shelf(
    remote: &RemoteServer,
    token: &str,
    library: Arc<Library>,
    outgoing: &Path,
    handle: &SendHandle,
) -> Result<ShelfSent, String> {
    handle.update(|progress| progress.phase = "snapshot".into());
    if outgoing.exists() {
        fs::remove_dir_all(outgoing).map_err(|error| format!("前の写しを消せません: {error}"))?;
    }
    fs::create_dir_all(outgoing).map_err(|error| format!("写しの置き場を作れません: {error}"))?;
    let snapshot = outgoing.join("library.db");
    let shelf = Arc::clone(&library);
    let written = snapshot.clone();
    let (objects, database, shipped_sequence) = tokio::task::spawn_blocking(move || {
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

    let shipper = Arc::new(Shipper {
        client: build_client(remote).await?,
        base: normalized_base(&remote.url),
        token: token.to_string(),
        handle: handle.clone(),
    });
    let before = shipper.status().await?;
    let opened: Opened = check_status(
        shipper
            .client
            .post(shipper.url(""))
            .bearer_auth(token)
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

    handle.update(|progress| progress.phase = "manifest".into());
    let mut seen = HashSet::new();
    let all: Vec<FileChunk> = objects
        .iter()
        .chain(&database.chunks)
        .filter(|chunk| seen.insert(chunk.id))
        .cloned()
        .collect();
    let mut needed = Vec::new();
    for batch in all.chunks(MAX_QUERY_CHUNKS) {
        handle.check_cancel()?;
        let wires: Vec<WireChunk> = batch.iter().map(wire).collect();
        let response: serde_json::Value = check_status(
            shipper
                .client
                .post(shipper.url(&format!("/{shipment}/needed")))
                .bearer_auth(token)
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
    handle.update(|progress| {
        progress.phase = "upload".into();
        progress.files_total = needed.len() as u64;
        progress.files_done = 0;
        progress.bytes_total = needed.iter().map(|chunk| chunk.length).sum();
        progress.bytes_done = 0;
    });
    let snapshot = Arc::new(snapshot);
    let uploaded: Vec<Result<(), String>> = stream::iter(needed.iter().cloned())
        .map(|chunk| {
            let shipper = Arc::clone(&shipper);
            let library = Arc::clone(&library);
            // 送信が中断されても、始まった読み出しが終わるまで実体を守る。
            let objects = Arc::clone(&objects);
            let snapshot = Arc::clone(&snapshot);
            let offset = offsets.get(&chunk.id).copied();
            let shipment = shipment.clone();
            async move {
                shipper.handle.check_cancel()?;
                let read = chunk.clone();
                let body = tokio::task::spawn_blocking(move || {
                    let _pin = objects;
                    let sent = match offset {
                        // 台帳の部分は写しから読んで、その場で符号化する。
                        Some(offset) => read_part(&snapshot, offset, &read),
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
                shipper.put_chunk(&shipment, &chunk, body).await?;
                shipper.handle.update(|progress| {
                    progress.files_done += 1;
                    progress.bytes_done += chunk.length;
                });
                Ok(())
            }
        })
        .buffer_unordered(PARALLEL_UPLOADS)
        .collect()
        .await;
    uploaded.into_iter().collect::<Result<Vec<_>, _>>()?;

    handle.check_cancel()?;
    handle.update(|progress| progress.phase = "commit".into());
    check_status(
        shipper
            .client
            .post(shipper.url(&format!("/{shipment}/commit")))
            .bearer_auth(token)
            .json(&serde_json::json!({ "database": database }))
            .send()
            .await
            .map_err(net_error)?,
    )
    .await?;
    let _ = fs::remove_dir_all(outgoing);

    // 組み終えるのを待つ。中止しても、サーバーは組み続けて次の起動で配る。
    handle.update(|progress| progress.phase = "assemble".into());
    let sent = needed.len() as u64;
    let mut silent = 0;
    loop {
        handle.check_cancel()?;
        tokio::time::sleep(STATUS_POLL).await;
        let status = match shipper.status().await {
            Ok(status) => status,
            Err(error) => {
                // 組み終えて立ち上げ直している間は返事が無い。
                silent += 1;
                if silent > RESTART_WAIT_POLLS {
                    return Err(error);
                }
                continue;
            }
        };
        silent = 0;
        match status.state.as_str() {
            "assembling" => continue,
            "ready" => {
                return Ok(ShelfSent {
                    works: status.works,
                    sent,
                    shipped_sequence,
                })
            }
            "failed" => {
                return Err(format!(
                    "サーバーで棚を組めませんでした: {}",
                    status.error.unwrap_or_default()
                ))
            }
            // 立ち上げ直したあとは、配る世代が変わっていれば届いている。
            _ if status.generation.is_some() && status.generation != before.generation => {
                return Ok(ShelfSent {
                    works: None,
                    sent,
                    shipped_sequence,
                })
            }
            _ => {
                return Err(
                    "サーバーが棚を組み終える前に止まりました。もう一度送ってください".into(),
                )
            }
        }
    }
}
