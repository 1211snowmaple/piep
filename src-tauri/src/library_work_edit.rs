//! 新しい棚の作品の編集画面（評価文書 4.1 の 4）。
//!
//! 編集画面は旧棚の段（`WorkBlock`）の形のまま使う。開くときは下書き、反映中の版、今の版の本文の
//! 順に段へ直し、画像は画面の中だけの番号（1 から）で指す。保存するときは段を編集文書
//! （`EditContent`）へ組み、画像は中身の識別子で持つ（枠の名前は識別子そのもの）。今の版の本文から
//! 段を作るのは旧棚の変換（`html_to_editor_blocks`）で、読む画面と同じ HTML を通す。
//!
//! 自動保存は数秒ごとに下書きを送るので、中身が今の下書きと同じなら書かない（段の ID は作品と
//! 順番から決めるので、同じ中身は同じ文書になる）。

use crate::database::models::{WorkBlock, WorkBlockInput};
use crate::database::queries::{html_to_editor_blocks, normalize_block_inputs};
use crate::library::{
    EditBlock, EditContent, EditMedia, EditPointerChange, EditRequest, EditState, Error,
    Fingerprint, Library, LibraryId, ResourceScope, Result,
};
use crate::library_view::{bound_assets, render_html};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// 編集の版の一つ（下書きか反映中）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorRevision {
    pub id: LibraryId,
    /// 基準の版の番号（取得元の改稿の順）。
    pub base_version: i64,
    /// 編集で付け直した題。付け直していなければ None。
    pub title: Option<String>,
    /// ミリ秒。
    pub created_at: i64,
}

/// 編集画面が使う画像。`id` は画面の中だけの番号。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EditorMedia {
    pub id: i64,
    /// ファイルの識別子（16 進）。
    pub file: String,
    pub name: String,
    pub mime_type: Option<String>,
}

/// 編集画面を開くときの中身。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorView {
    /// 段の基準の版。保存するときにそのまま返す。
    pub base: LibraryId,
    pub base_version: i64,
    /// 編集の世代。保存するときにそのまま返す。
    pub generation: i64,
    pub active: Option<EditorRevision>,
    pub draft: Option<EditorRevision>,
    /// 取得元の題（今の版）。
    pub source_title: String,
    pub blocks: Vec<WorkBlock>,
    pub media: Vec<EditorMedia>,
}

/// 作品の画面に出す編集のようす：下書き・反映中の版・以前の編集版（新しい順）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditHistory {
    /// 編集の世代。指し先を変えるときにそのまま返す。
    pub generation: i64,
    pub active: Option<EditorRevision>,
    pub draft: Option<EditorRevision>,
    /// 反映から外れた版。もう一度反映するか、一覧から外せる。
    pub archived: Vec<EditorRevision>,
}

/// 編集画面からの保存。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorSave {
    pub base: LibraryId,
    pub expected: i64,
    /// 付け直した題。空や取得元の題と同じものは持たない。
    pub title: Option<String>,
    pub blocks: Vec<WorkBlockInput>,
    pub media: Vec<EditorMedia>,
    /// 保存した下書きをそのまま反映するか。
    pub publish: bool,
}

/// 作品と順番から決まる段の ID。同じ中身の文書は同じ識別子になる。
fn block_id(work: LibraryId, ordinal: usize) -> LibraryId {
    let mut seed = work.as_bytes().to_vec();
    seed.extend_from_slice(b"\0edit-block\0");
    seed.extend_from_slice(&(ordinal as u64).to_be_bytes());
    let digest = Fingerprint::of(&seed);
    LibraryId::from_slice(&digest.0[..16]).expect("16 バイト")
}

/// 要求から決まる、続けて書く二つ目の要求の ID（送り直しで同じになる）。
fn follow_up(request: LibraryId, label: &str) -> LibraryId {
    let mut seed = request.as_bytes().to_vec();
    seed.extend_from_slice(label.as_bytes());
    LibraryId::from_slice(&Fingerprint::of(&seed).0[..16]).expect("16 バイト")
}

fn attrs_value(attrs: Option<&str>) -> Value {
    attrs
        .and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or(Value::Null)
}

