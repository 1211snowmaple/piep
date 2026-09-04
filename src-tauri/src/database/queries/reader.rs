//! 読書と編集の問い合わせ。
//!
//! `mod.rs` から切り出した。あの一枚は 13,700 行あり、`impl Database` だけで
//! 163 個の関数を抱えている。**割り方はすでに前例があり**（`collections.rs`、
//! `collection_sweep.rs`、`archive_state.rs`、`assist_inputs.rs`）、本体が
//! 割られていないだけだった。`impl` は複数のファイルに分けて書けるので、
//! 公開 API は変えずに置き場所だけを移している。
//!
//! 一度に全部は割らない。何が壊れたのかを言えなくなる。

use super::{
    active_edit_revision_locked, blocks_for_revision_locked, blocks_to_fanbox_blocks,
    blocks_to_html, blocks_to_pixiv_text, blocks_to_plain_text, draft_edit_revision_locked,
    extract_search_body, get_work_edit_revision_locked, hash_blocks, html_to_editor_blocks,
    insert_work_blocks_locked, normalize_block_inputs, paginate_reader_html,
    plain_text_from_reader_html, reader_source_content, reader_version_path,
    reindex_download_locked, Database, EditedSourceForm, EditorDocument, ReaderCacheEntry,
    ReaderCacheKey, ReaderContentPage, ReaderDocument, ReaderMetadata, ReaderOutlineEntry,
    ReaderSearchHit, WorkBlockInput, WorkEditRevision, READER_CACHE_MAX_BYTES,
    READER_CACHE_MAX_DOCUMENTS, READER_CACHE_TICK, READER_CONTENT_CACHE,
};
use crate::database::parser;
use rusqlite::{params, OptionalExtension};
use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::Arc;
use unicode_normalization::UnicodeNormalization;

/// 本文内検索のための、**1 文字を 1 文字へ**畳む正規化。
///
/// 棚の検索は形態素解析まで通して表記ゆれを吸収するのに、本文内検索だけが
/// `to_lowercase` の部分一致だった。同じアプリの中で探し方の水準が二段違って
/// いたので、「カタカナ」で書かれた語を「かたかな」で探すと出てこなかった。
///
/// ここで畳み方を 1 対 1 に留めているのは、見つけた位置をそのまま元の本文の
/// 文字位置として使うため。長さが変わる畳み方（半角カナの濁点など）は
/// 扱わない ―― 位置がずれるほうが、少し見つからないことより困る。
pub(super) fn fold_char(ch: char) -> char {
    if ch == '\u{3000}' {
        return ' ';
    }
    // 全角英数などは NFKC で半角へ。1 文字に収まるものだけを受け取る。
    let single = ch.to_string();
    let compat = {
        let mut chars = single.nfkc();
        match (chars.next(), chars.next()) {
            (Some(only), None) => only,
            _ => ch,
        }
    };
    let lowered = {
        let mut chars = compat.to_lowercase();
        match (chars.next(), chars.next()) {
            (Some(only), None) => only,
            _ => compat,
        }
    };
    // カタカナはひらがなへ寄せる。ヴ（U+30F4）までを対象にする。
    match lowered {
        'ァ'..='ヶ' => char::from_u32(lowered as u32 - 0x60).unwrap_or(lowered),
        other => other,
    }
}

static READER_HEADING: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?is)<h2\b[^>]*>(.*?)</h2>").expect("valid heading regex")
});

pub(super) fn fold_for_reader_search(query: &str) -> String {
    query.chars().map(fold_char).collect()
}

/// 畳んだ本文の中で、畳んだ検索語が始まる文字位置をすべて返す。
///
/// 重なる一致は数えない。「ああ」を「あああ」から 2 件と数えると、画面側が
/// 入れ子の印を描くことになり、一覧の件数と本文の印がずれる。
pub(super) fn folded_match_positions(haystack: &[char], needle: &str) -> Vec<usize> {
    let needle = needle.chars().collect::<Vec<_>>();
    if needle.is_empty() || haystack.len() < needle.len() {
        return Vec::new();
    }
    let mut positions = Vec::new();
    let mut start = 0;
    while start + needle.len() <= haystack.len() {
        if haystack[start..start + needle.len()] == needle[..] {
            positions.push(start);
            start += needle.len();
        } else {
            start += 1;
        }
    }
    positions
}

impl Database {
    pub fn get_reader_metadata(&self, download_id: i64) -> Result<ReaderMetadata, String> {
        let download = self.get_download(download_id)?;
        let versions = self.get_versions(download_id)?;
        let conn = self.read_conn()?;
        let active_edit_revision = active_edit_revision_locked(&conn, download_id)?;
        Ok(ReaderMetadata {
            asset_count: download.asset_count,
            download,
            versions,
            is_edited: active_edit_revision.is_some(),
            active_edit_revision,
        })
    }

