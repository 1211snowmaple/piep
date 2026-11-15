//! 新しい棚の作品を、作品の画面と読む画面が出す形にする（本文の HTML・画像・版）。
//!
//! 本文の HTML は旧棚の変換（`database::parser`）をそのまま使う。旧棚の変換は本文の中の
//! 画像の指定を保存名で探すので、新しい棚の「指定 → 画像」の対応（reader bindings）から、
//! 旧棚の保存名の形をした仮の添付の一覧を作って渡す。置き場には保存名ではなく
//! ファイルの識別子（中身の SHA-256）を入れ、画面が `piep-library` の URI にする。
//! 指定と画像の結び付けは新しい棚の対応が決め、旧棚の名前の推測には頼らない
//! （仮の名前は、旧棚の変換がその指定に対して探す名前そのもの）。

use crate::database::models::AssetEntry;
use crate::database::parser::{escape_html, parse_fanbox_value_to_html, parse_pixiv_value_to_html};
use crate::library::{
    BodyAssetReference, ContentFile, EditContent, Fingerprint, Library, LibraryId, ReaderBody,
    ReaderDocument, ResourceInfo, ResourceReference, ResourceScope, Result,
};
use rusqlite::OptionalExtension;
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;

/// 一冊を画面のページに分けたもの。
#[derive(Clone, Debug, Default)]
pub struct RenderedDocument {
    pub pages: Vec<String>,
    /// pixiv の原稿のページ（`[newpage]`）が、何ページ目（0 から）から始まるか。
    /// 長い原稿のページは転送のために割るので、`[jump:n]` はこれで読み替える。
    pub source_page_starts: Vec<usize>,
    /// 本文全体の平文の字数。読む画面の残り時間の目安。
    pub text_chars: usize,
}

/// 保存した版の一つ。同じ改稿の補完は最後のものだけを数える（旧棚の版と同じ数え方）。
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct VersionView {
    pub snapshot: LibraryId,
    /// 版の番号（取得元の改稿の順、1 から）。
    pub version: i64,
    /// ミリ秒。
    pub saved_at: i64,
    pub text_length: Option<i64>,
}

/// 作品の画像・添付の一件。
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct MediaView {
    /// `media` か `file`。
    pub kind: &'static str,
    pub role: String,
    pub name: Option<String>,
    pub mime_type: Option<String>,
    pub length: u64,
    /// ファイルの識別子（16 進）。未保存なら None。
    pub file: Option<String>,
}

impl Library {
    /// 保存した版（新しい順ではなく古い順）。
    pub fn work_versions(&self, work: LibraryId) -> Result<Vec<VersionView>> {
        let mut versions: BTreeMap<i64, VersionView> = BTreeMap::new();
        for snapshot in self.snapshots(work)? {
            let text_length = snapshot
                .metadata
                .get("text_length")
                .and_then(serde_json::Value::as_i64);
            versions.insert(
                snapshot.revision,
                VersionView {
                    snapshot: snapshot.id,
                    version: snapshot.revision,
                    saved_at: snapshot.saved_at,
                    text_length,
                },
            );
        }
        Ok(versions.into_values().collect())
    }

    /// 版（または有効な編集）の画像・添付。本文と取得元の応答は含めない。
    pub fn work_media(&self, work: LibraryId, scope: ResourceScope) -> Result<Vec<MediaView>> {
        Ok(self
            .resource_manifest(work, scope)?
            .resources
            .into_iter()
            .filter_map(|info| {
                let kind = match info.reference {
                    ResourceReference::Media { .. } => "media",
                    ResourceReference::File { .. } => "file",
                    _ => return None,
                };
                Some(MediaView {
                    kind,
                    role: info.role,
                    name: info.original_name,
                    mime_type: info.mime_type,
                    length: info.file.as_ref().map_or(0, |file| file.length),
                    file: info.file.map(|file| file.id.to_string()),
                })
            })
            .collect())
    }

