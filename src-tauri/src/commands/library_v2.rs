//! 新しい棚をアプリから読み取り専用で開く（評価文書 4.1 の 2）。
//!
//! 旧棚が正本のあいだ、新しい棚は旧棚から移行して作った写しで、アプリは読むだけにする。
//! 書き込みの命令は置かない（二重書き込みをしない）。棚は `PIEP_LIBRARY_V2_DIR` か、
//! アプリのフォルダーの隣の `….library-v2` にあれば開き、なければ何もしない（画面には何も出さない）。
//!
//! 開くのと索引の同期は起動の門（旧棚の準備）と別のスレッドで進める。全棚では索引を
//! 初めて作るのに数分かかるので、旧棚の画面を待たせない。
//!
//! 画像（表紙・本文の画像）は `piep-library` の URI で、ファイルの識別子（中身の SHA-256）
//! から返す。画面は `convertFileSrc(識別子, "piep-library")` で URL にする。

use crate::database::models::{ReaderOutlineEntry, ReaderSearchHit};
use crate::database::queries::{reader_outline, search_reader_pages};
use crate::library::{
    Fingerprint, Library, LibraryId, NeighborKind, ResourceScope, WorkCard, WorkListQuery,
};
use crate::library_search::{parse_query, SearchDecoration, SearchMatch, SearchPage, TextSearch};
use crate::library_view::{MediaView, RenderedDocument, VersionView};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
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
    /// 最近ページに分けた本文。ページをめくるたびに本文全体を組み直さない。
    documents: Mutex<VecDeque<CachedDocument>>,
    /// 作品の旧棚での番号。読むだけの棚は開いているあいだ変わらないので、一度だけ引く。
    legacy_numbers: Mutex<Option<Arc<HashMap<LibraryId, i64>>>>,
}

type CachedDocument = (LibraryId, ResourceScope, Arc<RenderedDocument>);

/// ページに分けた本文を覚えておく数。作品の画面と読む画面は一度に一作品で、版を見比べても数冊。
const DOCUMENT_CACHE: usize = 4;

