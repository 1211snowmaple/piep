//! 作者・シリーズの監視と利用者のグループ。操作は要求 ID とカタログの世代で確定する。

use super::{EntityKind, Error, Library, LibraryId, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchTarget {
    pub id: LibraryId,
    pub kind: String,
    pub provider: String,
    pub key: String,
    pub name: String,
    pub enabled: bool,
    pub checked_at: Option<i64>,
    pub seen_key: Option<String>,
    pub seen_updated_at: Option<i64>,
    pub hit_at: Option<i64>,
    pub errors: i64,
    pub options: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub groups: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchGroup {
    pub id: LibraryId,
    pub name: String,
    pub position: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub targets: Vec<LibraryId>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WatchEdit {
    Upsert {
        target_type: String,
        provider: String,
        key: String,
        name: String,
        enabled: bool,
        options: Value,
    },
    Enable {
        target: LibraryId,
        enabled: bool,
    },
    RemoveTarget {
        target: LibraryId,
    },
    CreateGroup {
        name: String,
    },
    RenameGroup {
        group: LibraryId,
        name: String,
    },
    RemoveGroup {
        group: LibraryId,
    },
    Members {
        group: LibraryId,
        targets: Vec<LibraryId>,
        add: bool,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatchResult {
    pub id: LibraryId,
    pub changed: usize,
    pub generation: i64,
}

fn id(bytes: Vec<u8>) -> Result<LibraryId> {
    LibraryId::from_slice(&bytes).map_err(|_| Error::Integrity("監視の識別子が不正です".into()))
}

fn group_name(value: &str) -> Result<&str> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 40 {
        return Err(Error::Integrity(
            "グループ名は1〜40文字で入力してください".into(),
        ));
    }
    Ok(value)
}

fn unique_group_name(conn: &Connection, name: &str, except: Option<LibraryId>) -> Result<()> {
    let duplicate: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM watch_groups WHERE name=?1 AND (?2 IS NULL OR id<>?2))",
        params![name, except.map(|id| id.as_bytes().to_vec())],
        |r| r.get(0),
    )?;
    if duplicate {
        return Err(Error::Integrity("同じ名前の監視グループがあります".into()));
    }
    Ok(())
}

fn present(conn: &Connection, table: &str, key: LibraryId) -> Result<()> {
    let found: bool = conn.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id=?1)"),
        [key.as_bytes().as_slice()],
        |r| r.get(0),
    )?;
    if !found {
        return Err(Error::Integrity(
            "監視対象またはグループが見つかりません".into(),
        ));
    }
    Ok(())
}

impl Library {
    pub fn watch_request_expected(&self, request: LibraryId) -> Result<Option<i64>> {
        let value: Option<String> = self
            .reader()?
            .query_row(
                "SELECT payload_json FROM outbox WHERE request_id=?1 AND operation='watch'",
                [request.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        value
            .map(|value| Ok(serde_json::from_str::<Value>(&value)?["request"]["expected"].as_i64()))
            .transpose()
            .map(Option::flatten)
    }
    pub fn watch_generation(&self) -> Result<i64> {
        Ok(self.reader()?.query_row(
            "SELECT watch_generation FROM library_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )?)
    }

    pub fn watch_targets(&self) -> Result<Vec<WatchTarget>> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare("SELECT t.id,e.kind,x.provider,x.external_key,t.display_name,t.enabled,t.last_checked_at,
            t.last_seen_external_key,t.last_seen_source_updated_at,t.last_hit_at,t.consecutive_errors,t.options_json,t.created_at,t.updated_at
            FROM watch_targets t JOIN entities e ON e.id=t.entity_id
            JOIN external_identities x ON x.entity_id=e.id AND x.kind=e.kind
            WHERE e.deleted_at IS NULL ORDER BY e.kind,x.provider,t.display_name COLLATE NOCASE,t.id")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get::<_, String>(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
                r.get(10)?,
                r.get(11)?,
                r.get(12)?,
                r.get(13)?,
            ))
        })?;
        let mut result = Vec::new();
        for row in rows {
            let (
                key,
                kind,
                provider,
                external,
                name,
                enabled,
                checked_at,
                seen_key,
                seen_updated_at,
                hit_at,
                errors,
                options,
                created_at,
                updated_at,
            ) = row?;
            let key = id(key)?;
            let groups=conn.prepare("SELECT g.name FROM watch_group_members m JOIN watch_groups g ON g.id=m.group_id WHERE m.target_id=?1 ORDER BY g.sort_order,g.id")?
                .query_map([key.as_bytes().as_slice()],|r|r.get(0))?.collect::<std::result::Result<_,_>>()?;
            result.push(WatchTarget {
                id: key,
                kind: if kind == "person" {
                    "author".into()
                } else {
                    kind
                },
                provider,
                key: external,
                name,
                enabled,
                checked_at,
                seen_key,
                seen_updated_at,
                hit_at,
                errors,
                options,
                created_at,
                updated_at,
                groups,
            });
        }
        Ok(result)
    }

