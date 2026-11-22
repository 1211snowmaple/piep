//! 棚の読書用の文書（`piep-reader-v1`）から、読む画面の HTML とページを組む。
//!
//! 文書はデスクトップでは棚から作り、携帯ではサーバーから JSON で受け取る。どちらもこの形に
//! 読み込んでから組むので、同じ版はどの端末でも同じページになる。
//!
//! 本文の HTML は取得元の応答の変換（[`crate::parser`]）で組む。変換は本文の中の画像の指定を
//! 保存名で探すので、文書の「指定 → 画像」の対応から、保存名の形をした仮の添付の一覧を作って
//! 渡す。置き場には保存名ではなくファイルの識別子（中身の SHA-256）を入れ、画面がそれを画像の
//! 在りかにする。指定と画像の結び付けは文書の対応が決め、変換の名前の推測には頼らない
//! （仮の名前は、変換がその指定に対して探す名前そのもの）。

use crate::parser::{escape_html, parse_fanbox_value_to_html, parse_pixiv_value_to_html};
use crate::{AssetEntry, WorkBlock};
use piep_storage::Fingerprint;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// 文書の形の名前。
pub const READER_DOCUMENT_FORMAT: &str = "piep-reader-v1";

/// 本文の中の画像・添付の指定。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BodyAssetReference {
    PixivUploaded { image_id: String },
    PixivIllustration { illustration_id: String, page: u32 },
    ArticleImage { key: String },
    ArticleFile { key: String },
    ArticleImageAt { index: u32 },
    ArticleFileAt { index: u32 },
}

impl BodyAssetReference {
    pub fn is_image(&self) -> bool {
        !matches!(self, Self::ArticleFile { .. } | Self::ArticleFileAt { .. })
    }
}

/// 組むのに要る部分だけを読む、読書用の文書。知らない項目は読み飛ばす。
#[derive(Clone, Debug, Deserialize)]
pub struct ReaderView {
    pub format: String,
    pub body: ViewBody,
    #[serde(default)]
    pub base_resources: Option<ViewManifest>,
    #[serde(default)]
    pub bindings: Vec<ViewBinding>,
    /// 携帯へ渡す文書だけが持つ、読み方の情報。
    #[serde(default)]
    pub reading: Option<ReadingInfo>,
}

/// 携帯へ渡す文書に添える読み方の情報。添付の本文は差し込み済み。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadingInfo {
    /// 取得元（`pixiv` は原稿のページで割る）。
    pub provider: String,
    /// 本文の平文の字数。読む画面の残り時間の目安。
    pub text_chars: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ViewBody {
    PlainText(String),
    PixivText(String),
    Article(Value),
    Edit(ViewEdit),
}

