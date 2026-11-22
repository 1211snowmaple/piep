//! デスクトップ版の側。単独サーバーへの接続と送信。
//!
//! 設定は Rust の側で持つ。画面を開いていないときの自動送信が読むからである。
//! 管理者の鍵は Windows の資格情報マネージャーに置き、設定ファイルには書かない。

use super::store::Role;
use super::sync_send::{
    admin_get, admin_get_if_present, admin_post, admin_put, pair_admin, RemoteServer, SendHandle,
    SendProgress, CANCELED,
};
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;

/// ダウンロードの途中で何度も送らないよう、変わってからこれだけ静かになるのを待つ。
const AUTO_SEND_QUIET: Duration = Duration::from_secs(5 * 60);
const AUTO_SEND_POLL: Duration = Duration::from_secs(30);
/// 起動直後の検索や修復と競わないように待つ。
const AUTO_SEND_STARTUP_DELAY: Duration = Duration::from_secs(2 * 60);
/// 失敗のあとは間を置く。送るたびに本棚の写しを作るので、すぐには繰り返さない。
const AUTO_SEND_RETRY: Duration = Duration::from_secs(15 * 60);
/// 携帯で変えたお気に入りを、単独サーバーへ聞きに行く間隔。
const FAVORITES_POLL: Duration = Duration::from_secs(60);
/// 一度に受け取る携帯のお気に入りの数。
/// サーバーの棚の読書位置を一度に読む数（サーバーの上限）。
const SHELF_READING_PAGE: usize = 500;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct LastSend {
    pub finished_at: Option<String>,
    pub works: Option<i64>,
    pub files_added: Option<i64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct DesktopServerSettings {
    pub remote: Option<RemoteServer>,
    pub auto_send: bool,
    pub last_send: LastSend,
    /// 単独サーバーの棚から、携帯の読書位置をどこまで取り込んだか（サーバーの印）。
    pub shelf_reading_cursor: String,
    /// 単独サーバーの棚から、携帯のお気に入りをどこまで取り込んだか（サーバーの印）。
    pub shelf_favorites_cursor: String,
    /// サーバーが配っている棚の写しの、変更の記録の最後の番号（0 は分からない）。これより後に
    /// この棚で変えた作品だけを、携帯の変更との食い違いとして扱う。
    pub shelf_shipped_sequence: i64,
    /// 送ったがまだ配られていない写しの番号（0 は無い）。サーバーが別の写しを配り始めたら
    /// （印の同期 epoch が `shelf_pending_from` から変わったら）`shelf_shipped_sequence` にする。
    pub shelf_pending_sequence: i64,
    /// 送ったときに配っていた写しの同期 epoch（配っていなければ空）。
    pub shelf_pending_from: String,
}

/// サーバーの棚のお気に入りの一ページ。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShelfFavoritesPage {
    library_id: crate::library::LibraryId,
    items: Vec<ShelfFavoriteItem>,
    cursor: String,
    more: bool,
}

#[derive(Debug, Deserialize)]
struct ShelfFavoriteItem {
    work: crate::library::LibraryId,
    favorite: bool,
}

/// サーバーの棚の読書位置の一ページ。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShelfReadingPage {
    library_id: crate::library::LibraryId,
    items: Vec<ShelfReadingItem>,
    cursor: String,
    more: bool,
}

#[derive(Debug, Deserialize)]
struct ShelfReadingItem {
    work: crate::library::LibraryId,
    position: crate::library::ReadingPosition,
}

/// 送信を頼んだ結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendAttempt {
    /// 別の送信が動いていて、何もしなかった。
    Busy,
    /// 利用者が止めた。
    Canceled,
    /// 最後まで送った。`generation` は写しを作る前のライブラリの世代である。
    Completed { generation: Option<i64> },
}

/// 自動送信がどこまで届けたか。実際に送った世代だけを覚える。
#[derive(Debug)]
struct AutoSendProgress {
    sent_generation: Option<i64>,
    pending_startup_send: bool,
}

