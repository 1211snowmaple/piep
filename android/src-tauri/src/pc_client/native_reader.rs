//! 棚の作品を携帯で読む。
//!
//! サーバーから読書用の一式（本文・画像の対応・資産の一覧）を受け取り、デスクトップと同じ
//! 組み立て（`piep-reader-render`）でページにする。ページの切れ目が同じなので、「何ページ目の
//! どこ」で憶えた読書位置はデスクトップと携帯で同じ場所を指す。画像は一式に載っている
//! ファイルだけを部分実体で受け取り、中身のハッシュを確かめてから出す。
use super::native_library::{self, NativeLibraryScope};
use super::{ClientError, PcClientState};
use piep_library_protocol::{BundleFile, Id, ReaderBundle, ResourceReference, ResourceScope};
use piep_mobile_api::MobilePage;
use piep_reader_render::document::ReaderView;
use piep_reader_render::mobile::{MobileAsset, mobile_html};
use piep_reader_render::pages::{ReaderOutlineEntry, reader_outline};
use piep_storage::{ContentFile, Fingerprint};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tauri::State;

/// 開いておく文書の数。行き来する数冊ぶん。
const KEEP_DOCUMENTS: usize = 4;

/// 組み上げた一冊。画像の番号は `images` の位置。
pub(crate) struct NativeDocument {
    key: String,
    scope: NativeLibraryScope,
    entity: Id,
    view: ResourceScope,
    pages: Vec<String>,
    source_page_starts: Vec<usize>,
    text_chars: usize,
    images: Vec<BundleFile>,
}

impl NativeDocument {
    pub(crate) fn pages(&self) -> &[String] {
        &self.pages
    }

    pub(crate) fn source_page_starts(&self) -> &[usize] {
        &self.source_page_starts
    }

    pub(crate) fn text_chars(&self) -> usize {
        self.text_chars
    }

    pub(crate) fn image_count(&self) -> usize {
        self.images.len()
    }

    /// 本文の `asset` 番目の画像を取りに行く先。
    pub(crate) fn image(&self, asset: usize) -> Option<ImageTarget> {
        let file = self.images.get(asset)?;
        Some(ImageTarget {
            scope: self.scope,
            entity: self.entity,
            view: file.scope.clone(),
            resource: file.resource.clone(),
            file: file.file.to_native(),
        })
    }
}

#[derive(Default)]
pub struct NativeReaderState {
    documents: Mutex<VecDeque<Arc<NativeDocument>>>,
}

impl NativeReaderState {
    fn find(&self, key: &str) -> Option<Arc<NativeDocument>> {
        let documents = self.documents.lock().unwrap_or_else(|e| e.into_inner());
        documents.iter().find(|d| d.key == key).cloned()
    }

