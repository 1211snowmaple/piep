//! 棚の作品を、作品の画面と読む画面が出す形にする（本文の HTML・画像・版）。
//!
//! 本文の HTML は取得元の応答の変換（`piep_reader_render::parser`）で組む。変換は本文の中の
//! 画像の指定を保存名で探すので、棚の「指定 → 画像」の対応（reader bindings）から、
//! 保存名の形をした仮の添付の一覧を作って渡す。置き場には保存名ではなく
//! ファイルの識別子（中身の SHA-256）を入れ、画面が `piep-library` の URI にする。
//! 指定と画像の結び付けは棚の対応が決め、変換の名前の推測には頼らない
//! （仮の名前は、変換がその指定に対して探す名前そのもの）。

use crate::library::{
    ContentFile, EditContent, Fingerprint, Library, LibraryId, ReaderBody, ReaderDocument,
    ResourceReference, ResourceScope, Result, WorkCard,
};
use crate::models::AssetEntry;
use piep_reader_render::document::ReaderView;
use rusqlite::OptionalExtension;
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;

#[path = "library_view_snapshot.rs"]
mod snapshot;
pub use snapshot::RevisionSnapshotView;

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

/// 保存した版の一つ。同じ改稿の補完は最後のものだけを数える。
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct VersionView {
    pub snapshot: LibraryId,
    /// 版の番号（取得元の改稿の順、1 から）。
    pub version: i64,
    /// ミリ秒。
    pub saved_at: i64,
    pub text_length: Option<i64>,
}

/// 作品の画面の頭。一覧の一件に、説明・作者とシリーズの取得元の ID・保存した版を足したもの。
/// デスクトップの作品の画面と、携帯へ返す作品の情報で同じものを使う。
#[derive(Clone, Debug, PartialEq)]
pub struct WorkHead {
    pub card: WorkCard,
    /// 取得元の説明（HTML のこともある）。なければ抜粋。
    pub caption: Option<String>,
    /// 取得元の作者の ID。作者の画面へのリンクに使う。
    pub author_key: Option<String>,
    /// 取得元のシリーズの ID。
    pub series_key: Option<String>,
    /// 保存した版（古い順）。
    pub versions: Vec<VersionView>,
    /// 有効な編集の基準の版の番号。編集が無ければ None。
    pub edit_base_version: Option<i64>,
}

/// 一覧の一件の作者・シリーズの、取得元での ID。カードから作者・シリーズの画面へ行くため。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CardKeys {
    pub author: Option<String>,
    pub series: Option<String>,
}

