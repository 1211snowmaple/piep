//! 新しい棚をアプリから読み取り専用で開く（評価文書 4.1 の 2）。
//!
//! 旧棚が正本のあいだ、新しい棚は旧棚から移行して作った写しで、アプリは読むだけにする。
//! 書き込みの命令は置かない（二重書き込みをしない）。棚は `PIEP_LIBRARY_V2_DIR` か、
//! アプリのフォルダーの `library-v2` にあれば開き、なければ何もしない（画面には何も出さない）。
//!
//! 開くのと索引の同期は起動の門（旧棚の準備）と別のスレッドで進める。全棚では索引を
//! 初めて作るのに数分かかるので、旧棚の画面を待たせない。
//!
//! 画像（表紙・本文の画像）は `piep-library` の URI で、ファイルの識別子（中身の SHA-256）
//! から返す。画面は `convertFileSrc(識別子, "piep-library")` で URL にする。

use crate::library::{Fingerprint, Library, LibraryId, WorkCard, WorkListQuery};
use crate::library_search::{SearchDecoration, SearchMatch, SearchPage, TextSearch};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::State;

pub const DIR_ENV: &str = "PIEP_LIBRARY_V2_DIR";
pub const DEFAULT_DIR: &str = "library-v2";
pub const URI_SCHEME: &str = "piep-library";

/// 新しい棚の状態。
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum LibraryV2Status {
    /// まだ探していない（起動の直後）。
    Starting,
    /// 棚がない。
    Absent {
        path: PathBuf,
    },
    Opening {
        path: PathBuf,
    },
    /// 開いて、本文の検索の索引を同期している。一覧と作品は読めるが、本文の検索はまだ。
    Indexing {
        path: PathBuf,
    },
    Ready {
        path: PathBuf,
        /// 移行の未解決の記録。0 でなければ、切り替えに使える棚ではない。
        unresolved: Vec<String>,
    },
    Failed {
        path: PathBuf,
        message: String,
    },
}

pub struct LibraryV2 {
    status: Mutex<LibraryV2Status>,
    library: OnceLock<Arc<Library>>,
}

impl Default for LibraryV2 {
    fn default() -> Self {
        Self {
            status: Mutex::new(LibraryV2Status::Starting),
            library: OnceLock::new(),
        }
    }
}

impl LibraryV2 {
    fn set(&self, status: LibraryV2Status) {
        if let Ok(mut slot) = self.status.lock() {
            *slot = status;
        }
    }

    pub fn status(&self) -> LibraryV2Status {
        self.status
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or(LibraryV2Status::Starting)
    }

    /// 開いた棚。開いていなければ誤り。
    pub fn library(&self) -> Result<Arc<Library>, String> {
        self.library
            .get()
            .cloned()
            .ok_or_else(|| "新しい棚は開いていません".to_string())
    }

    /// 本文の検索ができる棚。索引の同期が済むまでは誤り。
    fn searchable(&self) -> Result<Arc<Library>, String> {
        match self.status() {
            LibraryV2Status::Ready { .. } => self.library(),
            LibraryV2Status::Indexing { .. } => {
                Err("新しい棚の検索の索引を準備しています".to_string())
            }
            _ => Err("新しい棚は開いていません".to_string()),
        }
    }

    /// 棚を探して開き、本文の検索の索引を同期する。呼んだスレッドで最後まで進む。
    pub fn open(&self, path: &Path) {
        if !path.join("library.json").is_file() {
            self.set(LibraryV2Status::Absent { path: path.into() });
            return;
        }
        self.set(LibraryV2Status::Opening { path: path.into() });
        let failed = |message: String| LibraryV2Status::Failed {
            path: path.into(),
            message,
        };
        let library = match Library::open(path) {
            Ok(library) => Arc::new(library),
            Err(error) => {
                log::error!("新しい棚を開けません: {error}");
                self.set(failed(error.to_string()));
                return;
            }
        };
        let library = Arc::clone(self.library.get_or_init(|| library));
        self.set(LibraryV2Status::Indexing { path: path.into() });
        let started = std::time::Instant::now();
        match library.sync_search_index(&mut |_, _| {}) {
            Ok(report) => log::info!(
                "新しい棚の索引を同期しました: {} ms, {report:?}",
                started.elapsed().as_millis()
            ),
            Err(error) => {
                log::error!("新しい棚の索引を同期できません: {error}");
                self.set(failed(error.to_string()));
                return;
            }
        }
        let unresolved = library
            .activation_problems()
            .unwrap_or_else(|error| vec![format!("未解決の記録を読めません: {error}")]);
        self.set(LibraryV2Status::Ready {
            path: path.into(),
            unresolved,
        });
    }
}