    pub fn watch_groups(&self) -> Result<Vec<WatchGroup>> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "SELECT g.id,g.name,g.sort_order,g.created_at,g.updated_at
            FROM watch_groups g ORDER BY g.sort_order,g.id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
            ))
        })?;
        let mut result = Vec::new();
        for row in rows {
            let (key, name, position, created_at, updated_at) = row?;
            let key = id(key)?;
            let targets=conn.prepare("SELECT target_id FROM watch_group_members WHERE group_id=?1 ORDER BY target_id")?
                .query_map([key.as_bytes().as_slice()],|r|r.get::<_,Vec<u8>>(0))?.map(|v|id(v?)).collect::<Result<_>>()?;
            result.push(WatchGroup {
                id: key,
                name,
                position,
                created_at,
                updated_at,
                targets,
            });
        }
        Ok(result)
    }

    pub fn edit_watch(
        &self,
        request: LibraryId,
        expected: i64,
        edit: WatchEdit,
    ) -> Result<WatchResult> {
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let input = json!({"expected":expected,"edit":edit});
        let receipt: Option<(String, String)> = tx
            .query_row(
                "SELECT operation,payload_json FROM outbox WHERE request_id=?1",
                [request.as_bytes().as_slice()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((operation, receipt)) = receipt {
            let receipt: Value = serde_json::from_str(&receipt)?;
            if operation != "watch" || receipt["request"] != input {
                return Err(Error::Integrity(
                    "要求 ID が別の監視の操作に使われています".into(),
                ));
            }
            return Ok(serde_json::from_value(receipt["result"].clone())?);
        }
        let generation: i64 = tx.query_row(
            "SELECT watch_generation FROM library_meta WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        if generation != expected {
            return Err(Error::Integrity(
                "監視の設定が先に変更されました。読み直してください".into(),
            ));
        }
        let now = chrono::Utc::now().timestamp_millis();
        let (key, changed) = match &edit {
            WatchEdit::Upsert {
                target_type,
                provider,
                key,
                name,
                enabled,
                options,
            } => {
                let kind = match target_type.as_str() {
                    "author" => EntityKind::Person,
                    "series" => EntityKind::Series,
                    _ => {
                        return Err(Error::Integrity(
                            "監視は作者・シリーズを指定してください".into(),
                        ))
                    }
                };
                if !matches!(provider.as_str(), "pixiv" | "fanbox")
                    || key.trim().is_empty()
                    || name.trim().is_empty()
                {
                    return Err(Error::Integrity(
                        "取得元・ID・名前を指定してください".into(),
                    ));
                }
                let entity = Self::relation_entity(&tx, provider, kind, key, now)?;
                let existing: Option<Vec<u8>> = tx
                    .query_row(
                        "SELECT id FROM watch_targets WHERE entity_id=?1",
                        [entity.as_bytes().as_slice()],
                        |r| r.get(0),
                    )
                    .optional()?;
                let key = existing.map(id).transpose()?.unwrap_or(request);
                let changed=tx.execute("INSERT INTO watch_targets(id,entity_id,display_name,enabled,options_json,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?6)
                    ON CONFLICT(entity_id) DO UPDATE SET display_name=excluded.display_name,enabled=excluded.enabled,options_json=excluded.options_json,updated_at=excluded.updated_at
                    WHERE display_name IS NOT excluded.display_name OR enabled IS NOT excluded.enabled OR options_json IS NOT excluded.options_json",
                    params![key.as_bytes().as_slice(),entity.as_bytes().as_slice(),name.trim(),enabled,options.to_string(),now])?;
                (key, changed)
            }
            WatchEdit::Enable { target, enabled } => {
                present(&tx, "watch_targets", *target)?;
                (*target,tx.execute("UPDATE watch_targets SET enabled=?2,updated_at=?3 WHERE id=?1 AND enabled IS NOT ?2",params![target.as_bytes().as_slice(),enabled,now])?)
            }
            WatchEdit::RemoveTarget { target } => {
                present(&tx, "watch_targets", *target)?;
                tx.execute(
                    "DELETE FROM watch_group_members WHERE target_id=?1",
                    [target.as_bytes().as_slice()],
                )?;
                (
                    *target,
                    tx.execute(
                        "DELETE FROM watch_targets WHERE id=?1",
                        [target.as_bytes().as_slice()],
                    )?,
                )
            }
            WatchEdit::CreateGroup { name } => {
                let name = group_name(name)?;
                unique_group_name(&tx, name, None)?;
                let position: i64 = tx.query_row(
                    "SELECT coalesce(max(sort_order),-1)+1 FROM watch_groups",
                    [],
                    |r| r.get(0),
                )?;
                (request,tx.execute("INSERT INTO watch_groups(id,name,sort_order,created_at,updated_at) VALUES(?1,?2,?3,?4,?4)",params![request.as_bytes().as_slice(),name,position,now])?)
            }
            WatchEdit::RenameGroup { group, name } => {
                present(&tx, "watch_groups", *group)?;
                unique_group_name(&tx, group_name(name)?, Some(*group))?;
                (*group,tx.execute("UPDATE watch_groups SET name=?2,updated_at=?3 WHERE id=?1 AND name IS NOT ?2",params![group.as_bytes().as_slice(),group_name(name)?,now])?)
            }
            WatchEdit::RemoveGroup { group } => {
                present(&tx, "watch_groups", *group)?;
                tx.execute(
                    "DELETE FROM watch_group_members WHERE group_id=?1",
                    [group.as_bytes().as_slice()],
                )?;
                (
                    *group,
                    tx.execute(
                        "DELETE FROM watch_groups WHERE id=?1",
                        [group.as_bytes().as_slice()],
                    )?,
                )
            }
            WatchEdit::Members {
                group,
                targets,
                add,
            } => {
                present(&tx, "watch_groups", *group)?;
                for target in targets {
                    present(&tx, "watch_targets", *target)?;
                }
                let mut changed = 0;
                for target in targets {
                    changed += if *add {
                        tx.execute("INSERT OR IGNORE INTO watch_group_members(group_id,target_id) VALUES(?1,?2)",params![group.as_bytes().as_slice(),target.as_bytes().as_slice()])?
                    } else {
                        tx.execute(
                            "DELETE FROM watch_group_members WHERE group_id=?1 AND target_id=?2",
                            params![group.as_bytes().as_slice(), target.as_bytes().as_slice()],
                        )?
                    };
                }
                if changed > 0 {
                    tx.execute(
                        "UPDATE watch_groups SET updated_at=?2 WHERE id=?1",
                        params![group.as_bytes().as_slice(), now],
                    )?;
                }
                (*group, changed)
            }
        };
        let result = WatchResult {
            id: key,
            changed,
            generation: generation + 1,
        };
        tx.execute("UPDATE library_meta SET watch_generation=watch_generation+1,generation=generation+1 WHERE singleton=1",[])?;
        tx.execute("INSERT INTO outbox(request_id,watch_catalog_id,operation,payload_json,created_at) VALUES(?1,1,'watch',?2,?3)",params![request.as_bytes().as_slice(),json!({"request":input,"result":result}).to_string(),now])?;
        tx.commit()?;
        Ok(result)
    }

    /// ジョブの結果は設定を書き換えず、実行を始めた対象 ID にだけ記録する。削除・作り直し後の対象へは書かない。
    pub fn record_watch_check(
        &self,
        target: LibraryId,
        seen: Option<&str>,
        updated: Option<i64>,
        found: i64,
        failed: bool,
    ) -> Result<()> {
        if updated.is_some_and(|v| chrono::DateTime::from_timestamp_millis(v).is_none()) {
            return Err(Error::Integrity("更新日時が不正です".into()));
        }
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let now = chrono::Utc::now().timestamp_millis();
        conn.execute("UPDATE watch_targets SET last_checked_at=?2,
            last_seen_external_key=CASE WHEN ?6 THEN last_seen_external_key ELSE coalesce(?3,last_seen_external_key) END,
            last_seen_source_updated_at=CASE WHEN ?6 THEN last_seen_source_updated_at ELSE coalesce(?4,last_seen_source_updated_at) END,
            last_hit_at=CASE WHEN NOT ?6 AND ?5>0 THEN ?2 ELSE last_hit_at END,
            consecutive_errors=CASE WHEN ?6 THEN consecutive_errors+1 ELSE 0 END,updated_at=?2 WHERE id=?1",
            params![target.as_bytes().as_slice(),now,seen,updated,found,failed])?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "watching_tests.rs"]
mod tests;
