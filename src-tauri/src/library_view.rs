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
    BodyAssetReference, EditContent, Library, LibraryId, ReaderBody, ReaderDocument,
    ResourceInfo, ResourceReference, ResourceScope, Result,
};
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;

/// 一冊を画面のページに分けたもの。
#[derive(Clone, Debug)]
pub struct RenderedDocument {
    pub pages: Vec<String>,
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
            Err(crate::library::Error::ResourceUnavailable)
                if matches!(scope, ResourceScope::Snapshot(snapshot) if self.source_body(work, snapshot)?.is_none()) =>
            {
                return Ok(RenderedDocument { pages: Vec::new() });
            }
            Err(error) => return Err(error),
        };
        let html = render_html(&document, provider);
        let (pages, _) = crate::database::queries::paginate_reader_html(&html, provider);
        Ok(RenderedDocument { pages })
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
pub fn render_html(document: &ReaderDocument, provider: &str) -> String {
    match &document.body {
        ReaderBody::PixivText(text) => {
            parse_pixiv_value_to_html(&json!({ "text": text }), &bound_assets(document).0)
        }
        ReaderBody::Article(body) => {
            let (assets, marks) = bound_assets(document);
            let mut body = body.clone();
            marks.apply(&mut body);
            parse_fanbox_value_to_html(&json!({ "body": body }), &assets)
        }
        ReaderBody::PlainText(text) => plain_html(text),
        ReaderBody::Edit(content) => edit_html(content, document, provider),
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

    fn apply(&self, body: &mut serde_json::Value) {
        use serde_json::Value;
        if let Some(blocks) = body.get_mut("blocks").and_then(Value::as_array_mut) {
            for block in blocks {
                let token = block
                    .get("fileId")
                    .and_then(Value::as_str)
                    .and_then(|id| self.file_keys.get(id));
                if let Some(token) = token {
                    block["fileId"] = json!(token);
                }
            }
        }
        if let Some(map) = body.get_mut("fileMap").and_then(Value::as_object_mut) {
            let renamed: serde_json::Map<String, Value> = std::mem::take(map)
                .into_iter()
                .map(|(key, value)| (self.file_keys.get(&key).cloned().unwrap_or(key), value))
                .collect();
            *map = renamed;
        }
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
                    object.insert("id".into(), json!(token));
                }
            }
        }
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
            BodyAssetReference::ArticleImage { key } => (format!("{key}.img"), None, "illustration"),
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

/// 編集の本文。旧棚の編集の HTML と同じ段の組み（見出し・段落・画像・区切り・改ページ）。
fn edit_html(content: &EditContent, document: &ReaderDocument, provider: &str) -> String {
    let media: BTreeMap<&str, String> = content
        .media
        .iter()
        .map(|media| (media.slot.as_str(), media.file.to_string()))
        .collect();
    // 取り込んだ画像を指す段は、基準の版の画像の枠（slot）を指す。
    let base: BTreeMap<String, String> = document
        .base_resources
        .iter()
        .flat_map(|manifest| manifest.resources.iter())
        .filter_map(|info| match (&info.reference, &info.file) {
            (ResourceReference::Media { slot }, Some(file)) => Some((slot.clone(), file.id.to_string())),
            _ => None,
        })
        .collect();
    let separator = if provider.eq_ignore_ascii_case("pixiv") {
        "<!-- newpage -->"
    } else {
        "<!-- content-block -->"
    };
    let mut parts = Vec::with_capacity(content.blocks.len());
    for block in &content.blocks {
        let text = block.text.as_deref().unwrap_or("");
        parts.push(match block.kind.as_str() {
            "heading" | "header" | "h" => format!("<h2>{}</h2>", escape_html(text)),
            "separator" => "<hr />".to_string(),
            "page_break" => separator.to_string(),
            "image" => {
                let file = block
                    .media_slot
                    .as_deref()
                    .and_then(|slot| media.get(slot).or_else(|| base.get(slot)));
                match file {
                    Some(file) => format!(
                        r#"<img class="novel-image" data-local-path="{}" alt="{}" />"#,
                        escape_html(file),
                        escape_html(text)
                    ),
                    None => r#"<div class="missing-image-placeholder" role="note" data-image-missing="true">取得できなかった画像</div>"#.to_string(),
                }
            }
            _ => plain_html(text),
        });
    }
    parts.join("<br />\n")
}

#[cfg(test)]
#[path = "library_view_tests.rs"]
mod tests;