impl Default for LibraryV2 {
    fn default() -> Self {
        Self {
            status: Mutex::new(LibraryV2Status::Starting),
            library: OnceLock::new(),
            documents: Mutex::new(VecDeque::new()),
            legacy_numbers: Mutex::new(None),
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

    /// 作品の旧棚での番号（移行の記録）。棚にある作品は約 3 万件なので、全部を一度に持つ。
    fn legacy_numbers(&self, library: &Library) -> Result<Arc<HashMap<LibraryId, i64>>, String> {
        let mut slot = self.legacy_numbers.lock().map_err(|e| e.to_string())?;
        if let Some(numbers) = slot.as_ref() {
            return Ok(Arc::clone(numbers));
        }
        let numbers = Arc::new(library.legacy_work_numbers().map_err(|e| e.to_string())?);
        *slot = Some(Arc::clone(&numbers));
        Ok(numbers)
    }

    /// 一覧の一件を、画面が読む形にする。
    fn card_view(&self, library: &Library, card: &WorkCard) -> Result<CardView, String> {
        Ok(CardView {
            card: card.clone(),
            cover_file: card.cover.map(|cover| cover.to_string()),
            legacy_id: self.legacy_numbers(library)?.get(&card.id).copied(),
        })
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

    /// 版（または有効な編集）の本文をページに分けたもの。最近のものは覚えておく。
    fn document(
        &self,
        library: &Library,
        work: LibraryId,
        scope: ResourceScope,
        provider: &str,
    ) -> Result<Arc<RenderedDocument>, String> {
        let cached = self.documents.lock().ok().and_then(|documents| {
            documents
                .iter()
                .find(|(w, s, _)| *w == work && *s == scope)
                .map(|(_, _, document)| Arc::clone(document))
        });
        if let Some(document) = cached {
            return Ok(document);
        }
        let document = Arc::new(
            library
                .render_document(work, scope, provider)
                .map_err(|e| e.to_string())?,
        );
        if let Ok(mut documents) = self.documents.lock() {
            documents.retain(|(w, s, _)| !(*w == work && *s == scope));
            documents.push_front((work, scope, Arc::clone(&document)));
            documents.truncate(DOCUMENT_CACHE);
        }
        Ok(document)
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

/// 棚の場所。環境変数があればそれ、なければアプリのフォルダーの隣の `<アプリのフォルダー名>.library-v2`。
///
/// アプリのフォルダーの中には置かない。移行器は旧棚の中への移行を断り（旧棚の全件照合が
/// 新しい棚のファイルを知らないファイルとして数える）、旧棚の書庫やバックアップにも入ってしまう。
pub fn library_dir(app_data: &Path) -> PathBuf {
    std::env::var_os(DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| default_dir(app_data))
}

fn default_dir(app_data: &Path) -> PathBuf {
    let mut name = app_data.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(DEFAULT_DIR);
    app_data.with_file_name(name)
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

/// 新しい棚の状態（ない・開いている・索引の同期中・使える・失敗）。使えるときは移行の
/// 未解決の記録も返す。
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
    /// 旧棚から移した作品の、旧棚での番号。端末の読書位置・しおりはこの番号で引く（旧棚と共有）。
    pub legacy_id: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchView {
    #[serde(flatten)]
    pub page: SearchPage,
    /// `page.page.items` と同じ順。
    pub cards: Vec<CardView>,
}

/// 一覧・本文の検索。語がなければ一覧（`tag:` などの書き方だけでもよい）で、索引の同期中でも読める。
/// `legacy_works` を渡すと、旧棚の番号でその作品だけに絞る（読みかけの棚）。
#[tauri::command]
pub async fn library_v2_search(
    state: State<'_, Arc<LibraryV2>>,
    search: TextSearch,
    mut query: WorkListQuery,
    legacy_works: Option<Vec<i64>>,
) -> Result<SearchView, String> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        // 書き方（`tag:` など）だけで語のない問い合わせは一覧なので、索引の同期中でも読める。
        let library = if parse_query(&search.text).text.trim().is_empty() {
            state.library()?
        } else {
            state.searchable()?
        };
        // 読みかけの棚は端末の読書位置の鍵（旧棚の番号）で来る。棚の作品の ID に読み替えて絞る。
        if let Some(numbers) = legacy_works {
            let wanted: std::collections::HashSet<i64> = numbers.into_iter().collect();
            let known = state.legacy_numbers(&library)?;
            let works = known
                .iter()
                .filter(|(_, number)| wanted.contains(number))
                .map(|(work, _)| *work);
            query.filter.works = Some(match query.filter.works.take() {
                Some(existing) => {
                    let allowed: std::collections::HashSet<LibraryId> = works.collect();
                    existing
                        .into_iter()
                        .filter(|work| allowed.contains(work))
                        .collect()
                }
                None => works.collect(),
            });
        }
        let page = library
            .search_works(&search, &query)
            .map_err(|e| e.to_string())?;
        let cards = page
            .page
            .items
            .iter()
            .map(|card| state.card_view(&library, card))
            .collect::<Result<_, _>>()?;
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

/// 作品の画面の頭と版の一覧。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkView {
    #[serde(flatten)]
    pub card: CardView,
    /// 取得元の説明（HTML のこともある）。なければ抜粋。
    pub caption: Option<String>,
    /// 取得元の作者の ID。作者の画面へのリンクに使う。
    pub author_key: Option<String>,
    /// 取得元のシリーズの ID。
    pub series_key: Option<String>,
    /// 保存した版（古い順）。
    pub versions: Vec<VersionView>,
}

fn text_of(value: Option<&serde_json::Value>) -> Option<String> {
    match value? {
        serde_json::Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// 作品の画面の頭（一覧の項目・説明・作者とシリーズの ID）と、保存した版。
#[tauri::command]
pub async fn library_v2_work(
    state: State<'_, Arc<LibraryV2>>,
    work: LibraryId,
) -> Result<WorkView, String> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let library = state.library()?;
        let card = library.work_card(work).map_err(|e| e.to_string())?;
        let snapshots = library.snapshots(work).map_err(|e| e.to_string())?;
        let metadata = snapshots
            .iter()
            .find(|snapshot| snapshot.id == card.snapshot_id)
            .or(snapshots.last())
            .map(|snapshot| snapshot.metadata.clone())
            .unwrap_or_default();
        let versions = library.work_versions(work).map_err(|e| e.to_string())?;
        Ok(WorkView {
            caption: text_of(metadata.get("caption")).or_else(|| card.excerpt.clone()),
            author_key: text_of(metadata.get("author_key")),
            series_key: text_of(metadata.get("series_id")),
            card: state.card_view(&library, &card)?,
            versions,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 見る保存状態。版を指せばその版、指さなければ今読む本文（有効な編集があればそれ）。
fn scope_of(card: &WorkCard, snapshot: Option<LibraryId>) -> ResourceScope {
    match (snapshot, card.edit_id) {
        (Some(snapshot), _) => ResourceScope::Snapshot(snapshot),
        (None, Some(edit)) => ResourceScope::Edit(edit),
        (None, None) => ResourceScope::Snapshot(card.snapshot_id),
    }
}

/// 本文の一ページ。旧棚の読む画面の一ページ（`ReaderContentPage`）と同じ形。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentPageView {
    /// 0 から。
    pub page: usize,
    pub page_count: usize,
    /// 画像は `data-local-path` にファイルの識別子（添付は `識別子/印`）を持つ。
    pub html: String,
    /// 本文全体の平文の字数。
    pub total_plain_text_chars: usize,
    /// pixiv の原稿のページが何ページ目（0 から）から始まるか。
    pub source_page_starts: Vec<usize>,
}

/// 本文の一ページの HTML。`snapshot` を省くと今読む本文。
#[tauri::command]
pub async fn library_v2_content(
    state: State<'_, Arc<LibraryV2>>,
    work: LibraryId,
    snapshot: Option<LibraryId>,
    page: usize,
) -> Result<ContentPageView, String> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let library = state.library()?;
        let card = library.work_card(work).map_err(|e| e.to_string())?;
        let scope = scope_of(&card, snapshot);
        let document = state.document(&library, work, scope, &card.provider)?;
        let page_count = document.pages.len().max(1);
        let page = page.min(page_count - 1);
        Ok(ContentPageView {
            page,
            page_count,
            html: document.pages.get(page).cloned().unwrap_or_default(),
            total_plain_text_chars: document.text_chars,
            source_page_starts: document.source_page_starts.clone(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 本文全体の見出し（読む画面の目次）。`snapshot` を省くと今読む本文。
#[tauri::command]
pub async fn library_v2_outline(
    state: State<'_, Arc<LibraryV2>>,
    work: LibraryId,
    snapshot: Option<LibraryId>,
) -> Result<Vec<ReaderOutlineEntry>, String> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let library = state.library()?;
        let card = library.work_card(work).map_err(|e| e.to_string())?;
        let document = state.document(&library, work, scope_of(&card, snapshot), &card.provider)?;
        Ok(reader_outline(&document.pages))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 本文内検索（読む画面）。旧棚と同じ畳み方で、ページごとの一致を返す。
#[tauri::command]
pub async fn library_v2_search_content(
    state: State<'_, Arc<LibraryV2>>,
    work: LibraryId,
    snapshot: Option<LibraryId>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<ReaderSearchHit>, String> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let library = state.library()?;
        let card = library.work_card(work).map_err(|e| e.to_string())?;
        let document = state.document(&library, work, scope_of(&card, snapshot), &card.provider)?;
        Ok(search_reader_pages(
            &document.pages,
            &query,
            limit.unwrap_or(50),
        ))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 読む画面の前後の作品の、一つの並び。前後の作品は一覧の一件と同じ形（表紙の識別子つき）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NeighborsView {
    pub kind: NeighborKind,
    pub id: LibraryId,
    pub title: String,
    pub ordered: bool,
    pub position: usize,
    pub total: usize,
    pub missing: usize,
    pub previous: Option<CardView>,
    pub next: Option<CardView>,
}

/// 作品が入っているコレクション・シリーズ・作者ごとの、前後の作品。並びを丸ごと返さず、
/// 位置と両隣だけを返す（長いシリーズや作者でも一往復で済む）。
#[tauri::command]
pub async fn library_v2_neighbors(
    state: State<'_, Arc<LibraryV2>>,
    work: LibraryId,
) -> Result<Vec<NeighborsView>, String> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let library = state.library()?;
        let found = library.work_neighbors(work).map_err(|e| e.to_string())?;
        let view = |card: &Option<WorkCard>| {
            card.as_ref()
                .map(|card| state.card_view(&library, card))
                .transpose()
        };
        found
            .iter()
            .map(|row| {
                Ok(NeighborsView {
                    kind: row.kind,
                    id: row.id,
                    title: row.title.clone(),
                    ordered: row.ordered,
                    position: row.position,
                    total: row.total,
                    missing: row.missing,
                    previous: view(&row.previous)?,
                    next: view(&row.next)?,
                })
            })
            .collect()
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 版（省くと今読む本文）の画像・添付。
#[tauri::command]
pub async fn library_v2_media(
    state: State<'_, Arc<LibraryV2>>,
    work: LibraryId,
    snapshot: Option<LibraryId>,
) -> Result<Vec<MediaView>, String> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let library = state.library()?;
        let card = library.work_card(work).map_err(|e| e.to_string())?;
        library
            .work_media(work, scope_of(&card, snapshot))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 版の本文の平文。版どうしの比較に使う。
#[tauri::command]
pub async fn library_v2_version_text(
    state: State<'_, Arc<LibraryV2>>,
    work: LibraryId,
    snapshot: LibraryId,
) -> Result<String, String> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        state
            .library()?
            .work_text(work, ResourceScope::Snapshot(snapshot))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 版（省くと現在の版）を作った取得元の応答。なければ None。
#[tauri::command]
pub async fn library_v2_original(
    state: State<'_, Arc<LibraryV2>>,
    work: LibraryId,
    snapshot: Option<LibraryId>,
) -> Result<Option<String>, String> {
    let state = Arc::clone(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let library = state.library()?;
        let snapshot = match snapshot {
            Some(snapshot) => snapshot,
            None => {
                library
                    .work_card(work)
                    .map_err(|e| e.to_string())?
                    .snapshot_id
            }
        };
        library
            .work_original(work, snapshot)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
#[path = "library_v2_tests.rs"]
mod tests;