/// 棚の場所。環境変数があればそれ、なければアプリのフォルダーの `library-v2`。
pub fn library_dir(app_data: &Path) -> PathBuf {
    std::env::var_os(DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| app_data.join(DEFAULT_DIR))
}

/// 起動のときに呼ぶ。別のスレッドで開く。
pub fn open_in_background(state: Arc<LibraryV2>, app_data: &Path) {
    let path = library_dir(app_data);
    let spawned = std::thread::Builder::new()
        .name("piep-library-v2".into())
        .spawn(move || state.open(&path));
    if let Err(error) = spawned {
        log::error!("新しい棚を開くスレッドを作れません: {error}");
    }
}

/// `piep-library` の URI の要求に答える。パスはファイルの識別子（64 桁の 16 進）。
pub fn respond(state: &LibraryV2, path: &str) -> tauri::http::Response<Vec<u8>> {
    let respond = |status: u16, body: Vec<u8>, mime: Option<&str>| {
        let mut builder = tauri::http::Response::builder()
            .status(status)
            // 中身の SHA-256 で指すので、同じ URL の中身は変わらない。
            .header("Cache-Control", "private, max-age=31536000, immutable");
        if let Some(mime) = mime {
            builder = builder.header("Content-Type", mime);
        }
        builder
            .body(body)
            .unwrap_or_else(|_| tauri::http::Response::new(Vec::new()))
    };
    let id = path.trim_start_matches('/');
    let Ok(id) = Fingerprint::parse(id) else {
        return respond(400, Vec::new(), None);
    };
    let Ok(library) = state.library() else {
        return respond(503, Vec::new(), None);
    };
    let file = match library.file(id) {
        Ok(file) => file,
        Err(_) => return respond(404, Vec::new(), None),
    };
    let mut bytes = Vec::with_capacity(file.length as usize);
    if let Err(error) = library.objects.export(&file, &mut bytes) {
        log::error!("新しい棚のファイルを読めません: {id}: {error}");
        return respond(500, Vec::new(), None);
    }
    let mime = sniff(&bytes);
    respond(200, bytes, mime)
}

/// 画像の種類を先頭のバイト列から決める。棚のファイルは種類を持たない（中身で指す）。
/// 分からなければ付けない（WebView に推測させず、画像としては表示されない）。
fn sniff(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        [_, _, _, _, b'f', b't', b'y', b'p', b'a', b'v', b'i', b'f', ..] => Some("image/avif"),
        [b'B', b'M', ..] => Some("image/bmp"),
        _ => None,
    }
}

#[tauri::command]
pub fn library_v2_status(state: State<'_, Arc<LibraryV2>>) -> LibraryV2Status {
    state.status()
}

/// 一覧の一件と、画面が表紙を引くための識別子。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CardView {
    #[serde(flatten)]
    pub card: WorkCard,
    /// 表紙のファイルの識別子（16 進）。`convertFileSrc(…, "piep-library")` で URL になる。
    pub cover_file: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchView {
    #[serde(flatten)]
    pub page: SearchPage,
    /// `page.page.items` と同じ順。
    pub cards: Vec<CardView>,
}

fn card_view(card: &WorkCard) -> CardView {
    CardView {
        card: card.clone(),
        cover_file: card.cover.map(|cover| cover.to_string()),
    }
}

/// 一覧・本文の検索。語がなければ一覧（`tag:` などの書き方だけでもよい）。
#[tauri::command]
pub async fn library_v2_search(
    state: State<'_, Arc<LibraryV2>>,
    search: TextSearch,
    query: WorkListQuery,
) -> Result<SearchView, String> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let library = if search.text.trim().is_empty() {
            state.library()?
        } else {
            state.searchable()?
        };
        let page = library
            .search_works(&search, &query)
            .map_err(|e| e.to_string())?;
        let cards = page.page.items.iter().map(card_view).collect();
        Ok(SearchView { page, cards })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 一ページぶんの一致した箇所と理由。
#[tauri::command]
pub async fn library_v2_decorations(
    state: State<'_, Arc<LibraryV2>>,
    text: String,
    matches: Vec<SearchMatch>,
) -> Result<Vec<SearchDecoration>, String> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        state
            .searchable()?
            .search_decorations(&text, &matches)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 作品一件の一覧の項目。
#[tauri::command]
pub async fn library_v2_work_card(
    state: State<'_, Arc<LibraryV2>>,
    work: LibraryId,
) -> Result<CardView, String> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let card = state
            .library()?
            .work_card(work)
            .map_err(|e| e.to_string())?;
        Ok(card_view(&card))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
#[path = "library_v2_tests.rs"]
mod tests;