    /// `include_plain_text` を落とすと、本文の平文を積まずに返す。
    ///
    /// 読書画面は HTML しか使わないのに、ページを繰るたび同じ本文を平文でも
    /// 運んでいた。転送量がほぼ倍になっていたぶん、ページ送りが重かった。
    pub fn get_reader_content_page(
        &self,
        download_id: i64,
        version: Option<i64>,
        page: usize,
        include_plain_text: bool,
    ) -> Result<ReaderContentPage, String> {
        let content = self.get_cached_reader_content(download_id, version)?;
        let page_count = content.pages.len().max(1);
        let page = page.min(page_count.saturating_sub(1));
        let html = content.pages.get(page).cloned().unwrap_or_default();
        Ok(ReaderContentPage {
            page,
            page_count,
            plain_text: if include_plain_text {
                plain_text_from_reader_html(&html)
            } else {
                String::new()
            },
            html,
            total_plain_text_chars: content.total_plain_text_chars,
            source_page_starts: content.source_page_starts.as_ref().clone(),
        })
    }

    /// 作品全体の見出しと、それが載っているページ。
    ///
    /// `[chapter:]` は読書画面では見出しになるのに、章へ飛ぶ手立てが無かった。
    /// EPUB では目次を組んでいるのに、読むときだけ目次が無い状態だった。
    pub fn get_reader_outline(
        &self,
        download_id: i64,
        version: Option<i64>,
    ) -> Result<Vec<ReaderOutlineEntry>, String> {
        let content = self.get_cached_reader_content(download_id, version)?;
        let mut entries = Vec::new();
        for (page, html) in content.pages.iter().enumerate() {
            for (index, captures) in READER_HEADING.captures_iter(html).enumerate() {
                let title = plain_text_from_reader_html(&captures[1]);
                let title = title.trim();
                if title.is_empty() {
                    continue;
                }
                entries.push(ReaderOutlineEntry {
                    page: page + 1,
                    index,
                    title: title.to_string(),
                });
                // 目次が本文より長くなっては意味がない。
                if entries.len() >= 2_000 {
                    return Ok(entries);
                }
            }
        }
        Ok(entries)
    }