    /// 作品の添付の PDF（「原本を見る」）。作品の保存した版のどれかに PDF として載っている
    /// ファイルだけを返し、載っていなければ `None`。
    ///
    /// 画面から受け取った識別子をそのまま開くと、作品と関係のないファイルを読ませる口になる
    /// （旧棚の `render_pdf_attachment_page` が作品のアセットに限るのと同じ）。
    pub fn work_pdf_attachment(
        &self,
        work: LibraryId,
        file: Fingerprint,
    ) -> Result<Option<ContentFile>> {
        let listed = self
            .reader()?
            .query_row(
                "SELECT 1 FROM snapshot_files f JOIN revision_snapshots s ON s.id=f.snapshot_id
                 WHERE s.entity_id=?1 AND s.published=1 AND f.file_id=?2
                   AND lower(f.original_name) LIKE '%.pdf'
                 LIMIT 1",
                rusqlite::params![work.as_bytes().as_slice(), file.0.as_slice()],
                |_| Ok(()),
            )
            .optional()?;
        match listed {
            Some(()) => Ok(Some(self.file(file)?)),
            None => Ok(None),
        }
    }

    /// 取得元の応答（保存データ）。版に結び付いた最初のもの。
    pub fn work_original(&self, work: LibraryId, snapshot: LibraryId) -> Result<Option<String>> {
        let scope = ResourceScope::Snapshot(snapshot);
        let original = self
            .resource_manifest(work, scope)?
            .resources
            .into_iter()
            .filter(|info| matches!(info.reference, ResourceReference::Original { .. }))
            .filter(|info| info.file.is_some())
            // この版を作った応答を先に。
            .min_by_key(|info| info.role != "source_response");
        let Some(original) = original else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        self.read_resource(work, scope, &original.reference, None, &mut bytes)?;
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
    }

    /// 版の本文の平文（版どうしの比較に使う）。編集は段落の文を空行でつなぐ。
    pub fn work_text(&self, work: LibraryId, scope: ResourceScope) -> Result<String> {
        match scope {
            ResourceScope::Snapshot(snapshot) => Ok(self
                .source_body(work, snapshot)?
                .map(|body| body.text_content())
                .unwrap_or_default()),
            ResourceScope::Edit(_) => match self.reader_document(work, scope)?.body {
                ReaderBody::Edit(content) => Ok(edit_text(&content)),
                _ => Ok(String::new()),
            },
        }
    }

    /// 版（または有効な編集）の本文を、画面のページに分けた HTML にする。
    pub fn render_document(
        &self,
        work: LibraryId,
        scope: ResourceScope,
        provider: &str,
    ) -> Result<RenderedDocument> {
        let document = match self.reader_document(work, scope) {
            Ok(document) => document,
            // 取得元の応答に本文が無い作品（公開範囲の外の投稿など）。旧棚と同じく空の本文にする。
            Err(crate::library::Error::ResourceUnavailable) if matches!(scope, ResourceScope::Snapshot(snapshot) if self.source_body(work, snapshot)?.is_none()) =>
            {
                return Ok(RenderedDocument::default());
            }
            Err(error) => return Err(error),
        };
        let mut document = document;
        // FANBOX の保存版は、旧棚と同じく添付の PDF・テキストの本文を添付の位置へ差し込む。
        // 検索の文書と同じ関数を通すので、読める本文と検索に入る本文が食い違わない。
        let mut inserted_text = None;
        if let (ReaderBody::Article(body), ResourceScope::Snapshot(snapshot)) =
            (&mut document.body, scope)
        {
            let mut post = json!({ "body": std::mem::take(body) });
            let conn = self.reader()?;
            self.insert_attachment_bodies(&conn, snapshot, provider, &mut post)?;
            inserted_text = Some(crate::database::search::extract_search_body(
                &post, "fanbox",
            ));
            *body = post["body"].take();
        }
        let html = render_html(&document);
        let (pages, source_page_starts) =
            crate::database::queries::paginate_reader_html(&html, provider);
        // 旧棚の読む画面と同じく、HTML ではなく本文の平文（検索に使う本文、添付から取り込んだ
        // 本文を含む）の字数。
        let text_chars = match inserted_text {
            Some(text) => text.chars().count(),
            None => self.work_text(work, scope)?.chars().count(),
        };
        Ok(RenderedDocument {
            pages,
            source_page_starts,
            text_chars,
        })
    }
}

