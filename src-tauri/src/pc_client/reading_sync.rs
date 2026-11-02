//! 読書位置をサーバーと同期する。
//!
//! 読書を止めないよう、書き込みはまず端末のファイルに置き、通信はあとで行う。
//! 圏外で読んだ分もここに残り、次につながったときに届く。

use super::*;
use std::collections::BTreeMap;
use tokio::sync::Mutex as AsyncMutex;

const PULL_LIMIT: i64 = 200;
// 1 回の同期で読むページ数の上限。残りは次の同期で続きから読む。
const MAX_PULL_PAGES: usize = 50;

pub struct ReadingSyncState {
    file: PathBuf,
    file_lock: Mutex<()>,
    sync_lock: AsyncMutex<()>,
}

/// サーバーが持つ読書位置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadingPosition {
    source: String,
    source_id: String,
    version: Option<i64>,
    edit_hash: Option<String>,
    page: i64,
    position: f64,
    finished: bool,
    device_id: String,
    device_time: i64,
    cursor: i64,
}

/// まだサーバーへ届いていない書き込み。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadingWrite {
    source: String,
    source_id: String,
    version: Option<i64>,
    edit_hash: Option<String>,
    page: i64,
    position: f64,
    finished: bool,
    device_time: i64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SyncFile {
    url: String,
    device_id: String,
    cursor: i64,
    positions: BTreeMap<String, ReadingPosition>,
    pending: BTreeMap<String, ReadingWrite>,
}

impl SyncFile {
    /// ファイルを今の接続に合わせる。
    ///
    /// 別のサーバーの位置は混ぜない。同じサーバーへ登録し直しただけなら、圏外で読んだ分は
    /// 残し、読み直しのために見た位置だけ捨てる（サーバーを作り直していると番号が戻るため）。
    fn bind(&mut self, saved: &SavedConnection) {
        if self.url != saved.url {
            *self = Self {
                url: saved.url.clone(),
                device_id: saved.device_id.clone(),
                ..Self::default()
            };
        } else if self.device_id != saved.device_id {
            self.device_id = saved.device_id.clone();
            self.cursor = 0;
            self.positions.clear();
        }
    }

    fn accept(&mut self, position: ReadingPosition) {
        let key = reading_key(&position.source, &position.source_id);
        let newer = self
            .positions
            .get(&key)
            .is_none_or(|current| position.device_time >= current.device_time);
        if newer {
            self.positions.insert(key, position);
        }
    }

    /// 届いた書き込みを消す。送っている間に新しい位置が来ていたら、そちらは残す。
    fn delivered(&mut self, key: &str, device_time: i64) {
        if self
            .pending
            .get(key)
            .is_some_and(|write| write.device_time == device_time)
        {
            self.pending.remove(key);
        }
    }

    fn record(&mut self, write: ReadingWrite) {
        let key = reading_key(&write.source, &write.source_id);
        let newer = self
            .pending
            .get(&key)
            .is_none_or(|current| write.device_time >= current.device_time);
        if newer {
            self.pending.insert(key, write);
        }
    }
}

fn reading_key(source: &str, source_id: &str) -> String {
    format!("{source}:{source_id}")
}

fn validate_write(write: &ReadingWrite) -> Result<(), ClientError> {
    if !matches!(write.source.as_str(), "pixiv" | "fanbox")
        || write.source_id.is_empty()
        || write.source_id.chars().count() > 200
        || !(0..=1_000_000).contains(&write.page)
        || !(0.0..=1.0).contains(&write.position)
        || write.device_time <= 0
        || write.edit_hash.as_ref().is_some_and(|hash| hash.len() > 128)
    {
        return Err(ClientError::remote());
    }
    Ok(())
}

fn write_endpoint(base: &str, source: &str, source_id: &str) -> Result<Url, ClientError> {
    let mut url = Url::parse(base).map_err(|_| ClientError::storage())?;
    url.path_segments_mut()
        .map_err(|_| ClientError::storage())?
        .pop_if_empty()
        .extend(["api", "v1", "reading", source, source_id]);
    Ok(url)
}

impl ReadingSyncState {
    pub fn new(file: PathBuf) -> Self {
        Self {
            file,
            file_lock: Mutex::new(()),
            sync_lock: AsyncMutex::new(()),
        }
    }