    pub fn search_reader_content(
        &self,
        download_id: i64,
        version: Option<i64>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ReaderSearchHit>, String> {
        let needle = fold_for_reader_search(query.trim());
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        let content = self.get_cached_reader_content(download_id, version)?;
        let mut hits = Vec::new();
        for (page, html) in content.pages.iter().enumerate() {
            let plain = plain_text_from_reader_html(html);
            let chars = plain.chars().collect::<Vec<_>>();
            // 1 文字が 1 文字に対応する畳み方なので、畳んだ側で見つけた位置は
            // そのまま元の本文の文字位置になる。
            let folded = chars.iter().map(|ch| fold_char(*ch)).collect::<Vec<_>>();
            let positions = folded_match_positions(&folded, &needle);
            if positions.is_empty() {
                continue;
            }
            let char_index = positions[0];
            let start = char_index.saturating_sub(42);
            let end = (char_index + needle.chars().count() + 70).min(chars.len());
            let snippet = format!(
                "{}{}{}",
                if start > 0 { "…" } else { "" },
                chars[start..end].iter().collect::<String>(),
                if end < chars.len() { "…" } else { "" }
            );
            hits.push(ReaderSearchHit {
                page: page + 1,
                snippet,
                count: positions.len(),
            });
            if hits.len() >= limit.clamp(1, 200) {
                break;
            }
        }
        Ok(hits)
    }

    fn get_cached_reader_content(
        &self,
        download_id: i64,
        version: Option<i64>,
    ) -> Result<ReaderCacheEntry, String> {
        let download = self.get_download(download_id)?;
        let versions = self.get_versions(download_id)?;
        let conn = self.read_conn()?;
        let active_edit = if version.is_none() {
            active_edit_revision_locked(&conn, download_id)?
        } else {
            None
        };
        let stamp = if let Some(edit) = &active_edit {
            format!(
                "edit:{}:{}:{}",
                edit.id, edit.updated_at, download.asset_count
            )
        } else {
            let target_version = version.unwrap_or(download.current_version);
            let target_path = reader_version_path(&download, &versions, target_version);
            let metadata = std::fs::metadata(&target_path).ok();
            let modified = metadata
                .as_ref()
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .unwrap_or(0);
            format!(
                "source:{target_version}:{}:{modified}:{}:{}",
                metadata.as_ref().map(|meta| meta.len()).unwrap_or(0),
                download.content_hash.as_deref().unwrap_or(""),
                download.asset_count
            )
        };
        let key = ReaderCacheKey {
            storage: self.storage_dir.clone(),
            download_id,
            requested_version: version,
            stamp,
        };
        let tick = READER_CACHE_TICK.fetch_add(1, AtomicOrdering::Relaxed);
        let cache = READER_CONTENT_CACHE.get_or_init(Default::default);
        if let Some(entry) = cache.lock().get_mut(&key) {
            entry.last_used = tick;
            return Ok(entry.clone());
        }

        let assets = self.get_assets(download_id)?;
        let (html, plain_text) = if let Some(edit) = active_edit {
            let blocks = blocks_for_revision_locked(&conn, edit.id)?;
            (
                blocks_to_html(&blocks, &assets),
                blocks_to_plain_text(&blocks),
            )
        } else {
            drop(conn);
            reader_source_content(self, &download, &versions, version, &assets)?
        };
        let (pages, source_page_starts) = paginate_reader_html(&html, &download.source);
        let pages = Arc::new(pages);
        let entry = ReaderCacheEntry {
            bytes: pages.iter().map(String::len).sum::<usize>() + plain_text.len(),
            pages,
            source_page_starts: Arc::new(source_page_starts),
            total_plain_text_chars: plain_text.chars().count(),
            last_used: tick,
        };
        let mut cache = cache.lock();
        cache.retain(|candidate, _| {
            candidate.storage != key.storage
                || candidate.download_id != download_id
                || candidate.requested_version != version
                || candidate == &key
        });
        cache.insert(key, entry.clone());
        while cache.len() > READER_CACHE_MAX_DOCUMENTS
            || cache.values().map(|entry| entry.bytes).sum::<usize>() > READER_CACHE_MAX_BYTES
        {
            let Some(oldest) = cache
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            // Keep one oversized current document cached; otherwise the next
            // page request would immediately parse it again.
            if cache.len() == 1 {
                break;
            }
            cache.remove(&oldest);
        }
        Ok(entry)
    }

    pub fn get_reader_document(
        &self,
        download_id: i64,
        version: Option<i64>,
    ) -> Result<ReaderDocument, String> {
        let download = self.get_download(download_id)?;
        let assets = self.get_assets(download_id)?;
        let versions = self.get_versions(download_id)?;

        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let active_edit = active_edit_revision_locked(&conn, download_id)?;
        if version.is_none() {
            if let Some(edit) = active_edit.clone() {
                let blocks = blocks_for_revision_locked(&conn, edit.id)?;
                return Ok(ReaderDocument {
                    download,
                    assets: assets.clone(),
                    versions,
                    html: blocks_to_html(&blocks, &assets),
                    plain_text: blocks_to_plain_text(&blocks),
                    is_edited: true,
                    active_edit_revision: Some(edit),
                });
            }
        }
        drop(conn);

        let target_version = version.unwrap_or(download.current_version);
        let raw_json = self.read_download_json_for_version(&download, &versions, target_version)?;
        let html = if download.source == "pixiv" {
            parser::parse_pixiv_to_html(&raw_json, &assets)
        } else if download.source == "fanbox" {
            parser::parse_fanbox_to_html(&raw_json, &assets)
        } else {
            String::new()
        };
        let plain_text = serde_json::from_str::<serde_json::Value>(&raw_json)
            .ok()
            .map(|value| extract_search_body(&value, &download.source))
            .unwrap_or_default();

        Ok(ReaderDocument {
            download,
            assets,
            versions,
            html,
            plain_text,
            is_edited: false,
            active_edit_revision: active_edit,
        })
    }

    /// 反映済みの編集を、取得元と同じ書式へ組み直して返す。
    ///
    /// 何も反映されていなければ `None`。EPUB の書き出しはここを通ることで、
    /// 挿絵・改ページ・見出し・区切りを保ったまま編集版を本にできる。
    /// 平文だけを渡していたころは、そのすべてが落ちていた。
    pub fn active_edit_source_form(
        &self,
        download_id: i64,
    ) -> Result<Option<EditedSourceForm>, String> {
        let assets = self.get_assets(download_id)?;
        let conn = self.read_conn()?;
        let Some(revision) = active_edit_revision_locked(&conn, download_id)? else {
            return Ok(None);
        };
        let blocks = blocks_for_revision_locked(&conn, revision.id)?;
        drop(conn);
        Ok(Some(EditedSourceForm {
            pixiv_text: blocks_to_pixiv_text(&blocks, &assets),
            fanbox_blocks: blocks_to_fanbox_blocks(&blocks, &assets),
            plain_text: blocks_to_plain_text(&blocks),
        }))
    }

    pub fn get_editor_document(&self, download_id: i64) -> Result<EditorDocument, String> {
        let download = self.get_download(download_id)?;
        let assets = self.get_assets(download_id)?;
        let versions = self.get_versions(download_id)?;
        let base_version = versions
            .iter()
            .map(|v| v.version)
            .max()
            .unwrap_or(download.current_version);
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let active_revision = active_edit_revision_locked(&conn, download_id)?;
        let draft_revision = draft_edit_revision_locked(&conn, download_id)?;

        if let Some(draft) = draft_revision.clone() {
            let blocks = blocks_for_revision_locked(&conn, draft.id)?;
            return Ok(EditorDocument {
                download,
                assets,
                active_revision,
                draft_revision: Some(draft),
                base_version,
                blocks,
            });
        }

        if let Some(active) = active_revision.clone() {
            let blocks = blocks_for_revision_locked(&conn, active.id)?;
            return Ok(EditorDocument {
                download,
                assets,
                active_revision: Some(active),
                draft_revision: None,
                base_version,
                blocks,
            });
        }
        drop(conn);

        let raw_json =
            self.read_download_json_for_version(&download, &versions, download.current_version)?;
        let source_html = if download.source == "pixiv" {
            parser::parse_pixiv_to_html(&raw_json, &assets)
        } else if download.source == "fanbox" {
            parser::parse_fanbox_to_html(&raw_json, &assets)
        } else {
            String::new()
        };
        let blocks = html_to_editor_blocks(&source_html, &assets);

        Ok(EditorDocument {
            download,
            assets,
            active_revision,
            draft_revision: None,
            base_version,
            blocks,
        })
    }

    /// `title` は、取得元の題を書き換えたいときだけ渡す。
    ///
    /// 空文字や元の題と同じものは持たない。持たせておくと、取得元が題を
    /// 直したときに「直っていない古い題」で上書きし続けることになる。
    pub fn save_work_draft(
        &self,
        download_id: i64,
        base_version: i64,
        title: Option<&str>,
        blocks: &[WorkBlockInput],
    ) -> Result<WorkEditRevision, String> {
        let source_title: String = {
            let conn = self.read_conn()?;
            conn.query_row(
                "SELECT title FROM downloads WHERE id = ?1",
                params![download_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to read the source title: {e}"))?
        };
        let title = title
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != source_title.trim());
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        let normalized_blocks = normalize_block_inputs(blocks);
        let content_hash = hash_blocks(&normalized_blocks);
        let tx = conn
            .transaction()
            .map_err(|e| format!("Editor transaction begin failed: {}", e))?;

        // 下書きは 1 本を書き換え続ける。保存のたびに版を起こして前の版を
        // 書庫送りにしていたころは、**自動保存を入れると履歴が下書きで
        // 埋まる**うえ、何も変えずに保存し直しただけで版が増えていた。
        // 反映済みの版（`active`）とは別の行なので、これで失うものはない。
        let existing_draft: Option<i64> = tx
            .query_row(
                "SELECT id FROM work_edit_revisions
                 WHERE download_id = ?1 AND status = 'draft'
                 ORDER BY updated_at DESC, id DESC
                 LIMIT 1",
                params![download_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| format!("Failed to read the current draft: {}", e))?;

        let revision_id = match existing_draft {
            Some(revision_id) => {
                // 版を起こしていたころの取りこぼしが残っていることがある。
                // 下書きは 1 本に収束させる。
                tx.execute(
                    "UPDATE work_edit_revisions
                     SET status = 'archived', updated_at = ?3
                     WHERE download_id = ?1 AND status = 'draft' AND id <> ?2",
                    params![download_id, revision_id, now],
                )
                .map_err(|e| format!("Failed to archive stale drafts: {}", e))?;
                tx.execute(
                    "UPDATE work_edit_revisions
                     SET base_version = ?2, content_hash = ?3, updated_at = ?4, title = ?5
                     WHERE id = ?1",
                    params![revision_id, base_version, content_hash, now, title],
                )
                .map_err(|e| format!("Failed to update the draft revision: {}", e))?;
                // 表の名前は `work_edit_blocks`。`work_blocks` を消そうとして
                // いたので、**二度目からの下書き保存が毎回失敗していた** ――
                // 自動保存は同じ1本を書き換え続ける作りなので、最初の一回より
                // あとに書いたものは、どこにも残らなかった。
                tx.execute(
                    "DELETE FROM work_edit_blocks WHERE edit_revision_id = ?1",
                    params![revision_id],
                )
                .map_err(|e| format!("Failed to clear the draft blocks: {}", e))?;
                revision_id
            }
            None => {
                tx.execute(
                    "INSERT INTO work_edit_revisions (
                        download_id, base_version, status, title, content_hash, created_at, updated_at
                     ) VALUES (?1, ?2, 'draft', ?5, ?3, ?4, ?4)",
                    params![download_id, base_version, content_hash, now, title],
                )
                .map_err(|e| format!("Failed to insert draft revision: {}", e))?;
                tx.last_insert_rowid()
            }
        };

        insert_work_blocks_locked(&tx, revision_id, &normalized_blocks)?;
        tx.commit()
            .map_err(|e| format!("Editor transaction commit failed: {}", e))?;

        get_work_edit_revision_locked(&conn, revision_id)
    }

    /// 書きかけを捨てて、取り込んだままの本文（反映中ならその版）へ戻す。
    ///
    /// 捨てる口がどこにも無かった。自動保存は触りはじめて6秒で下書きを作るので、
    /// 一度でも編集画面を開いて何か触れば、**そこから先はいつ開いても書きかけが
    /// 出てくる**。元の本文に戻る道は、手で全部打ち直すことしかなかった。
    pub fn discard_work_draft(&self, download_id: i64) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Editor transaction begin failed: {}", e))?;
        tx.execute(
            "DELETE FROM work_edit_blocks
             WHERE edit_revision_id IN (
                 SELECT id FROM work_edit_revisions WHERE download_id = ?1 AND status = 'draft'
             )",
            params![download_id],
        )
        .map_err(|e| format!("Failed to clear the draft blocks: {}", e))?;
        tx.execute(
            "DELETE FROM work_edit_revisions WHERE download_id = ?1 AND status = 'draft'",
            params![download_id],
        )
        .map_err(|e| format!("Failed to delete the draft revision: {}", e))?;
        tx.commit()
            .map_err(|e| format!("Editor transaction commit failed: {}", e))
    }

    pub fn activate_work_edit(&self, edit_revision_id: i64) -> Result<WorkEditRevision, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        let revision = get_work_edit_revision_locked(&conn, edit_revision_id)?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Editor transaction begin failed: {}", e))?;
        tx.execute(
            "UPDATE work_edit_revisions
             SET status = 'archived', updated_at = ?2
             WHERE download_id = ?1 AND status = 'active'",
            params![revision.download_id, now],
        )
        .map_err(|e| format!("Failed to archive active revision: {}", e))?;
        tx.execute(
            "UPDATE work_edit_revisions
             SET status = 'active', updated_at = ?2
             WHERE id = ?1",
            params![edit_revision_id, now],
        )
        .map_err(|e| format!("Failed to activate revision: {}", e))?;
        tx.execute(
            "DELETE FROM search_index_state WHERE download_id = ?1",
            params![revision.download_id],
        )
        .map_err(|e| format!("Failed to invalidate search index: {}", e))?;
        tx.commit()
            .map_err(|e| format!("Editor transaction commit failed: {}", e))?;

        reindex_download_locked(&conn, &self.storage_dir, revision.download_id)?;
        get_work_edit_revision_locked(&conn, edit_revision_id)
    }