    fn keep(&self, document: Arc<NativeDocument>) {
        let mut documents = self.documents.lock().unwrap_or_else(|e| e.into_inner());
        documents.retain(|d| d.key != document.key);
        documents.push_front(document);
        documents.truncate(KEEP_DOCUMENTS);
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeOpened {
    /// ページと画像を引くときの鍵。
    key: String,
    entity_id: Id,
    view: ResourceScope,
    /// 利用者の編集を読んでいるか。
    edited: bool,
    page_count: usize,
    source_page_starts: Vec<usize>,
    total_plain_text_chars: usize,
    outline: Vec<ReaderOutlineEntry>,
}

fn view_key(entity: Id, view: &ResourceScope) -> String {
    let (kind, id) = match view {
        ResourceScope::Snapshot(id) => ("snapshot", id),
        ResourceScope::Edit(id) => ("edit", id),
    };
    format!("{}/{kind}/{}", entity.get(), id.get())
}

/// 一式を組む。本文の画像の置き場（ファイルの識別子）を、一式に載った資産の番号に置き換える。
/// 一式に無い置き場を指す本文は組まない（知らないファイルを取りに行かない）。
pub(crate) fn build(
    scope: NativeLibraryScope,
    bundle: ReaderBundle,
) -> Result<NativeDocument, ClientError> {
    let document = ReaderView::from_value(bundle.document).map_err(|_| ClientError::protocol())?;
    let reading = document.reading.clone().ok_or_else(ClientError::protocol)?;
    let (pages, source_page_starts) =
        piep_reader_render::document::render_pages(&document, &reading.provider);
    let mut by_file: HashMap<String, usize> = HashMap::new();
    let mut images: Vec<BundleFile> = Vec::new();
    let available: HashMap<String, &BundleFile> = bundle
        .files
        .iter()
        .filter(|file| {
            matches!(
                file.resource,
                ResourceReference::Media { .. } | ResourceReference::File { .. }
            )
        })
        .map(|file| (file.file.id.get().to_string(), file))
        .collect();
    let pages = pages
        .iter()
        .map(|html| {
            mobile_html(html, |path| {
                // 添付は `識別子/印` の形。画面は最初の `/` の前をファイルとして読む。
                let file = path.split('/').next().unwrap_or_default();
                let index = match by_file.get(file) {
                    Some(index) => *index,
                    None => {
                        let entry = available.get(file)?;
                        images.push((*entry).clone());
                        by_file.insert(file.to_string(), images.len() - 1);
                        images.len() - 1
                    }
                };
                Some(MobileAsset {
                    id: index as i64,
                    dimensions: None,
                })
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ClientError::protocol())?;
    Ok(NativeDocument {
        key: view_key(bundle.entity_id, &bundle.scope),
        scope,
        entity: bundle.entity_id,
        view: bundle.scope,
        pages,
        source_page_starts,
        text_chars: reading.text_chars,
        images,
    })
}

/// 取得元と作品の鍵から、この利用者が今読む版を開く。
#[tauri::command]
pub async fn pc_native_open(
    state: State<'_, PcClientState>,
    reader: State<'_, NativeReaderState>,
    scope: NativeLibraryScope,
    provider: String,
    key: String,
) -> Result<NativeOpened, ClientError> {
    let found = native_library::lookup(&state, scope, &provider, &key).await?;
    let view = match found.edit_id {
        Some(edit) => ResourceScope::Edit(edit),
        None => ResourceScope::Snapshot(found.snapshot_id),
    };
    open_view(&state, &reader, scope, found.entity_id, view).await
}

/// 作品の保存した版の一つ（版ごとの写し）を開く。作品の画面の版の一覧の `snapshotId`。
#[tauri::command]
pub async fn pc_native_open_snapshot(
    state: State<'_, PcClientState>,
    reader: State<'_, NativeReaderState>,
    scope: NativeLibraryScope,
    work: Id,
    snapshot: Id,
) -> Result<NativeOpened, ClientError> {
    open_view(
        &state,
        &reader,
        scope,
        work,
        ResourceScope::Snapshot(snapshot),
    )
    .await
}

async fn open_view(
    state: &PcClientState,
    reader: &NativeReaderState,
    scope: NativeLibraryScope,
    entity: Id,
    view: ResourceScope,
) -> Result<NativeOpened, ClientError> {
    let document_key = view_key(entity, &view);
    let document = match reader.find(&document_key) {
        Some(document) if document.scope.same_as(&scope) => document,
        _ => {
            let bundle = native_library::bundle(state, scope, entity, view.clone()).await?;
            let document = Arc::new(build(scope, bundle)?);
            reader.keep(Arc::clone(&document));
            document
        }
    };
    Ok(NativeOpened {
        key: document.key.clone(),
        entity_id: document.entity,
        view: document.view.clone(),
        edited: matches!(document.view, ResourceScope::Edit(_)),
        page_count: document.pages.len(),
        source_page_starts: document.source_page_starts.clone(),
        total_plain_text_chars: document.text_chars,
        outline: reader_outline(&document.pages),
    })
}

/// 開いた文書のページ（0 から）。
#[tauri::command]
pub fn pc_native_page(
    reader: State<'_, NativeReaderState>,
    key: String,
    page: usize,
) -> Result<MobilePage, ClientError> {
    let document = reader.find(&key).ok_or_else(ClientError::remote)?;
    let html = document
        .pages
        .get(page)
        .cloned()
        .ok_or_else(ClientError::remote)?;
    Ok(MobilePage {
        page,
        page_count: document.pages.len(),
        html,
        total_plain_text_chars: document.text_chars,
        source_page_starts: document.source_page_starts.clone(),
        revision: None,
    })
}

/// 画像を取りに行く先。開いた文書の一式に載っているものだけ。
pub(crate) struct ImageTarget {
    pub scope: NativeLibraryScope,
    pub entity: Id,
    pub view: ResourceScope,
    pub resource: ResourceReference,
    pub file: ContentFile,
}

impl ImageTarget {
    pub fn id(&self) -> Fingerprint {
        self.file.id
    }
}

pub(crate) fn image_target(
    reader: &NativeReaderState,
    key: &str,
    asset: usize,
) -> Option<ImageTarget> {
    reader.find(key)?.image(asset)
}

/// 部分実体を順に受け取り、それぞれと全体のハッシュを確かめてつなぐ。
pub(crate) async fn fetch_image(
    state: &PcClientState,
    target: &ImageTarget,
) -> Result<Vec<u8>, ClientError> {
    let mut bytes = Vec::with_capacity(target.file.length as usize);
    for (ordinal, chunk) in target.file.chunks.iter().enumerate() {
        let encoded = native_library::chunk(
            state,
            target.scope,
            target.entity,
            &target.view,
            &target.resource,
            target.file.id,
            ordinal,
        )
        .await?;
        let part =
            piep_storage::decode_chunk(&encoded, chunk).map_err(|_| ClientError::protocol())?;
        bytes.extend_from_slice(&part);
    }
    if bytes.len() as u64 != target.file.length || Fingerprint::of(&bytes) != target.file.id {
        return Err(ClientError::protocol());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use piep_library_protocol::File;
    use serde_json::json;

    fn id(n: u128) -> Id {
        Id::new(uuid::Uuid::from_u128(n)).unwrap()
    }

    fn scope() -> NativeLibraryScope {
        NativeLibraryScope::for_tests(id(1), id(2), id(3), 7)
    }

    fn file(bytes: &[u8]) -> ContentFile {
        let fingerprint = Fingerprint::of(bytes);
        ContentFile {
            id: fingerprint,
            length: bytes.len() as u64,
            chunks: vec![piep_storage::FileChunk {
                id: fingerprint,
                length: bytes.len() as u64,
            }],
        }
    }

    #[test]
    fn pages_point_only_at_files_in_the_bundle() {
        let picture = file(b"picture");
        let snapshot = id(5);
        let bundle = |files: Vec<BundleFile>| ReaderBundle {
            format: piep_library_protocol::BUNDLE_FORMAT.into(),
            library_id: id(1),
            entity_id: id(4),
            scope: ResourceScope::Snapshot(snapshot),
            document: json!({
                "format": "piep-reader-v1",
                "body": { "kind": "pixiv_text", "value": "一行目\n[uploadedimage:9]\n[newpage]\n二ページ目" },
                "bindings": [{
                    "reference": { "kind": "pixiv_uploaded", "image_id": "9" },
                    "resource": {
                        "reference": { "kind": "media", "slot": "inline" },
                        "file": { "id": picture.id, "length": picture.length }
                    }
                }],
                "reading": { "provider": "pixiv", "text_chars": 9 }
            }),
            files,
        };
        let listed = BundleFile {
            scope: ResourceScope::Snapshot(snapshot),
            resource: ResourceReference::Media {
                slot: "inline".into(),
            },
            file: File::from_native(&picture),
        };
        let document = build(scope(), bundle(vec![listed])).unwrap();
        assert_eq!(document.pages.len(), 2);
        assert_eq!(document.source_page_starts, vec![0, 1]);
        assert_eq!(document.text_chars, 9);
        assert!(document.pages[0].contains(r#"data-asset-id="0""#));
        assert!(!document.pages[0].contains("data-local-path"));
        assert_eq!(document.images.len(), 1);
        // 一式に載っていない画像を指す本文は組まない。
        assert!(build(scope(), bundle(Vec::new())).is_err());
    }
}