fn edit_text(content: &EditContent) -> String {
    content
        .blocks
        .iter()
        .filter(|block| !matches!(block.kind.as_str(), "image" | "separator" | "page_break"))
        .filter_map(|block| block.text.as_deref())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 本文を HTML にする。画像は `data-local-path` にファイルの識別子を入れる。
pub fn render_html(document: &ReaderDocument) -> String {
    match &document.body {
        ReaderBody::PixivText(text) => {
            parse_pixiv_value_to_html(&json!({ "text": text }), &bound_assets(document).0)
        }
        ReaderBody::Article(body) => {
            let (assets, marks) = bound_assets(document);
            let mut body = body.clone();
            let restore = marks.apply(&mut body);
            let mut html = parse_fanbox_value_to_html(&json!({ "body": body }), &assets);
            // 照合のために置いた画像の印は、画像の説明（alt）と参照にそのまま出る。旧棚と同じく
            // 取得元の画像の ID に戻す（ID の無い項目は空）。
            for (token, original) in restore {
                html = html.replace(&token, &escape_html(&original));
            }
            html
        }
        ReaderBody::PlainText(text) => plain_html(text),
        ReaderBody::Edit(content) => edit_html(content, document),
    }
}

fn plain_html(text: &str) -> String {
    escape_html(text).replace('\n', "<br />\n")
}

/// 置き場（`local_path`）の形。画面は最初の `/` の前をファイルの識別子として読む。
/// 添付は印（`file_token`）を後ろに付ける。
fn local_path(info: &ResourceInfo, key: Option<&str>) -> Option<String> {
    let file = info.file.as_ref()?;
    Some(match key {
        Some(key) => format!("{}/{key}", file.id),
        None => file.id.to_string(),
    })
}

/// 本文に付ける印。旧棚の変換は添付を「名前が一致する」か「置き場に取得元の ID を含む」で
/// 探す。置き場の 16 進は短い ID（`f1` など）をたまたま含むので、別の画像や添付に当たる。
/// 画像だけ・添付だけの投稿の配列（`images`・`files`）は項目に ID が無いこともある。
/// そこで新しい棚の対応が結んだ項目の ID を、16 進に現れない文字（`z`）で囲んだ印に
/// 置き換え、その印で照合させる。
#[derive(Default)]
struct Marks {
    /// `blocks` の `fileId` と `fileMap` の鍵 → 印。
    file_keys: BTreeMap<String, String>,
    /// `images` の何番目 → 印。
    images_at: BTreeMap<usize, String>,
    /// `files` の何番目 → 印。
    files_at: BTreeMap<usize, String>,
}

impl Marks {
    fn next(&self, kind: &str) -> String {
        let count = self.file_keys.len() + self.images_at.len() + self.files_at.len();
        format!("piep-{kind}-z{count}z")
    }

    /// 印を置く。画像の印と元の ID の組を返す（組んだ HTML の中で元に戻すため）。
    fn apply(&self, body: &mut serde_json::Value) -> Vec<(String, String)> {
        use serde_json::Value;
        // 印へ置き換える ID。`blocks` の投稿は `fileMap` の鍵、`files` の投稿は項目の ID。
        // 添付の本文を差し込んだ `files` の投稿は、`files` から組んだ `blocks` と `fileMap` が
        // 項目の元の ID で添付を指すので、その ID も印にする。
        let mut keys = self.file_keys.clone();
        let mut restore = Vec::new();
        for (name, marks) in [("images", &self.images_at), ("files", &self.files_at)] {
            let Some(items) = body.get_mut(name).and_then(Value::as_array_mut) else {
                continue;
            };
            for (index, item) in items.iter_mut().enumerate() {
                let Some(object) = item.as_object_mut() else {
                    continue;
                };
                // 結ばれていない項目は、保存された別の画像に当たらないよう置き場の手がかりを外す。
                object.remove("localPath");
                if let Some(token) = marks.get(&index) {
                    let original = object.get("id").and_then(Value::as_str).unwrap_or("");
                    if name == "files" && !original.is_empty() {
                        keys.insert(original.to_string(), token.clone());
                    } else if name == "images" {
                        restore.push((token.clone(), original.to_string()));
                    }
                    object.insert("id".into(), json!(token));
                }
            }
        }
        if let Some(blocks) = body.get_mut("blocks").and_then(Value::as_array_mut) {
            for block in blocks {
                let token = block
                    .get("fileId")
                    .and_then(Value::as_str)
                    .and_then(|id| keys.get(id));
                if let Some(token) = token {
                    block["fileId"] = json!(token);
                }
            }
        }
        if let Some(map) = body.get_mut("fileMap").and_then(Value::as_object_mut) {
            let renamed: serde_json::Map<String, Value> = std::mem::take(map)
                .into_iter()
                .map(|(key, value)| (keys.get(&key).cloned().unwrap_or(key), value))
                .collect();
            *map = renamed;
        }
        restore
    }
}

/// 新しい棚の対応から、旧棚の変換が探す名前の仮の添付を作る。本文に付ける印も返す。
fn bound_assets(document: &ReaderDocument) -> (Vec<AssetEntry>, Marks) {
    let mut assets = Vec::new();
    let mut marks = Marks::default();
    for binding in &document.bindings {
        let Some(resource) = &binding.resource else {
            continue;
        };
        let (filename, key, asset_type) = match &binding.reference {
            BodyAssetReference::PixivUploaded { image_id } => {
                (format!("inline_{image_id}.img"), None, "illustration")
            }
            BodyAssetReference::PixivIllustration {
                illustration_id,
                page,
            } => (
                format!("{illustration_id}_p{}.img", page.saturating_sub(1)),
                None,
                "illustration",
            ),
            BodyAssetReference::ArticleImage { key } => {
                (format!("{key}.img"), None, "illustration")
            }
            BodyAssetReference::ArticleFile { key } => {
                if resource.file.is_none() {
                    continue;
                }
                let token = marks.next("file");
                marks.file_keys.insert(key.clone(), token.clone());
                (token.clone(), Some(token), "file")
            }
            BodyAssetReference::ArticleImageAt { index } => {
                if resource.file.is_none() {
                    continue;
                }
                let token = marks.next("image");
                marks.images_at.insert(*index as usize, token.clone());
                (format!("{token}.img"), None, "illustration")
            }
            BodyAssetReference::ArticleFileAt { index } => {
                if resource.file.is_none() {
                    continue;
                }
                let token = marks.next("file");
                marks.files_at.insert(*index as usize, token.clone());
                (token.clone(), Some(token), "file")
            }
        };
        let Some(path) = local_path(resource, key.as_deref()) else {
            continue;
        };
        assets.push(AssetEntry {
            id: assets.len() as i64,
            download_id: 0,
            asset_type: asset_type.into(),
            filename,
            local_path: path,
            original_url: None,
            mime_type: resource.mime_type.clone(),
            file_size_bytes: resource.file.as_ref().map_or(0, |file| file.length as i64),
        });
    }
    (assets, marks)
}

/// 編集の本文。旧棚の編集と同じ関数（`blocks_to_html`）で組む。
///
/// 段の間の空行の数（`gap`）、見出し・引用・リンク・画像の説明の組み方、段の切れ目の印が旧棚と
/// 同じでなければ、「何ページ目の何行目」で憶えた読書位置としおりが別の行を指す。前は新しい棚で
/// 組み直していて、段の間を一律に一行にしていたので、編集した作品だけ行の数が違っていた。
/// 移した編集は段の種類・属性・画像の枠（`legacy-asset:旧い番号`）を旧棚のまま持っている。
fn edit_html(content: &EditContent, document: &ReaderDocument) -> String {
    use crate::database::models::WorkBlock;
    // 枠 → (ファイルの識別子, 名前)。取り込んだ画像と、基準の版の画像。
    let mut files: BTreeMap<&str, (String, String)> = BTreeMap::new();
    for info in document
        .base_resources
        .iter()
        .flat_map(|manifest| manifest.resources.iter())
    {
        if let (ResourceReference::Media { slot }, Some(file)) = (&info.reference, &info.file) {
            files.insert(slot.as_str(), (file.id.to_string(), slot.clone()));
        }
    }
    for media in &content.media {
        files.insert(
            media.slot.as_str(),
            (media.file.to_string(), media.original_name.clone()),
        );
    }
    let mut assets: Vec<AssetEntry> = Vec::new();
    let mut asset_of: BTreeMap<&str, i64> = BTreeMap::new();
    let blocks: Vec<WorkBlock> = content
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            let asset_id = block.media_slot.as_deref().and_then(|slot| {
                let (file, name) = files.get(slot)?;
                Some(*asset_of.entry(slot).or_insert_with(|| {
                    let id = assets.len() as i64 + 1;
                    assets.push(AssetEntry {
                        id,
                        download_id: 0,
                        asset_type: "illustration".into(),
                        filename: name.clone(),
                        local_path: file.clone(),
                        original_url: None,
                        mime_type: None,
                        file_size_bytes: 0,
                    });
                    id
                }))
            });
            WorkBlock {
                id: index as i64,
                edit_revision_id: 0,
                order: block.ordinal,
                block_type: block.kind.clone(),
                text: block.text.clone(),
                asset_id,
                attrs_json: (!block.attrs.is_null()).then(|| block.attrs.to_string()),
            }
        })
        .collect();
    crate::database::queries::blocks_to_html(&blocks, &assets)
}

#[cfg(test)]
#[path = "library_view_tests.rs"]
mod tests;
