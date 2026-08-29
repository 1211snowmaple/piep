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
    active_edit_revision_locked, blocks_for_revision_locked, blocks_to_html, blocks_to_plain_text,
    draft_edit_revision_locked, extract_search_body, get_work_edit_revision_locked, hash_blocks,
    html_to_editor_blocks, insert_work_blocks_locked, normalize_block_inputs, paginate_reader_html,
    plain_text_from_reader_html, reader_source_content, reader_version_path,
    reindex_download_locked, Database, EditorDocument, ReaderCacheEntry, ReaderCacheKey,
    ReaderContentPage, ReaderDocument, ReaderMetadata, ReaderSearchHit, WorkBlockInput,
    WorkEditRevision, READER_CACHE_MAX_BYTES, READER_CACHE_MAX_DOCUMENTS, READER_CACHE_TICK,
    READER_CONTENT_CACHE,
};
use crate::database::parser;
use rusqlite::params;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::Arc;

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

    pub fn get_reader_content_page(
        &self,
        download_id: i64,
        version: Option<i64>,
        page: usize,
    ) -> Result<ReaderContentPage, String> {
        let content = self.get_cached_reader_content(download_id, version)?;
        let page_count = content.pages.len().max(1);
        let page = page.min(page_count.saturating_sub(1));
        let html = content.pages.get(page).cloned().unwrap_or_default();
        Ok(ReaderContentPage {
            page,
            page_count,
            plain_text: plain_text_from_reader_html(&html),
            html,
            total_plain_text_chars: content.total_plain_text_chars,
        })
    }

    pub fn search_reader_content(
        &self,
        download_id: i64,
        version: Option<i64>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ReaderSearchHit>, String> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let content = self.get_cached_reader_content(download_id, version)?;
        let mut hits = Vec::new();
        for (page, html) in content.pages.iter().enumerate() {
            let plain = plain_text_from_reader_html(html);
            let normalized = plain.to_lowercase();
            let count = normalized.match_indices(&query).count();
            if count == 0 {
                continue;
            }
            let byte_index = normalized.find(&query).unwrap_or(0);
            let char_index = normalized[..byte_index].chars().count();
            let chars = plain.chars().collect::<Vec<_>>();
            let start = char_index.saturating_sub(42);
            let end = (char_index + query.chars().count() + 70).min(chars.len());
            let snippet = format!(
                "{}{}{}",
                if start > 0 { "…" } else { "" },
                chars[start..end].iter().collect::<String>(),
                if end < chars.len() { "…" } else { "" }
            );
            hits.push(ReaderSearchHit {
                page: page + 1,
                snippet,
                count,
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
        let pages = Arc::new(paginate_reader_html(&html, &download.source));
        let entry = ReaderCacheEntry {
            bytes: pages.iter().map(String::len).sum::<usize>() + plain_text.len(),
            pages,
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

    pub fn save_work_draft(
        &self,
        download_id: i64,
        base_version: i64,
        blocks: &[WorkBlockInput],
    ) -> Result<WorkEditRevision, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        let normalized_blocks = normalize_block_inputs(blocks);
        let content_hash = hash_blocks(&normalized_blocks);
        let tx = conn
            .transaction()
            .map_err(|e| format!("Editor transaction begin failed: {}", e))?;

        tx.execute(
            "UPDATE work_edit_revisions
             SET status = 'archived', updated_at = ?2
             WHERE download_id = ?1 AND status = 'draft'",
            params![download_id, now],
        )
        .map_err(|e| format!("Failed to archive previous draft: {}", e))?;

        tx.execute(
            "INSERT INTO work_edit_revisions (
                download_id, base_version, status, title, content_hash, created_at, updated_at
             ) VALUES (?1, ?2, 'draft', NULL, ?3, ?4, ?4)",
            params![download_id, base_version, content_hash, now],
        )
        .map_err(|e| format!("Failed to insert draft revision: {}", e))?;
        let revision_id = tx.last_insert_rowid();

        insert_work_blocks_locked(&tx, revision_id, &normalized_blocks)?;
        tx.commit()
            .map_err(|e| format!("Editor transaction commit failed: {}", e))?;

        get_work_edit_revision_locked(&conn, revision_id)
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
}