impl Default for AutoSendProgress {
    fn default() -> Self {
        Self {
            sent_generation: None,
            pending_startup_send: true,
        }
    }
}

impl AutoSendProgress {
    fn should_send(&self, quiet: bool, generation: Option<i64>) -> bool {
        self.pending_startup_send || (quiet && generation != self.sent_generation)
    }

    fn after_attempt(&mut self, attempt: SendAttempt, current: Option<i64>) {
        match attempt {
            // 動いている送信は今の世代を含まないかもしれない。送ったことにせず、次の周回で試す。
            SendAttempt::Busy => {}
            // 止めた世代を送り続けない。次に変わったら、また送る。
            SendAttempt::Canceled => {
                self.pending_startup_send = false;
                self.sent_generation = current;
            }
            SendAttempt::Completed { generation } => {
                self.pending_startup_send = false;
                self.sent_generation = generation;
            }
        }
    }

    fn after_failure(&mut self) {
        self.pending_startup_send = false;
    }
}

/// 携帯がお気に入りを変えたと画面へ知らせる係。
pub type FavoritesListener = Arc<dyn Fn() + Send + Sync>;

pub struct DesktopServer {
    dir: PathBuf,
    settings: Mutex<DesktopServerSettings>,
    send: SendHandle,
    send_lock: AsyncMutex<()>,
    /// 携帯がお気に入りを変えたと画面へ知らせる係。起動の途中で付ける。
    /// Tauri の型を持たせると、試験の実行ファイルにまで画面の部品がつながるので、関数で受け取る。
    favorites_listener: OnceLock<FavoritesListener>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopServerStatus {
    pub remote: Option<RemoteServer>,
    pub auto_send: bool,
    pub send: SendProgress,
    pub last_send: LastSend,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingDetails {
    pub payload: String,
    pub expires_in_seconds: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairingText {
    version: u32,
    role: Role,
    url: String,
    secret: String,
    fingerprint: Option<String>,
}

fn save_settings(path: &Path, settings: &DesktopServerSettings) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|error| format!("設定を記録できません: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, bytes).map_err(|error| format!("設定を記録できません: {error}"))?;
    fs::rename(&temporary, path).map_err(|error| format!("設定を確定できません: {error}"))
}

mod credential {
    //! 管理者の鍵は Windows の資格情報マネージャーに、サーバーの URL ごとに置く。
    use crate::credential_manager;

    /// サーバーの URL ごとの名前（大文字・小文字と末尾の `/` は区別しない）。
    pub(super) fn name(url: &str) -> String {
        format!(
            "{}/server-admin-token/{}",
            credential_manager::PREFIX,
            url.trim().trim_end_matches('/').to_ascii_lowercase()
        )
    }

    pub fn store(url: &str, token: &str) -> Result<(), String> {
        credential_manager::write(&name(url), token.as_bytes())
            .map_err(|_| "サーバーの鍵を資格情報マネージャーに保存できません".to_string())
    }

    pub fn load(url: &str) -> Result<Option<String>, String> {
        let Some(bytes) = credential_manager::read(&name(url))
            .map_err(|_| "サーバーの鍵を資格情報マネージャーから読めません".to_string())?
        else {
            return Ok(None);
        };
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| "保存されたサーバーの鍵を読み取れません".to_string())
    }

    pub fn delete(url: &str) -> Result<(), String> {
        credential_manager::delete(&name(url))
            .map_err(|_| "サーバーの鍵を資格情報マネージャーから消せません".to_string())
    }
}

impl DesktopServer {
    pub fn new(app_data: &Path) -> Result<Self, String> {
        let dir = app_data.join("server");
        fs::create_dir_all(&dir)
            .map_err(|error| format!("サーバーの置き場を作れません: {error}"))?;
        forget_embedded_server(&dir);
        let settings = fs::read(dir.join("settings.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Ok(Self {
            dir,
            settings: Mutex::new(settings),
            send: SendHandle::new(),
            send_lock: AsyncMutex::new(()),
            favorites_listener: OnceLock::new(),
        })
    }

    /// 画面へ知らせる係を付ける。
    pub fn set_favorites_listener(&self, listener: FavoritesListener) {
        let _ = self.favorites_listener.set(listener);
    }

    fn notify_favorites(&self) {
        if let Some(listener) = self.favorites_listener.get() {
            listener();
        }
    }

    fn settings(&self) -> DesktopServerSettings {
        self.settings
            .lock()
            .map(|settings| settings.clone())
            .unwrap_or_default()
    }

    fn change_settings(
        &self,
        change: impl FnOnce(&mut DesktopServerSettings),
    ) -> Result<(), String> {
        let mut settings = self.settings.lock().map_err(|_| "設定を開けません")?;
        change(&mut settings);
        save_settings(&self.dir.join("settings.json"), &settings)
    }

    pub async fn status(&self) -> DesktopServerStatus {
        let settings = self.settings();
        DesktopServerStatus {
            remote: settings.remote,
            auto_send: settings.auto_send,
            send: self
                .send
                .progress
                .lock()
                .map(|progress| progress.clone())
                .unwrap_or_default(),
            last_send: settings.last_send,
        }
    }

    /// 携帯の登録文字列を、つないでいる単独サーバーで発行する。
    pub async fn create_reader_pairing(&self) -> Result<PairingDetails, String> {
        let remote = self
            .settings()
            .remote
            .ok_or("単独サーバーにつないでいません")?;
        let token = credential::load(&remote.url)?
            .ok_or("サーバーの鍵がありません。登録し直してください")?;
        let body = admin_post(
            &remote,
            &token,
            "pairings",
            &serde_json::json!({ "url": remote.url }),
        )
        .await?;
        let payload = body
            .get("payload")
            .and_then(|value| value.as_str())
            .ok_or("サーバーの応答を読み取れません")?
            .to_string();
        let expires_in_seconds = body
            .get("expiresInSeconds")
            .and_then(|value| value.as_u64())
            .unwrap_or(600);
        Ok(PairingDetails {
            payload,
            expires_in_seconds,
        })
    }
    pub async fn list_remote_devices(&self) -> Result<Vec<serde_json::Value>, String> {
        let remote = self
            .settings()
            .remote
            .ok_or("単独サーバーにつないでいません")?;
        let token = credential::load(&remote.url)?
            .ok_or("サーバーの鍵がありません。登録し直してください")?;
        let body = admin_get(&remote, &token, "devices").await?;
        Ok(body.as_array().cloned().unwrap_or_default())
    }

    pub async fn remote_status(&self) -> Result<serde_json::Value, String> {
        let remote = self
            .settings()
            .remote
            .ok_or("単独サーバーにつないでいません")?;
        let token = credential::load(&remote.url)?
            .ok_or("サーバーの鍵がありません。登録し直してください")?;
        admin_get(&remote, &token, "status").await
    }

    /// デスクトップ版の方が新しければ、サーバーにも更新を頼む。
    async fn update_remote_if_behind(&self) {
        let Ok(status) = self.remote_status().await else {
            return;
        };
        let behind = status["version"]
            .as_str()
            .is_some_and(|version| super::update::is_newer(env!("CARGO_PKG_VERSION"), version));
        // 自動更新を切っているサーバーは、押されるまで変えない。
        if behind && status["update"]["auto"].as_bool() == Some(true) {
            if let Err(error) = self.update_remote().await {
                log::warn!("サーバーの更新を頼めません: {error}");
            }
        }
    }

    /// 新しいバージョンがあるかだけを確かめてもらう。当てはしない。
    pub async fn check_remote_update(&self) -> Result<serde_json::Value, String> {
        let remote = self
            .settings()
            .remote
            .ok_or("単独サーバーにつないでいません")?;
        let token = credential::load(&remote.url)?
            .ok_or("サーバーの鍵がありません。登録し直してください")?;
        admin_post(&remote, &token, "update/check", &serde_json::json!({})).await
    }

    /// 管理画面へ入る、一度きりのリンクを作る。接続先の URL で開くので、その名前が覚えられる。
    pub async fn admin_link(&self) -> Result<String, String> {
        let remote = self
            .settings()
            .remote
            .ok_or("単独サーバーにつないでいません")?;
        let token = credential::load(&remote.url)?
            .ok_or("サーバーの鍵がありません。登録し直してください")?;
        let ticket = admin_post(&remote, &token, "tickets", &serde_json::json!({})).await?;
        let path = ticket["path"]
            .as_str()
            .ok_or("サーバーの応答を読めません")?;
        Ok(format!("{}{path}", remote.url.trim_end_matches('/')))
    }

    pub async fn update_remote(&self) -> Result<(), String> {
        let remote = self
            .settings()
            .remote
            .ok_or("単独サーバーにつないでいません")?;
        let token = credential::load(&remote.url)?
            .ok_or("サーバーの鍵がありません。登録し直してください")?;
        admin_post(&remote, &token, "update", &serde_json::json!({})).await?;
        Ok(())
    }

    /// 単独サーバーの携帯に、棚を読む許可を付け外しする。
    pub async fn set_remote_shelf_reader(&self, device_id: &str, read: bool) -> Result<(), String> {
        if device_id.is_empty()
            || !device_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            return Err("端末を指定し直してください".into());
        }
        let remote = self
            .settings()
            .remote
            .ok_or("単独サーバーにつないでいません")?;
        let token = credential::load(&remote.url)?
            .ok_or("サーバーの鍵がありません。登録し直してください")?;
        admin_put(
            &remote,
            &token,
            &format!("devices/{device_id}/shelf"),
            &serde_json::json!({ "read": read }),
        )
        .await
    }

    pub async fn connect_remote(&self, pairing: &str, device_name: &str) -> Result<(), String> {
        let parsed: PairingText = serde_json::from_str(pairing.trim())
            .map_err(|_| "登録文字列を読み取れません".to_string())?;
        if parsed.version != 2 {
            return Err("この版の登録文字列には対応していません".into());
        }
        if parsed.role != Role::Admin {
            return Err("これは携帯用の登録文字列です。サーバーの piep-server init か admin-pair の出力を貼ってください".into());
        }
        let remote = RemoteServer {
            url: parsed.url,
            fingerprint: parsed.fingerprint,
        };
        let token = pair_admin(&remote, &parsed.secret, device_name).await?;
        credential::store(&remote.url, &token)?;
        self.change_settings(|settings| {
            settings.remote = Some(remote);
            settings.last_send = LastSend::default();
            // 別のサーバーの記録の番号は、このサーバーでは意味を持たない。
            settings.shelf_reading_cursor.clear();
            settings.shelf_favorites_cursor.clear();
            settings.shelf_shipped_sequence = 0;
            settings.shelf_pending_sequence = 0;
            settings.shelf_pending_from.clear();
        })
    }

    pub async fn disconnect_remote(&self) -> Result<(), String> {
        self.send.cancel.store(true, Ordering::Relaxed);
        let _running = self.send_lock.lock().await;
        if let Some(remote) = self.settings().remote {
            credential::delete(&remote.url)?;
        }
        self.change_settings(|settings| {
            settings.remote = None;
            settings.auto_send = false;
            settings.last_send = LastSend::default();
            settings.shelf_reading_cursor.clear();
            settings.shelf_favorites_cursor.clear();
            settings.shelf_shipped_sequence = 0;
            settings.shelf_pending_sequence = 0;
            settings.shelf_pending_from.clear();
        })
    }

    pub fn set_auto_send(&self, enabled: bool) -> Result<(), String> {
        self.change_settings(|settings| settings.auto_send = enabled)
    }

    pub fn stop_send(&self) {
        self.send.cancel.store(true, Ordering::Relaxed);
    }

    /// 携帯で変えたお気に入りと読書位置を単独サーバーの棚から受け取り、棚へ書く。送信の途中なら何もしない。
    pub async fn pull_favorites(&self, state: &Arc<AppState>) -> Result<usize, String> {
        let Ok(_running) = self.send_lock.try_lock() else {
            return Ok(0);
        };
        self.pull_favorites_locked(state).await
    }

    /// 送信の鍵を持ったまま呼ぶ。
    ///
    /// 写しを作る前に取り込めば、取り込んだ変更は写しに入る。送信の途中に取り込むと、
    /// 取り込む前の写しが届いて携帯の変更が消えるので、送信と同時には動かさない。
    async fn pull_favorites_locked(&self, state: &Arc<AppState>) -> Result<usize, String> {
        // 携帯が棚に書いたお気に入りを読む。
        let applied = match self.pull_shelf_favorites_locked(state).await {
            Ok(applied) => applied.unwrap_or(0),
            Err(error) => {
                log::warn!("携帯のお気に入りを棚から取り込めません: {error}");
                0
            }
        };
        // 携帯が棚に書いた読書位置も、同じ機会に取り込む。取り込めなくてもお気に入りは止めない。
        let read = match self.pull_shelf_reading_locked(state).await {
            Ok(read) => read,
            Err(error) => {
                log::warn!("携帯の読書位置を取り込めません: {error}");
                0
            }
        };
        if applied > 0 || read > 0 {
            state.shelf.forget_derived();
            self.notify_favorites();
        }
        Ok(applied)
    }

    /// サーバーの印 `cursor` の写しに当てはまる、送った写しの番号。送った写しが配られ始めて
    /// いれば（印の同期 epoch が送ったときと違えば）、それを配っている写しの番号にする。
    fn shipped_for(&self, cursor: &str) -> Result<i64, String> {
        let epoch = cursor.split('.').next().unwrap_or_default();
        let settings = self.settings();
        if settings.shelf_pending_sequence > 0
            && !epoch.is_empty()
            && epoch != settings.shelf_pending_from
        {
            let shipped = settings.shelf_pending_sequence;
            self.change_settings(|settings| {
                settings.shelf_shipped_sequence = shipped;
                settings.shelf_pending_sequence = 0;
                settings.shelf_pending_from.clear();
            })?;
            return Ok(shipped);
        }
        Ok(settings.shelf_shipped_sequence)
    }

    /// 単独サーバーの棚から、携帯が書いた読書位置を棚へ取り込む。
    ///
    /// サーバーの棚は配る写しなので、写しを入れ替える前に取り込んでおけば携帯の位置は消えない。
    /// 一覧は初めに全部、そのあとは変わった分だけ。どちらの位置が新しいかは書いた時刻で決め、
    /// 同じ位置を何度読んでも二度は書かない。印が別の写しのものならサーバーが初めから返す。
    async fn pull_shelf_reading_locked(&self, state: &Arc<AppState>) -> Result<usize, String> {
        let Ok(library) = state.shelf.library() else {
            return Ok(0);
        };
        let Some(remote) = self.settings().remote else {
            return Ok(0);
        };
        let Some(token) = credential::load(&remote.url)? else {
            return Ok(0);
        };
        // 印はサーバーが作る（数字・英字・`-`・`.` だけ）。それ以外なら初めから読み直す。
        let mut cursor = self.settings().shelf_reading_cursor;
        if !cursor
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
        {
            cursor.clear();
        }
        let mut applied = 0;
        loop {
            let path = format!("shelf/reading?cursor={cursor}&limit={SHELF_READING_PAGE}");
            // 棚を配っていないサーバーには口が無い。携帯も棚へ書いていない。
            let Some(page) = admin_get_if_present(&remote, &token, &path).await? else {
                break;
            };
            let page: ShelfReadingPage = serde_json::from_value(page)
                .map_err(|_| "サーバーの棚の読書位置を読み取れません".to_string())?;
            if page.library_id != library.id {
                return Err("サーバーの棚は、この棚の写しではありません".into());
            }
            let shelf = Arc::clone(&library);
            let items = page.items;
            let shipped = self.shipped_for(&page.cursor)?;
            applied += tokio::task::spawn_blocking(move || {
                let mut applied = 0;
                for item in &items {
                    if crate::commands::shelf::adopt_remote_reading(
                        &shelf,
                        item.work,
                        &item.position,
                        shipped,
                    )? {
                        applied += 1;
                    }
                }
                Ok::<_, String>(applied)
            })
            .await
            .map_err(|error| format!("読書位置を書けません: {error}"))??;
            let next = page.cursor;
            self.change_settings(|settings| settings.shelf_reading_cursor = next.clone())?;
            if !page.more || next == cursor {
                break;
            }
            cursor = next;
        }
        Ok(applied)
    }

    /// 単独サーバーの棚から、携帯が付け外ししたお気に入りを棚へ取り込む。
    /// 前に送ってからこの棚でも変えた作品はこの棚の値を残す。棚を配っていないサーバー
    /// （口が無い）なら `None`。
    async fn pull_shelf_favorites_locked(
        &self,
        state: &Arc<AppState>,
    ) -> Result<Option<usize>, String> {
        let Ok(library) = state.shelf.library() else {
            return Ok(None);
        };
        let Some(remote) = self.settings().remote else {
            return Ok(None);
        };
        let Some(token) = credential::load(&remote.url)? else {
            return Ok(None);
        };
        let mut cursor = self.settings().shelf_favorites_cursor;
        if !cursor
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.'))
        {
            cursor.clear();
        }
        let mut applied = 0;
        loop {
            let path = format!("shelf/favorites?cursor={cursor}&limit={SHELF_READING_PAGE}");
            let Some(page) = admin_get_if_present(&remote, &token, &path).await? else {
                return Ok(None);
            };
            let page: ShelfFavoritesPage = serde_json::from_value(page)
                .map_err(|_| "サーバーの棚のお気に入りを読み取れません".to_string())?;
            if page.library_id != library.id {
                return Err("サーバーの棚は、この棚の写しではありません".into());
            }
            let shelf = Arc::clone(&library);
            let items = page.items;
            let shipped = self.shipped_for(&page.cursor)?;
            applied += tokio::task::spawn_blocking(move || {
                let mut applied = 0;
                for item in &items {
                    if crate::commands::shelf::adopt_remote_favorite(
                        &shelf,
                        item.work,
                        item.favorite,
                        shipped,
                    )? {
                        applied += 1;
                    }
                }
                Ok::<_, String>(applied)
            })
            .await
            .map_err(|error| format!("お気に入りを書けません: {error}"))??;
            let next = page.cursor;
            self.change_settings(|settings| settings.shelf_favorites_cursor = next.clone())?;
            if !page.more || next == cursor {
                break;
            }
            cursor = next;
        }
        Ok(Some(applied))
    }

    /// 送信を 1 回行う。すでに送っていれば何もしない。
    pub async fn send_once(&self, state: Arc<AppState>) -> Result<SendAttempt, String> {
        let Ok(_running) = self.send_lock.try_lock() else {
            return Ok(SendAttempt::Busy);
        };
        // 写しに入れるため、携帯の変更を先に取り込む。取り込めなくても送信は止めない。
        if let Err(error) = self.pull_favorites_locked(&state).await {
            log::warn!("携帯のお気に入りを取り込めません: {error}");
        }
        let library = state.shelf.library()?;
        // 写しを作る前に読む。この間に変わっても、次回もう一度送るだけで取りこぼさない。
        let generation = change_generation(&state);
        let remote = self
            .settings()
            .remote
            .ok_or("単独サーバーにつないでいません")?;
        let token = credential::load(&remote.url)?
            .ok_or("サーバーの鍵がありません。登録し直してください")?;
        self.send.cancel.store(false, Ordering::Relaxed);
        if let Ok(mut progress) = self.send.progress.lock() {
            *progress = SendProgress {
                running: true,
                phase: "snapshot".into(),
                ..Default::default()
            };
        }
        let finished_at = Some(chrono::Utc::now().to_rfc3339());
        let result = {
            let outgoing = self.dir.join("outgoing").join("shelf");
            // 送る前に読んだ印の写しが、送ったときに配っていた写し。
            let from = self.settings().shelf_reading_cursor;
            let from = from.split('.').next().unwrap_or_default().to_string();
            match super::shelf_send::send_shelf(&remote, &token, library, &outgoing, &self.send)
                .await
            {
                Ok(sent) => self
                    .change_settings(|settings| {
                        settings.shelf_pending_sequence = sent.shipped_sequence;
                        settings.shelf_pending_from = from.clone();
                    })
                    .map(|()| LastSend {
                        finished_at: finished_at.clone(),
                        works: sent.works.map(|works| works as i64),
                        files_added: Some(sent.sent as i64),
                        ..Default::default()
                    }),
                Err(error) => Err(error),
            }
        };
        let finished_at = Some(chrono::Utc::now().to_rfc3339());
        let (phase, record) = match result {
            Ok(record) => (
                "done",
                LastSend {
                    finished_at,
                    ..record
                },
            ),
            Err(error) if error == CANCELED => (
                "canceled",
                LastSend {
                    finished_at,
                    error: Some(error),
                    ..Default::default()
                },
            ),
            Err(error) => {
                log::warn!("単独サーバーへの送信に失敗しました: {error}");
                (
                    "failed",
                    LastSend {
                        finished_at,
                        error: Some(error),
                        ..Default::default()
                    },
                )
            }
        };
        if let Ok(mut progress) = self.send.progress.lock() {
            progress.running = false;
            progress.phase = phase.into();
            progress.message = record.error.clone();
        }
        let failed = record.error.clone();
        self.change_settings(|settings| settings.last_send = record)?;
        if phase == "done" {
            self.update_remote_if_behind().await;
        }
        match failed {
            Some(error) if error == CANCELED => Ok(SendAttempt::Canceled),
            Some(error) => Err(error),
            None => Ok(SendAttempt::Completed { generation }),
        }
    }

    pub fn start_background(self: &Arc<Self>, state: Arc<AppState>) {
        let server = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(AUTO_SEND_STARTUP_DELAY).await;
            // 送るものがなくても、デスクトップ版が新しくなったらサーバーを追いつかせる。
            if server.settings().remote.is_some() {
                server.update_remote_if_behind().await;
            }
            // 起動して落ち着いたあとに一度送る。変わっていなければ問い合わせだけで終わる。
            let mut progress = AutoSendProgress::default();
            let mut seen = None;
            let mut changed_at = None;
            let mut retry_at: Option<Instant> = None;
            let mut favorites_at: Option<Instant> = None;
            loop {
                // 自動送信を切っていても、携帯の変更はデスクトップ版へ届ける。
                if server.settings().remote.is_some()
                    && favorites_at.is_none_or(|at| at.elapsed() >= FAVORITES_POLL)
                {
                    favorites_at = Some(Instant::now());
                    if let Err(error) = server.pull_favorites(&state).await {
                        log::debug!("携帯のお気に入りを取り込めません: {error}");
                    }
                }
                if server.settings().auto_send && server.settings().remote.is_some() {
                    let generation = change_generation(&state);
                    if generation != seen {
                        seen = generation;
                        changed_at = Some(Instant::now());
                    }
                    let quiet =
                        changed_at.is_none_or(|at: Instant| at.elapsed() >= AUTO_SEND_QUIET);
                    let waited = retry_at.is_none_or(|at| Instant::now() >= at);
                    if waited && progress.should_send(quiet, generation) {
                        match server.send_once(Arc::clone(&state)).await {
                            Ok(attempt) => {
                                progress.after_attempt(attempt, generation);
                                retry_at = None;
                            }
                            Err(error) => {
                                progress.after_failure();
                                log::warn!("自動送信に失敗しました: {error}");
                                retry_at = Some(Instant::now() + AUTO_SEND_RETRY);
                            }
                        }
                    }
                }
                tokio::time::sleep(AUTO_SEND_POLL).await;
            }
        });
    }
}