fn text_of(value: Option<&serde_json::Value>) -> Option<String> {
    match value? {
        serde_json::Value::String(text) if !text.trim().is_empty() => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
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
    /// 作品の画面の頭。説明・作者とシリーズの ID は今の版の取得元の情報から取る。
    pub fn work_head(&self, work: LibraryId) -> Result<WorkHead> {
        let card = self.work_card(work)?;
        let snapshots = self.snapshots(work)?;
        let metadata = snapshots
            .iter()
            .find(|snapshot| snapshot.id == card.snapshot_id)
            .or(snapshots.last())
            .map(|snapshot| snapshot.metadata.clone())
            .unwrap_or_default();
        let versions = self.work_versions(work)?;
        let edit_base_version = card
            .edit_id
            .map(|edit| self.edit_base_version(work, edit))
            .transpose()?;
        Ok(WorkHead {
            caption: text_of(metadata.get("caption")).or_else(|| card.excerpt.clone()),
            author_key: text_of(metadata.get("author_key")),
            series_key: text_of(metadata.get("series_id")),
            card,
            versions,
            edit_base_version,
        })
    }

    /// 一覧の各件の作者・シリーズの取得元の ID（`work_head` と同じ取り方、版の情報から）。
    pub fn card_keys(&self, cards: &[WorkCard]) -> Result<Vec<CardKeys>> {
        let conn = self.reader()?;
        let mut statement = conn.prepare_cached(
            "SELECT json_extract(metadata_json,'$.author_key'),json_extract(metadata_json,'$.series_id') FROM revision_snapshots WHERE id=?1",
        )?;
        let key = |value: rusqlite::types::Value| match value {
            rusqlite::types::Value::Text(text) if !text.trim().is_empty() => Some(text),
            rusqlite::types::Value::Integer(number) => Some(number.to_string()),
            _ => None,
        };
        cards
            .iter()
            .map(|card| {
                let found = statement
                    .query_row([card.snapshot_id.as_bytes().as_slice()], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .optional()?;
                Ok(match found {
                    Some((author, series)) => CardKeys {
                        author: key(author),
                        series: key(series),
                    },
                    None => CardKeys::default(),
                })
            })
            .collect()
    }

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

    /// 編集の基準の版の番号（取得元の改稿の順、`VersionView::version` と同じ数え方）。
    ///
    /// 読む画面は、これが今の版より古いと「編集版（v… 基準）」と出す。編集のあとに取り込んだ
    /// 改稿は、編集版を読んでいるあいだ見えないからである。
    pub fn edit_base_version(&self, work: LibraryId, edit: LibraryId) -> Result<i64> {
        let base: Vec<u8> = self.reader()?.query_row(
            "SELECT base_snapshot_id FROM edit_revisions WHERE id=?1 AND work_id=?2 AND sealed=1",
            rusqlite::params![edit.as_bytes().as_slice(), work.as_bytes().as_slice()],
            |r| r.get(0),
        )?;
        self.snapshots(work)?
            .into_iter()
            .find(|snapshot| snapshot.id.as_bytes().as_slice() == base.as_slice())
            .map(|snapshot| snapshot.revision)
            .ok_or(crate::library::Error::ResourceUnavailable)
    }

    /// 作品の添付の PDF（「原本を見る」）。作品の保存した版のどれかに PDF として載っている
    /// ファイルだけを返し、載っていなければ `None`。
    ///
    /// 画面から受け取った識別子をそのまま開くと、作品と関係のないファイルを読ませる口になる
    /// 。
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

    /// 読む画面へ渡す文書と、本文の平文の字数。デスクトップの読む画面と携帯へ渡す文書が通す。
    ///
    /// FANBOX の保存版は、添付の PDF・テキストの本文を添付の位置へ差し込む。
    /// 検索の文書と同じ関数を通すので、読める本文と検索に入る本文が食い違わない。携帯は添付を
    /// 読み解けないので、差し込んだ文書を渡す。字数は HTML ではなく
    /// 本文の平文（検索に使う本文、添付から取り込んだ本文を含む）で数える。
    ///
    /// 取得元の応答に本文が無い作品（公開範囲の外の投稿など）は、空の本文にする。
    pub fn reading_document(
        &self,
        work: LibraryId,
        scope: ResourceScope,
        provider: &str,
    ) -> Result<(ReaderDocument, usize)> {
        let mut document = match self.reader_document(work, scope) {
            Ok(document) => document,
            Err(crate::library::Error::ResourceUnavailable) if matches!(scope, ResourceScope::Snapshot(snapshot) if matches!(self.source_body(work, snapshot), Ok(None))) =>
            {
                let document = ReaderDocument {
                    format: piep_reader_render::document::READER_DOCUMENT_FORMAT,
                    entity: work,
                    scope,
                    metadata: BTreeMap::new(),
                    body: ReaderBody::PlainText(String::new()),
                    resources: self.resource_manifest(work, scope)?,
                    base_resources: None,
                    bindings: Vec::new(),
                    bindings_hash: None,
                };
                return Ok((document, 0));
            }
            Err(error) => return Err(error),
        };
        let mut inserted_text = None;
        if let (ReaderBody::Article(body), ResourceScope::Snapshot(snapshot)) =
            (&mut document.body, scope)
        {
            let mut post = json!({ "body": std::mem::take(body) });
            let conn = self.reader()?;
            self.insert_attachment_bodies(&conn, snapshot, provider, &mut post)?;
            inserted_text = Some(crate::search_engine::text::extract_search_body(
                &post, "fanbox",
            ));
            *body = post["body"].take();
        }
        let text_chars = match inserted_text {
            Some(text) => text.chars().count(),
            None => self.work_text(work, scope)?.chars().count(),
        };
        Ok((document, text_chars))
    }

    /// 版（または有効な編集）の本文を、画面のページに分けた HTML にする。
    pub fn render_document(
        &self,
        work: LibraryId,
        scope: ResourceScope,
        provider: &str,
    ) -> Result<RenderedDocument> {
        let (document, text_chars) = self.reading_document(work, scope, provider)?;
        let (pages, source_page_starts) =
            piep_reader_render::document::render_pages(&view(&document)?, provider);
        Ok(RenderedDocument {
            pages,
            source_page_starts,
            text_chars,
        })
    }

    /// 携帯へ渡す読書用の文書と資産の一覧。文書には添付の本文を差し込み、取得元と字数を添える
    /// （携帯はこれで同じページに組む）。
    pub fn reading_bundle(
        &self,
        work: LibraryId,
        scope: ResourceScope,
    ) -> Result<piep_library_protocol::ReaderBundle> {
        let provider: String = self
            .reader()?
            .query_row(
                "SELECT provider FROM work_current_projection WHERE work_id=?1",
                rusqlite::params![work.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?
            .ok_or(crate::library::Error::ResourceUnavailable)?;
        let (document, text_chars) = self.reading_document(work, scope, &provider)?;
        self.bundle_of(
            &document,
            Some(serde_json::to_value(
                piep_reader_render::document::ReadingInfo {
                    provider,
                    text_chars,
                },
            )?),
        )
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

/// 共通の組み立てが読む形にする。携帯がサーバーから受け取る文書と同じ JSON を通す。
fn view(document: &ReaderDocument) -> Result<ReaderView> {
    ReaderView::from_value(serde_json::to_value(document)?)
        .map_err(crate::library::Error::Integrity)
}

/// 本文を HTML にする。画像は `data-local-path` にファイルの識別子を入れる。
pub fn render_html(document: &ReaderDocument) -> Result<String> {
    Ok(piep_reader_render::document::render_html(&view(document)?))
}

/// 変換が探す名前の仮の添付。
pub(crate) fn bound_assets(document: &ReaderDocument) -> Result<Vec<AssetEntry>> {
    Ok(piep_reader_render::document::bound_assets(&view(document)?).0)
}

#[cfg(test)]
#[path = "library_view_tests.rs"]
mod tests;