#[derive(Clone, Debug, Deserialize)]
pub struct ViewEdit {
    pub blocks: Vec<ViewEditBlock>,
    #[serde(default)]
    pub media: Vec<ViewEditMedia>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ViewEditBlock {
    pub ordinal: i64,
    pub kind: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub attrs: Value,
    #[serde(default)]
    pub media_slot: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ViewEditMedia {
    pub slot: String,
    pub file: Fingerprint,
    pub original_name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ViewManifest {
    pub resources: Vec<ViewResource>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ViewResource {
    pub reference: ViewReference,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub file: Option<ViewFile>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ViewReference {
    Media {
        slot: String,
    },
    File {
        slot: String,
    },
    /// 本文・原応答。組むのには使わない。
    #[serde(other)]
    Other,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ViewFile {
    pub id: Fingerprint,
    pub length: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ViewBinding {
    pub reference: BodyAssetReference,
    /// None は未取得。
    #[serde(default)]
    pub resource: Option<ViewResource>,
}

impl ReaderView {
    /// JSON の文書を読む。形の名前が違うものは受け取らない。
    pub fn from_value(document: Value) -> Result<Self, String> {
        let view: Self = serde_json::from_value(document).map_err(|e| e.to_string())?;
        if view.format != READER_DOCUMENT_FORMAT {
            return Err(format!("読書用の文書の形が違います: {}", view.format));
        }
        Ok(view)
    }
}

/// 本文を HTML にする。画像は `data-local-path` にファイルの識別子を入れる。
pub fn render_html(document: &ReaderView) -> String {
    match &document.body {
        ViewBody::PixivText(text) => {
            parse_pixiv_value_to_html(&json!({ "text": text }), &bound_assets(document).0)
        }
        ViewBody::Article(body) => {
            let (assets, marks) = bound_assets(document);
            let mut body = body.clone();
            let restore = marks.apply(&mut body);
            let mut html = parse_fanbox_value_to_html(&json!({ "body": body }), &assets);
            // 照合のために置いた画像の印は、画像の説明（alt）と参照にそのまま出る。
            // 取得元の画像の ID に戻す（ID の無い項目は空）。
            for (token, original) in restore {
                html = html.replace(&token, &escape_html(&original));
            }
            html
        }
        ViewBody::PlainText(text) => plain_html(text),
        ViewBody::Edit(content) => edit_html(content, document),
    }
}

/// 本文を転送用のページに割る。`provider` は取得元（`pixiv` は原稿のページで割る）。
pub fn render_pages(document: &ReaderView, provider: &str) -> (Vec<String>, Vec<usize>) {
    crate::pages::paginate_reader_html(&render_html(document), provider)
}

/// 編集の平文。`blocks_to_plain_text` と同じ数え方。
pub fn edit_text(content: &ViewEdit) -> String {
    content
        .blocks
        .iter()
        .filter(|block| !matches!(block.kind.as_str(), "image" | "separator" | "page_break"))
        .filter_map(|block| block.text.as_deref())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn plain_html(text: &str) -> String {
    escape_html(text).replace('\n', "<br />\n")
}

/// 置き場（`local_path`）の形。画面は最初の `/` の前をファイルの識別子として読む。
/// 添付は印（`file_token`）を後ろに付ける。
fn local_path(info: &ViewResource, key: Option<&str>) -> Option<String> {
    let file = info.file.as_ref()?;
    Some(match key {
        Some(key) => format!("{}/{key}", file.id),
        None => file.id.to_string(),
    })
}

/// 本文に付ける印。変換は添付を「名前が一致する」か「置き場に取得元の ID を含む」で
/// 探す。置き場の 16 進は短い ID（`f1` など）をたまたま含むので、別の画像や添付に当たる。
/// 画像だけ・添付だけの投稿の配列（`images`・`files`）は項目に ID が無いこともある。
/// そこで文書の対応が結んだ項目の ID を、16 進に現れない文字（`z`）で囲んだ印に
/// 置き換え、その印で照合させる。
#[derive(Default)]
pub struct Marks {
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
    pub fn apply(&self, body: &mut Value) -> Vec<(String, String)> {
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

/// 文書の対応から、変換が探す名前の仮の添付を作る。本文に付ける印も返す。
pub fn bound_assets(document: &ReaderView) -> (Vec<AssetEntry>, Marks) {
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

/// 編集の本文。編集画面と同じ関数（`blocks_to_html`）で組む。
///
/// 段の間の空行の数（`gap`）、見出し・引用・リンク・画像の説明の組み方、段の切れ目の印が編集画面と
/// 同じでなければ、「何ページ目の何行目」で憶えた読書位置としおりが別の行を指す。
/// 移した編集は段の種類・属性・画像の枠（`legacy-asset:旧い番号`）を移す前のまま持っている。
fn edit_html(content: &ViewEdit, document: &ReaderView) -> String {
    // 枠 → (ファイルの識別子, 名前)。取り込んだ画像と、基準の版の画像。
    let mut files: BTreeMap<&str, (String, String)> = BTreeMap::new();
    for info in document
        .base_resources
        .iter()
        .flat_map(|manifest| manifest.resources.iter())
    {
        if let (ViewReference::Media { slot }, Some(file)) = (&info.reference, &info.file) {
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
    crate::blocks::blocks_to_html(&blocks, &assets)
}