    /// ファイルを読み、接続に合わせて、変更を書き戻す。
    fn update<T>(
        &self,
        saved: &SavedConnection,
        change: impl FnOnce(&mut SyncFile) -> T,
    ) -> Result<T, ClientError> {
        let _guard = self.file_lock.lock().map_err(|_| ClientError::storage())?;
        // 壊れていても読書は止めない。読み直せば位置はまた届く。
        let mut file: SyncFile = fs::read(&self.file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        file.bind(saved);
        let result = change(&mut file);
        let bytes = serde_json::to_vec(&file).map_err(|_| ClientError::storage())?;
        let temporary = self.file.with_extension("json.tmp");
        fs::write(&temporary, bytes).map_err(|_| ClientError::storage())?;
        fs::rename(&temporary, &self.file).map_err(|_| ClientError::storage())?;
        Ok(result)
    }

    /// 溜まった書き込みを送る。圏外やサーバーの不調なら残して次に任せる。
    async fn push(
        &self,
        client: &reqwest::Client,
        saved: &SavedConnection,
    ) -> Result<(), ClientError> {
        let pending = self.update(saved, |file| file.pending.clone())?;
        for (key, write) in pending {
            let response = client
                .put(write_endpoint(&saved.url, &write.source, &write.source_id)?)
                .bearer_auth(&saved.token)
                .json(&serde_json::json!({
                    "version": write.version,
                    "editHash": write.edit_hash,
                    "page": write.page,
                    "position": write.position,
                    "finished": write.finished,
                    "deviceTime": write.device_time,
                }))
                .send()
                .await
                .map_err(|error| ClientError::from_send(&error, saved.pinned()))?;
            let status = response.status();
            if status.is_success() {
                let stored: ReadingPosition = response.bounded_json().await?;
                self.update(saved, |file| {
                    file.accept(stored);
                    file.delivered(&key, write.device_time);
                })?;
            } else if status == reqwest::StatusCode::UNAUTHORIZED {
                return Err(ClientError::revoked());
            } else if status.is_client_error() {
                // サーバーが受け取らない書き込みを持ち続けると、後ろが永久に詰まる。
                self.update(saved, |file| file.delivered(&key, write.device_time))?;
            } else {
                return Err(ClientError::remote());
            }
        }
        Ok(())
    }

    /// 前回の続きからサーバーの変更を読む。
    async fn pull(
        &self,
        client: &reqwest::Client,
        saved: &SavedConnection,
    ) -> Result<(), ClientError> {
        #[derive(Deserialize)]
        struct Page {
            items: Vec<ReadingPosition>,
            cursor: i64,
        }
        for _ in 0..MAX_PULL_PAGES {
            let since = self.update(saved, |file| file.cursor)?;
            let response = client
                .get(format!(
                    "{}/api/v1/reading?since={since}&limit={PULL_LIMIT}",
                    saved.url
                ))
                .bearer_auth(&saved.token)
                .send()
                .await
                .map_err(|error| ClientError::from_send(&error, saved.pinned()))?;
            let page: Page = check_response(response)?.bounded_json().await?;
            let full = page.items.len() as i64 >= PULL_LIMIT;
            self.update(saved, |file| {
                for item in page.items {
                    file.accept(item);
                }
                file.cursor = page.cursor.max(file.cursor);
            })?;
            if !full {
                break;
            }
        }
        Ok(())
    }
}

fn sync_target(pc: &PcClientState) -> Result<Option<SavedConnection>, ClientError> {
    Ok(pc.load()?.filter(SavedConnection::can_sync_reading))
}

/// 読んだ位置を端末に置く。通信はしない。
#[tauri::command]
pub async fn reading_record(
    state: State<'_, ReadingSyncState>,
    pc: State<'_, PcClientState>,
    write: ReadingWrite,
) -> Result<(), ClientError> {
    validate_write(&write)?;
    let Some(saved) = sync_target(&pc)? else {
        return Ok(());
    };
    state.update(&saved, |file| file.record(write))
}

/// 溜まった位置を送り、サーバーの変更を受け取る。
#[tauri::command]
pub async fn reading_sync(
    state: State<'_, ReadingSyncState>,
    pc: State<'_, PcClientState>,
) -> Result<(), ClientError> {
    let Some(saved) = sync_target(&pc)? else {
        return Ok(());
    };
    let _running = state.sync_lock.lock().await;
    let client = pc.client(&saved)?;
    state.push(&client, &saved).await?;
    state.pull(&client, &saved).await
}

/// 最後に受け取った、この作品の位置を返す。通信はしない。
#[tauri::command]
pub async fn reading_get(
    state: State<'_, ReadingSyncState>,
    pc: State<'_, PcClientState>,
    source: String,
    source_id: String,
) -> Result<Option<ReadingPosition>, ClientError> {
    let Some(saved) = sync_target(&pc)? else {
        return Ok(None);
    };
    state.update(&saved, |file| {
        file.positions.get(&reading_key(&source, &source_id)).cloned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved(url: &str, device_id: &str) -> SavedConnection {
        SavedConnection {
            url: url.to_string(),
            fingerprint: None,
            certificate_der: None,
            token: String::new(),
            device_id: device_id.to_string(),
            device_name: "phone".to_string(),
            capabilities: vec!["reading-sync".to_string()],
        }
    }

    fn write(source_id: &str, page: i64, device_time: i64) -> ReadingWrite {
        ReadingWrite {
            source: "pixiv".to_string(),
            source_id: source_id.to_string(),
            version: Some(1),
            edit_hash: None,
            page,
            position: 0.5,
            finished: false,
            device_time,
        }
    }

    fn position(source_id: &str, page: i64, device_time: i64, cursor: i64) -> ReadingPosition {
        ReadingPosition {
            source: "pixiv".to_string(),
            source_id: source_id.to_string(),
            version: Some(1),
            edit_hash: None,
            page,
            position: 0.5,
            finished: false,
            device_id: "other".to_string(),
            device_time,
            cursor,
        }
    }

    fn state() -> (PathBuf, ReadingSyncState) {
        let directory = std::env::temp_dir().join(format!("piep-android-reading-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).expect("directory");
        let file = directory.join("reading-sync.json");
        (directory, ReadingSyncState::new(file))
    }

    #[test]
    fn newest_write_wins_and_an_older_one_does_not_replace_it() {
        let mut file = SyncFile::default();
        file.record(write("1", 5, 200));
        file.record(write("1", 2, 100));
        assert_eq!(file.pending["pixiv:1"].page, 5);
        file.record(write("1", 7, 300));
        assert_eq!(file.pending["pixiv:1"].page, 7);
    }

    #[test]
    fn a_write_made_while_sending_survives_delivery() {
        let mut file = SyncFile::default();
        file.record(write("1", 5, 200));
        file.record(write("1", 7, 300));
        // 200 を送っている間に 300 が来た。
        file.delivered("pixiv:1", 200);
        assert_eq!(file.pending["pixiv:1"].page, 7);
        file.delivered("pixiv:1", 300);
        assert!(file.pending.is_empty());
    }

    #[test]
    fn an_older_position_from_the_server_does_not_replace_a_newer_one() {
        let mut file = SyncFile::default();
        file.accept(position("1", 5, 200, 1));
        file.accept(position("1", 2, 100, 2));
        assert_eq!(file.positions["pixiv:1"].page, 5);
    }

    #[test]
    fn another_server_starts_empty_and_a_new_pairing_keeps_only_unsent_reading() {
        let mut file = SyncFile::default();
        file.bind(&saved("https://a.example", "one"));
        file.record(write("1", 5, 200));
        file.accept(position("2", 3, 100, 9));
        file.cursor = 9;

        // 同じサーバーへ登録し直した。読み取りは最初から、未送信は残す。
        file.bind(&saved("https://a.example", "two"));
        assert_eq!(file.cursor, 0);
        assert!(file.positions.is_empty());
        assert_eq!(file.pending.len(), 1);

        // 別のサーバーへ、前のサーバーの作品の位置を送らない。
        file.bind(&saved("https://b.example", "three"));
        assert!(file.pending.is_empty());
        assert_eq!(file.url, "https://b.example");
    }

    #[test]
    fn unsent_reading_survives_on_disk_and_a_broken_file_does_not_stop_reading() {
        let (directory, state) = state();
        let connection = saved("https://a.example", "one");
        state.update(&connection, |file| file.record(write("1", 5, 200))).expect("record");
        let pending = state.update(&connection, |file| file.pending.len()).expect("read");
        assert_eq!(pending, 1);

        fs::write(&state.file, b"{ broken").expect("break");
        let pending = state.update(&connection, |file| file.pending.len()).expect("recover");
        assert_eq!(pending, 0);
        fs::remove_dir_all(&directory).expect("cleanup");
    }

    #[test]
    fn validates_what_the_reader_sends() {
        assert!(validate_write(&write("1", 0, 1)).is_ok());
        for bad in [
            ReadingWrite { source: "web".to_string(), ..write("1", 0, 1) },
            ReadingWrite { source_id: String::new(), ..write("1", 0, 1) },
            ReadingWrite { page: -1, ..write("1", 0, 1) },
            ReadingWrite { position: 1.5, ..write("1", 0, 1) },
            ReadingWrite { position: f64::NAN, ..write("1", 0, 1) },
            ReadingWrite { device_time: 0, ..write("1", 0, 1) },
            ReadingWrite { edit_hash: Some("x".repeat(129)), ..write("1", 0, 1) },
        ] {
            assert!(validate_write(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn write_endpoint_keeps_the_path_prefix_and_escapes_the_id() {
        assert_eq!(
            write_endpoint("https://example.com/piep", "fanbox", "a/b c").unwrap().as_str(),
            "https://example.com/piep/api/v1/reading/fanbox/a%2Fb%20c",
        );
        assert_eq!(
            write_endpoint("https://192.168.1.2:38777", "pixiv", "123").unwrap().as_str(),
            "https://192.168.1.2:38777/api/v1/reading/pixiv/123",
        );
    }
}