    /// 反映した編集を下ろし、取り込んだままの本文へ戻す。
    ///
    /// 版そのものは残す。もう一度反映できるし、履歴からも消えない。戻る道が
    /// 無かったころは、一度反映してしまうと編集前の本文を読む手立てが
    /// 「版の選択で同じ番号をもう一度選ぶ」という、誰にも分からない操作
    /// しかなかった。
    pub fn deactivate_work_edit(&self, download_id: i64) -> Result<(), String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        let tx = conn
            .transaction()
            .map_err(|e| format!("Editor transaction begin failed: {}", e))?;
        let archived = tx
            .execute(
                "UPDATE work_edit_revisions
                 SET status = 'archived', updated_at = ?2
                 WHERE download_id = ?1 AND status = 'active'",
                params![download_id, now],
            )
            .map_err(|e| format!("Failed to archive active revision: {}", e))?;
        if archived == 0 {
            return Ok(());
        }
        tx.execute(
            "DELETE FROM search_index_state WHERE download_id = ?1",
            params![download_id],
        )
        .map_err(|e| format!("Failed to invalidate search index: {}", e))?;
        tx.commit()
            .map_err(|e| format!("Editor transaction commit failed: {}", e))?;

        reindex_download_locked(&conn, &self.storage_dir, download_id)
    }
}
