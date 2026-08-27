//! Portable user-authored state that belongs in a library backup.
//!
//! Database row ids are installation-local. These shapes use work-relative asset paths and
//! stable provider keys so a restore can allocate fresh ids without losing edits or decisions.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::Database;
use crate::database::models::SavedSearch;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PortableTag {
    pub name: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortableWorkEdit {
    pub base_version: i64,
    pub status: String,
    pub title: Option<String>,
    pub content_hash: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub blocks: Vec<PortableWorkEditBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortableWorkEditBlock {
    pub order: i64,
    pub block_type: String,
    pub text: Option<String>,
    /// Export changes this live absolute path to a storage-relative path. Restore resolves it
    /// back to the promoted file before calling `restore_work_edits`.
    pub asset_path: Option<String>,
    pub attrs_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PortableCollectionPairFeedback {
    pub left_source: String,
    pub left_source_id: String,
    pub right_source: String,
    pub right_source_id: String,
    pub decision: String,
    pub rule_version: String,
    pub updated_at: String,
}

fn valid_tag_source(source: &str) -> bool {
    matches!(source, "origin" | "manual" | "llm")
}

fn valid_edit_status(status: &str) -> bool {
    matches!(status, "draft" | "active" | "archived")
}

fn valid_pair_decision(decision: &str) -> bool {
    matches!(decision, "accept" | "reject")
}

impl Database {
    pub fn archive_tags(&self, download_id: i64) -> Result<Vec<PortableTag>, String> {
        let conn = self.read_conn()?;
        let mut statement = conn
            .prepare(
                "SELECT t.name, dt.tag_source FROM download_tags dt
                 JOIN tags t ON t.id = dt.tag_id
                 WHERE dt.download_id = ?1
                 ORDER BY t.name COLLATE NOCASE",
            )
            .map_err(|error| format!("Failed to prepare backup tags: {error}"))?;
        let rows = statement
            .query_map(params![download_id], |row| {
                Ok(PortableTag {
                    name: row.get(0)?,
                    source: row.get(1)?,
                })
            })
            .map_err(|error| format!("Failed to query backup tags: {error}"))?;
        let collected = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to read backup tags: {error}"))?;
        Ok(collected)
    }

    pub fn restore_tags(&self, download_id: i64, tags: &[PortableTag]) -> Result<(), String> {
        // Archive restore already owns the database-wide restore transaction. Starting a nested
        // rusqlite transaction here would fail, so every statement joins that outer transaction.
        let conn = self.conn.lock().map_err(|error| error.to_string())?;
        conn.execute(
            "DELETE FROM download_tags WHERE download_id = ?1",
            params![download_id],
        )
        .map_err(|error| format!("Failed to clear restored tags: {error}"))?;
        for tag in tags {
            let name = tag.name.trim();
            if name.is_empty() || name.chars().count() > 100 {
                return Err("Backup contains an invalid tag name".to_string());
            }
            if !valid_tag_source(&tag.source) {
                return Err(format!(
                    "Backup contains an invalid tag source: {}",
                    tag.source
                ));
            }
            conn.execute(
                "INSERT OR IGNORE INTO tags (name) VALUES (?1)",
                params![name],
            )
            .map_err(|error| format!("Failed to restore tag: {error}"))?;
            conn.execute(
                "INSERT INTO download_tags (download_id, tag_id, tag_source)
                 SELECT ?1, id, ?3 FROM tags WHERE name = ?2",
                params![download_id, name, tag.source],
            )
            .map_err(|error| format!("Failed to restore tag source: {error}"))?;
        }
        Ok(())
    }

    pub fn archive_work_edits(&self, download_id: i64) -> Result<Vec<PortableWorkEdit>, String> {
        let conn = self.read_conn()?;
        let mut revisions = conn
            .prepare(
                "SELECT id, base_version, status, title, content_hash, created_at, updated_at
                 FROM work_edit_revisions WHERE download_id = ?1 ORDER BY id",
            )
            .map_err(|error| format!("Failed to prepare work edits for backup: {error}"))?;
        let rows = revisions
            .query_map(params![download_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    PortableWorkEdit {
                        base_version: row.get(1)?,
                        status: row.get(2)?,
                        title: row.get(3)?,
                        content_hash: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                        blocks: Vec::new(),
                    },
                ))
            })
            .map_err(|error| format!("Failed to query work edits for backup: {error}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to read work edits for backup: {error}"))?;
        let mut output = Vec::with_capacity(rows.len());
        for (revision_id, mut revision) in rows {
            let mut blocks = conn
                .prepare(
                    "SELECT b.block_order, b.block_type, b.text, a.local_path, b.attrs_json
                     FROM work_edit_blocks b
                     LEFT JOIN assets a ON a.id = b.asset_id
                     WHERE b.edit_revision_id = ?1 ORDER BY b.block_order",
                )
                .map_err(|error| format!("Failed to prepare edit blocks for backup: {error}"))?;
            revision.blocks = blocks
                .query_map(params![revision_id], |row| {
                    Ok(PortableWorkEditBlock {
                        order: row.get(0)?,
                        block_type: row.get(1)?,
                        text: row.get(2)?,
                        asset_path: row.get(3)?,
                        attrs_json: row.get(4)?,
                    })
                })
                .map_err(|error| format!("Failed to query edit blocks for backup: {error}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| format!("Failed to read edit blocks for backup: {error}"))?;
            output.push(revision);
        }
        Ok(output)
    }

    pub fn restore_work_edits(
        &self,
        download_id: i64,
        revisions: &[PortableWorkEdit],
    ) -> Result<(), String> {
        // This participates in archive.rs' outer atomic restore transaction.
        let conn = self.conn.lock().map_err(|error| error.to_string())?;
        for revision in revisions {
            if !valid_edit_status(&revision.status) {
                return Err(format!(
                    "Backup contains an invalid edit status: {}",
                    revision.status
                ));
            }
            conn.execute(
                "INSERT INTO work_edit_revisions
                 (download_id, base_version, status, title, content_hash, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    download_id,
                    revision.base_version,
                    revision.status,
                    revision.title,
                    revision.content_hash,
                    revision.created_at,
                    revision.updated_at
                ],
            )
            .map_err(|error| format!("Failed to restore work edit: {error}"))?;
            let revision_id = conn.last_insert_rowid();
            for block in &revision.blocks {
                let asset_id = block
                    .asset_path
                    .as_deref()
                    .map(|path| {
                        conn.query_row(
                            "SELECT id FROM assets WHERE download_id = ?1 AND local_path = ?2",
                            params![download_id, path],
                            |row| row.get::<_, i64>(0),
                        )
                        .optional()
                        .map_err(|error| format!("Failed to resolve restored edit asset: {error}"))
                    })
                    .transpose()?
                    .flatten();
                conn.execute(
                    "INSERT INTO work_edit_blocks
                     (edit_revision_id, block_order, block_type, text, asset_id, attrs_json)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        revision_id,
                        block.order,
                        block.block_type,
                        block.text,
                        asset_id,
                        block.attrs_json
                    ],
                )
                .map_err(|error| format!("Failed to restore edit block: {error}"))?;
            }
        }
        Ok(())
    }

    pub fn restore_saved_search(&self, search: &SavedSearch) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|error| error.to_string())?;
        conn.execute(
            "INSERT INTO saved_searches (name, query, params_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(name) DO UPDATE SET query = excluded.query,
                 params_json = excluded.params_json, updated_at = excluded.updated_at",
            params![
                search.name,
                search.query,
                search.params_json,
                search.created_at,
                search.updated_at
            ],
        )
        .map_err(|error| format!("Failed to restore saved search: {error}"))?;
        Ok(())
    }

    pub fn archive_collection_pair_feedback(
        &self,
    ) -> Result<Vec<PortableCollectionPairFeedback>, String> {
        let conn = self.read_conn()?;
        let mut statement = conn
            .prepare(
                "SELECT left_source, left_source_id, right_source, right_source_id,
                        decision, rule_version, updated_at
                 FROM collection_pair_feedback
                 ORDER BY left_source, left_source_id, right_source, right_source_id",
            )
            .map_err(|error| format!("Failed to prepare collection feedback backup: {error}"))?;
        let rows = statement
            .query_map([], |row| {
                Ok(PortableCollectionPairFeedback {
                    left_source: row.get(0)?,
                    left_source_id: row.get(1)?,
                    right_source: row.get(2)?,
                    right_source_id: row.get(3)?,
                    decision: row.get(4)?,
                    rule_version: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            })
            .map_err(|error| format!("Failed to query collection feedback backup: {error}"))?;
        let collected = rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("Failed to read collection feedback backup: {error}"))?;
        Ok(collected)
    }

    pub fn restore_collection_pair_feedback(
        &self,
        feedback: &PortableCollectionPairFeedback,
    ) -> Result<(), String> {
        if !valid_pair_decision(&feedback.decision) {
            return Err(format!(
                "Backup contains invalid collection feedback: {}",
                feedback.decision
            ));
        }
        let conn = self.conn.lock().map_err(|error| error.to_string())?;
        conn.execute(
            "INSERT INTO collection_pair_feedback
             (left_source, left_source_id, right_source, right_source_id, decision, rule_version, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(left_source, left_source_id, right_source, right_source_id)
             DO UPDATE SET decision = excluded.decision, rule_version = excluded.rule_version,
                           updated_at = excluded.updated_at",
            params![
                feedback.left_source,
                feedback.left_source_id,
                feedback.right_source,
                feedback.right_source_id,
                feedback.decision,
                feedback.rule_version,
                feedback.updated_at
            ],
        )
        .map_err(|error| format!("Failed to restore collection feedback: {error}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{valid_edit_status, valid_pair_decision, valid_tag_source};

    #[test]
    fn portable_enums_reject_values_that_cannot_be_restored_safely() {
        assert!(["origin", "manual", "llm"]
            .into_iter()
            .all(valid_tag_source));
        assert!(["draft", "active", "archived"]
            .into_iter()
            .all(valid_edit_status));
        assert!(["accept", "reject"].into_iter().all(valid_pair_decision));
        assert!(!valid_tag_source("model"));
        assert!(!valid_edit_status("published"));
        assert!(!valid_pair_decision("maybe"));
    }
}