impl Library {
    fn source_title(&self, work: LibraryId) -> Result<String> {
        Ok(self
            .snapshots(work)?
            .last()
            .and_then(|snapshot| snapshot.metadata.get("title").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string())
    }

    fn editor_revision(&self, work: LibraryId, edit: LibraryId) -> Result<EditorRevision> {
        let info = self.edit(work, edit)?;
        Ok(EditorRevision {
            id: edit,
            base_version: self.edit_base_version(work, edit)?,
            title: info.content.title,
            created_at: info.created_at,
        })
    }

    /// 作品の編集のようす（本文は組まない）。
    pub fn edit_history(&self, work: LibraryId) -> Result<EditHistory> {
        let EditState {
            active,
            draft,
            generation,
        } = self.edit_state(work, self.id)?;
        let archived: Vec<Vec<u8>> = {
            let conn = self.reader()?;
            let mut stmt = conn.prepare(
                "SELECT a.edit_id FROM edit_archive a JOIN edit_revisions r ON r.id=a.edit_id
                 WHERE a.work_id=?1 AND a.user_id=?2 ORDER BY r.created_at DESC,r.id DESC",
            )?;
            let rows = stmt.query_map(
                [work.as_bytes().as_slice(), self.id.as_bytes().as_slice()],
                |r| r.get(0),
            )?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let revision = |edit: Option<LibraryId>| {
            edit.map(|edit| self.editor_revision(work, edit))
                .transpose()
        };
        Ok(EditHistory {
            generation,
            active: revision(active)?,
            draft: revision(draft)?,
            archived: archived
                .iter()
                .map(|edit| {
                    let edit = LibraryId::from_slice(edit)
                        .map_err(|_| Error::Integrity("編集の ID が不正です".into()))?;
                    self.editor_revision(work, edit)
                })
                .collect::<Result<_>>()?,
        })
    }

    /// 編集画面を開く。下書き、反映中の版、今の版の本文の順に段へ直す。
    pub fn editor_view(&self, work: LibraryId) -> Result<EditorView> {
        let EditState {
            active,
            draft,
            generation,
        } = self.edit_state(work, self.id)?;
        let source_title = self.source_title(work)?;
        let active_view = active
            .map(|edit| self.editor_revision(work, edit))
            .transpose()?;
        let draft_view = draft
            .map(|edit| self.editor_revision(work, edit))
            .transpose()?;
        if let Some(edit) = draft.or(active) {
            let info = self.edit(work, edit)?;
            let ids: BTreeMap<&str, i64> = info
                .content
                .media
                .iter()
                .enumerate()
                .map(|(index, media)| (media.slot.as_str(), index as i64 + 1))
                .collect();
            let media = info
                .content
                .media
                .iter()
                .enumerate()
                .map(|(index, media)| EditorMedia {
                    id: index as i64 + 1,
                    file: media.file.to_string(),
                    name: media.original_name.clone(),
                    mime_type: media.mime_type.clone(),
                })
                .collect();
            let blocks = info
                .content
                .blocks
                .iter()
                .enumerate()
                .map(|(index, block)| WorkBlock {
                    id: index as i64,
                    edit_revision_id: 0,
                    order: index as i64,
                    block_type: block.kind.clone(),
                    text: block.text.clone(),
                    asset_id: block
                        .media_slot
                        .as_deref()
                        .and_then(|slot| ids.get(slot).copied()),
                    attrs_json: (!block.attrs.is_null()).then(|| block.attrs.to_string()),
                })
                .collect();
            let base_version = self.edit_base_version(work, edit)?;
            return Ok(EditorView {
                base: info.base,
                base_version,
                generation,
                active: active_view,
                draft: draft_view,
                source_title,
                blocks,
                media,
            });
        }

        let base = self.current_snapshot(work)?;
        let base_version = self
            .snapshots(work)?
            .into_iter()
            .find(|snapshot| snapshot.id == base)
            .map(|snapshot| snapshot.revision)
            .ok_or(Error::ResourceUnavailable)?;
        let (html, assets) = match self.reader_document(work, ResourceScope::Snapshot(base)) {
            Ok(document) => (render_html(&document), bound_assets(&document).0),
            // 本文の無い作品（公開範囲の外の投稿など）は空の本文から書く。
            Err(Error::ResourceUnavailable) => (String::new(), Vec::new()),
            Err(error) => return Err(error),
        };
        // 変換が返す画像の番号（仮の添付の並び）を、ファイルごとの画面の番号へ。
        let mut media: Vec<EditorMedia> = Vec::new();
        let mut renumber: BTreeMap<i64, i64> = BTreeMap::new();
        for asset in &assets {
            let file = asset
                .local_path
                .split('/')
                .next()
                .unwrap_or_default()
                .to_string();
            let id = match media.iter().find(|item| item.file == file) {
                Some(item) => item.id,
                None => {
                    let id = media.len() as i64 + 1;
                    media.push(EditorMedia {
                        id,
                        file,
                        name: asset.filename.clone(),
                        mime_type: asset.mime_type.clone(),
                    });
                    id
                }
            };
            renumber.insert(asset.id, id);
        }
        let blocks = html_to_editor_blocks(&html, &assets)
            .into_iter()
            .enumerate()
            .map(|(index, mut block)| {
                block.id = index as i64;
                block.order = index as i64;
                block.asset_id = block.asset_id.and_then(|id| renumber.get(&id).copied());
                block
            })
            .collect();
        Ok(EditorView {
            base,
            base_version,
            generation,
            active: active_view,
            draft: draft_view,
            source_title,
            blocks,
            media,
        })
    }

    /// 編集画面の段を下書きとして保存し、`publish` なら続けて反映する。保存した後の世代を返す。
    pub fn save_editor(
        &self,
        request: LibraryId,
        work: LibraryId,
        save: EditorSave,
    ) -> Result<EditState> {
        let source_title = self.source_title(work)?;
        let title = save
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty() && *title != source_title.trim())
            .map(str::to_owned);
        let by_id: BTreeMap<i64, &EditorMedia> =
            save.media.iter().map(|media| (media.id, media)).collect();
        let mut media: Vec<EditMedia> = Vec::new();
        let mut blocks = Vec::new();
        for (index, block) in normalize_block_inputs(&save.blocks).into_iter().enumerate() {
            let slot = match block.asset_id {
                Some(id) => {
                    let item = by_id
                        .get(&id)
                        .ok_or_else(|| Error::Integrity("編集の画像が見つかりません".into()))?;
                    let file = Fingerprint::parse(&item.file)
                        .map_err(|_| Error::Integrity("画像の識別子が不正です".into()))?;
                    // 棚に無いファイルは指せない。
                    self.file(file)?;
                    if !media.iter().any(|entry| entry.slot == item.file) {
                        media.push(EditMedia {
                            slot: item.file.clone(),
                            file,
                            original_name: item.name.clone(),
                            mime_type: item.mime_type.clone(),
                        });
                    }
                    Some(item.file.clone())
                }
                None => None,
            };
            blocks.push(EditBlock {
                id: block_id(work, index),
                ordinal: index as i64,
                kind: block.block_type,
                text: block.text,
                attrs: attrs_value(block.attrs_json.as_deref()),
                media_slot: slot,
            });
        }
        media.sort_by(|left, right| left.slot.cmp(&right.slot));
        let content = EditContent {
            format: "piep-edit-v1".into(),
            title,
            blocks,
            media,
            provenance: None,
        };
        let state = self.edit_state(work, self.id)?;
        if state.generation != save.expected {
            return Err(Error::Integrity("編集中に別の変更が保存されました".into()));
        }
        // 今の下書きと同じ中身（同じ基準）なら書かない。
        let unchanged = match state.draft {
            Some(draft) => {
                let info = self.edit(work, draft)?;
                info.base == save.base && info.content == content
            }
            None => false,
        };
        let mut state = state;
        if !unchanged {
            self.save_edit(EditRequest {
                request_id: request,
                work,
                user: self.id,
                base: save.base,
                parent: state.draft.or(state.active),
                expected: save.expected,
                content: &content,
                activate: false,
            })?;
            state = self.edit_state(work, self.id)?;
        }
        if save.publish {
            state = self.change_edit_pointers(
                follow_up(request, "publish"),
                work,
                self.id,
                state.generation,
                EditPointerChange::Publish,
            )?;
        }
        Ok(state)
    }
}

#[cfg(test)]
#[path = "library_work_edit_tests.rs"]
mod tests;