/// 0.40 まであった内蔵サーバーの登録・証明書を消す。設定の `embedded` は読むときに捨てる。
fn forget_embedded_server(dir: &Path) {
    for name in ["state.db", "state.db-wal", "state.db-shm"] {
        let _ = fs::remove_file(dir.join(name));
    }
    let _ = fs::remove_dir_all(dir.join("tls"));
}

/// 送るべき変更があったかを見る印。
fn change_generation(state: &AppState) -> Option<i64> {
    state.shelf.library().ok()?.change_marker().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_server_keeps_its_own_key_name() {
        assert_eq!(
            credential::name("https://Maple.example:8443/"),
            credential::name("https://maple.example:8443")
        );
        assert_ne!(
            credential::name("https://maple.example:8443"),
            credential::name("https://127.0.0.1:8443")
        );
        assert!(credential::name("https://a")
            .starts_with("io.github.u1211snowmaple.piep/server-admin-token/"));
    }

    #[test]
    fn the_shipped_number_applies_once_the_server_serves_another_copy() {
        let dir = std::env::temp_dir().join(format!("piep-desktop-{}", uuid::Uuid::now_v7()));
        let server = DesktopServer::new(&dir).unwrap();
        server
            .change_settings(|settings| {
                settings.shelf_shipped_sequence = 5;
                settings.shelf_pending_sequence = 9;
                settings.shelf_pending_from = "old".into();
            })
            .unwrap();
        // 送ったときと同じ写しを配っている間は、前の番号のまま。
        assert_eq!(server.shipped_for("old.s.3").unwrap(), 5);
        assert_eq!(server.shipped_for("").unwrap(), 5);
        // 別の写しを配り始めたら、送った番号にする。
        assert_eq!(server.shipped_for("new.f.0").unwrap(), 9);
        let settings = server.settings();
        assert_eq!(
            (
                settings.shelf_shipped_sequence,
                settings.shelf_pending_sequence
            ),
            (9, 0)
        );
        assert_eq!(server.shipped_for("newer.s.1").unwrap(), 9);
        drop(server);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_busy_attempt_does_not_count_as_sent() {
        let mut progress = AutoSendProgress::default();
        assert!(progress.should_send(false, Some(1)));
        // 手で始めた g1 の送信中に g2 へ変わり、自動送信はロックで見送られる。
        progress.after_attempt(SendAttempt::Busy, Some(2));
        assert!(progress.should_send(true, Some(2)));
        progress.after_attempt(
            SendAttempt::Completed {
                generation: Some(2),
            },
            Some(2),
        );
        assert!(!progress.should_send(true, Some(2)));
    }

    #[test]
    fn the_generation_written_during_a_send_is_still_owed() {
        let mut progress = AutoSendProgress::default();
        // g1 の写しを送っている間に g2 になった。
        progress.after_attempt(
            SendAttempt::Completed {
                generation: Some(1),
            },
            Some(2),
        );
        assert!(progress.should_send(true, Some(2)));
    }

    #[test]
    fn a_canceled_generation_waits_for_the_next_change() {
        let mut progress = AutoSendProgress::default();
        progress.after_attempt(SendAttempt::Canceled, Some(3));
        assert!(!progress.should_send(true, Some(3)));
        assert!(progress.should_send(true, Some(4)));
    }

    #[test]
    fn a_failure_keeps_the_generation_owed() {
        let mut progress = AutoSendProgress::default();
        progress.after_failure();
        assert!(!progress.pending_startup_send);
        assert!(progress.should_send(true, Some(1)));
    }
}
