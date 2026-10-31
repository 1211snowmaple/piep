//! 監視グループの問い合わせ。
//!
//! 監視グループは、利用者が監視対象（作者・シリーズ）を束ねて名前を付けたもの。
//! 「毎日見る作者」のように束ねておけば、その束だけを確認したり、自動確認の
//! 範囲にしたりできる。**一つの対象がいくつのグループに入ってもよい**（タグと
//! 同じ）。グループを消しても、中の監視対象は消えない。
//!
//! 既定の束（すべて・作者・シリーズ・停止中）は監視対象そのものの性質なので、
//! ここには置かない。ここにあるのは利用者が作った束だけ。

use super::Database;
use crate::database::WatchGroup;
use rusqlite::{params, OptionalExtension};

/// グループ名の長さの上限。一覧の一行に収まり、題として読める長さ。
const MAX_WATCH_GROUP_NAME_CHARS: usize = 40;

/// 前後の空白を落とし、空と長すぎる名前を断る。
fn normalize_group_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("グループ名を入力してください".into());
    }
    if name.chars().count() > MAX_WATCH_GROUP_NAME_CHARS {
        return Err(format!(
            "グループ名は{MAX_WATCH_GROUP_NAME_CHARS}文字までにしてください"
        ));
    }
    Ok(name.to_string())
}

fn unique_name_error(error: rusqlite::Error, name: &str) -> String {
    if matches!(
        error.sqlite_error_code(),
        Some(rusqlite::ErrorCode::ConstraintViolation)
    ) {
        format!("「{name}」という名前のグループはもうあります")
    } else {
        error.to_string()
    }
}

impl Database {
    /// 利用者が作ったグループを、並び順で返す。中の監視対象の ID も添える。
    pub fn list_watch_groups(&self) -> Result<Vec<WatchGroup>, String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT g.id, g.name, g.sort_order, g.created_at, g.updated_at,
                        (SELECT json_group_array(m.target_id) FROM watch_group_members m WHERE m.group_id = g.id)
                   FROM watch_groups g
                  ORDER BY g.sort_order ASC, g.id ASC",
            )
            .map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                let members: String = row.get(5)?;
                Ok(WatchGroup {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    sort_order: row.get(2)?,
                    created_at: row.get(3)?,
                    updated_at: row.get(4)?,
                    target_ids: serde_json::from_str(&members).unwrap_or_default(),
                })
            })
            .map_err(|error| error.to_string())?;
        rows.collect::<Result<_, _>>()
            .map_err(|error| error.to_string())
    }

    /// 新しいグループを一番下に作り、その ID を返す。
    pub fn create_watch_group(&self, name: &str) -> Result<i64, String> {
        let name = normalize_group_name(name)?;
        let conn = self.conn.lock().map_err(|error| error.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO watch_groups(name, sort_order, created_at, updated_at)
             VALUES (?1, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM watch_groups), ?2, ?2)",
            params![name, now],
        )
        .map_err(|error| unique_name_error(error, &name))?;
        Ok(conn.last_insert_rowid())
    }

    pub fn rename_watch_group(&self, group_id: i64, name: &str) -> Result<(), String> {
        let name = normalize_group_name(name)?;
        let conn = self.conn.lock().map_err(|error| error.to_string())?;
        let changed = conn
            .execute(
                "UPDATE watch_groups SET name = ?2, updated_at = ?3 WHERE id = ?1",
                params![group_id, name, chrono::Utc::now().to_rfc3339()],
            )
            .map_err(|error| unique_name_error(error, &name))?;
        if changed == 0 {
            return Err("グループが見つかりません".into());
        }
        Ok(())
    }

    /// グループだけを消す。中の監視対象はそのまま残る。
    pub fn delete_watch_group(&self, group_id: i64) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|error| error.to_string())?;
        conn.execute("DELETE FROM watch_groups WHERE id = ?1", params![group_id])
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// 監視対象をグループに入れる（`add`）か、外す。入っているものを入れても、
    /// 入っていないものを外しても何も起きない。変わった件数を返す。
    pub fn set_watch_group_members(
        &self,
        group_id: i64,
        target_ids: &[i64],
        add: bool,
    ) -> Result<usize, String> {
        let mut conn = self.conn.lock().map_err(|error| error.to_string())?;
        let tx = conn.transaction().map_err(|error| error.to_string())?;
        let exists = tx
            .query_row(
                "SELECT 1 FROM watch_groups WHERE id = ?1",
                params![group_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if exists.is_none() {
            return Err("グループが見つかりません".into());
        }
        let mut changed = 0;
        for target_id in target_ids {
            changed += if add {
                tx.execute(
                    "INSERT OR IGNORE INTO watch_group_members(group_id, target_id)
                     SELECT ?1, id FROM update_targets WHERE id = ?2",
                    params![group_id, target_id],
                )
            } else {
                tx.execute(
                    "DELETE FROM watch_group_members WHERE group_id = ?1 AND target_id = ?2",
                    params![group_id, target_id],
                )
            }
            .map_err(|error| error.to_string())?;
        }
        tx.execute(
            "UPDATE watch_groups SET updated_at = ?2 WHERE id = ?1",
            params![group_id, chrono::Utc::now().to_rfc3339()],
        )
        .map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
        Ok(changed)
    }

    /// バックアップから戻した監視対象を、名前で覚えていたグループへ入れ直す。
    /// 同じ名前のグループが無ければ作る。
    ///
    /// 戻しは外側の取引の中で走るので、ここでは取引を開かない。
    pub(crate) fn restore_watch_group_memberships(
        &self,
        target_type: &str,
        source: &str,
        source_key: &str,
        group_names: &[String],
    ) -> Result<(), String> {
        if group_names.is_empty() {
            return Ok(());
        }
        let conn = self.conn.lock().map_err(|error| error.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        for name in group_names {
            let Ok(name) = normalize_group_name(name) else {
                continue;
            };
            conn.execute(
                "INSERT INTO watch_groups(name, sort_order, created_at, updated_at)
                 VALUES (?1, (SELECT COALESCE(MAX(sort_order), -1) + 1 FROM watch_groups), ?2, ?2)
                 ON CONFLICT(name) DO NOTHING",
                params![name, now],
            )
            .map_err(|error| error.to_string())?;
            conn.execute(
                "INSERT OR IGNORE INTO watch_group_members(group_id, target_id)
                 SELECT g.id, t.id FROM watch_groups g, update_targets t
                  WHERE g.name = ?1 AND t.target_type = ?2 AND t.source = ?3 AND t.source_key = ?4",
                params![name, target_type, source, source_key],
            )
            .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}
