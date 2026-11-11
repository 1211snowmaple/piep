use super::{Error, Library, LibraryId, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

const MAX_SEARCHES: i64 = 100;

/// 条件の JSON 字句と未知の項目を保つ。検索画面の解釈規則は別に版管理する。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedSearchDefinition {
    pub name: String,
    pub query: Option<String>,
    pub params_json: String,
}

impl SavedSearchDefinition {
    pub(super) fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty()
            || self.name.chars().count() > 80
            || self.query.as_ref().is_some_and(|s| s.chars().count() > 512)
            || self.params_json.len() > 16 * 1024
        {
            return Err(Error::Integrity(
                "保存する検索の名前・条件が不正です".into(),
            ));
        }
        // Value に直して保存し直すと数値や未知の項目の字句が変わるため、検査だけ行う。
        let json: serde_json::Value = serde_json::from_str(&self.params_json)?;
        if !json.is_object() {
            return Err(Error::Integrity(
                "検索条件がオブジェクトではありません".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedSearch {
    pub id: LibraryId,
    pub user: LibraryId,
    pub definition: SavedSearchDefinition,
    pub definition_version: i64,
    pub position: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub generation: i64,
    pub deleted_at: Option<i64>,
}

pub struct SavedSearchUpdate {
    pub request_id: LibraryId,
    pub id: LibraryId,
    pub user: LibraryId,
    pub expected_generation: i64,
    /// None は削除。内容を消さず、削除の世代を残す。
    pub definition: Option<SavedSearchDefinition>,
}

fn sql_id(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<LibraryId> {
    let bytes: Vec<u8> = row.get(column)?;
    LibraryId::from_slice(&bytes).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Blob, Box::new(e))
    })
}
fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<SavedSearch> {
    Ok(SavedSearch {
        id: sql_id(row, 0)?,
        user: sql_id(row, 1)?,
        definition: SavedSearchDefinition {
            name: row.get(2)?,
            query: row.get(3)?,
            params_json: row.get(4)?,
        },
        definition_version: row.get(5)?,
        position: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        generation: row.get(9)?,
        deleted_at: row.get(10)?,
    })
}
const COLUMNS: &str = "id,user_id,name,query,params_json,definition_version,position,created_at,updated_at,generation,deleted_at";

fn load(conn: &Connection, user: LibraryId, id: LibraryId) -> Result<Option<SavedSearch>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM saved_searches WHERE id=?1 AND user_id=?2"),
            params![id.as_bytes().as_slice(), user.as_bytes().as_slice()],
            decode,
        )
        .optional()?)
}

pub(super) fn verify_searches(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM saved_searches"))?;
    for search in stmt.query_map([], decode)? {
        let search = search?;
        search.definition.validate()?;
        if search.definition_version != 1
            || [
                Some(search.created_at),
                Some(search.updated_at),
                search.deleted_at,
            ]
            .into_iter()
            .flatten()
            .any(|date| chrono::DateTime::from_timestamp_millis(date).is_none())
        {
            return Err(Error::Integrity(
                "保存した検索の形式・日時が不正です".into(),
            ));
        }
    }
    Ok(())
}

impl Library {
    pub fn saved_search(&self, user: LibraryId, id: LibraryId) -> Result<SavedSearch> {
        let conn = self.reader()?;
        load(&conn, user, id)?.ok_or_else(|| Error::Integrity("保存した検索がありません".into()))
    }

    pub fn saved_searches(&self, user: LibraryId) -> Result<Vec<SavedSearch>> {
        let conn = self.reader()?;
        let mut stmt=conn.prepare(&format!("SELECT {COLUMNS} FROM saved_searches WHERE user_id=?1 AND deleted_at IS NULL ORDER BY position,id"))?;
        let rows = stmt.query_map([user.as_bytes().as_slice()], decode)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn update_saved_search(&self, request: SavedSearchUpdate) -> Result<SavedSearch> {
        if request.expected_generation < 0 {
            return Err(Error::Integrity("検索の更新世代が不正です".into()));
        }
        if let Some(definition) = &request.definition {
            definition.validate()?;
        }
        let payload = serde_json::json!({"id":request.id,"user":request.user,"expected":request.expected_generation,"definition":request.definition});
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let receipt: Option<(String, String)> = tx
            .query_row(
                "SELECT operation,payload_json FROM outbox WHERE request_id=?1",
                [request.request_id.as_bytes().as_slice()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((operation, receipt)) = receipt {
            let receipt: serde_json::Value = serde_json::from_str(&receipt)?;
            if operation != "saved_search" || receipt.get("request") != Some(&payload) {
                return Err(Error::Integrity(
                    "同じ要求 ID が異なる操作に使われています".into(),
                ));
            }
            return Ok(serde_json::from_value(receipt["result"].clone())?);
        }
        let old = load(&tx, request.user, request.id)?;
        if old.as_ref().map_or(0, |s| s.generation) != request.expected_generation {
            return Err(Error::Integrity(
                "検索の編集中に別の変更が保存されました".into(),
            ));
        }
        if request.definition.is_none() && old.is_none() {
            return Err(Error::Integrity("削除する検索がありません".into()));
        }
        let now = chrono::Utc::now().timestamp_millis();
        let generation = request
            .expected_generation
            .checked_add(1)
            .ok_or_else(|| Error::Integrity("検索の更新世代の上限を超えました".into()))?;
        if request.definition.is_some() && old.as_ref().is_none_or(|s| s.deleted_at.is_some()) {
            let count: i64 = tx.query_row(
                "SELECT count(*) FROM saved_searches WHERE user_id=?1 AND deleted_at IS NULL",
                [request.user.as_bytes().as_slice()],
                |r| r.get(0),
            )?;
            if count >= MAX_SEARCHES {
                return Err(Error::Integrity(
                    "保存できる検索の上限に達しています".into(),
                ));
            }
        }
        let deleted = request.definition.is_none();
        let definition = request
            .definition
            .or_else(|| old.as_ref().map(|s| s.definition.clone()))
            .unwrap();
        let position = match &old {
            Some(s) => s.position,
            None => tx.query_row(
                "SELECT coalesce(max(position)+1,0) FROM saved_searches WHERE user_id=?1",
                [request.user.as_bytes().as_slice()],
                |r| r.get(0),
            )?,
        };
        tx.execute("INSERT INTO saved_searches(id,user_id,name,query,params_json,definition_version,position,created_at,updated_at,generation,deleted_at) VALUES(?1,?2,?3,?4,?5,1,?6,?7,?8,?9,?10) ON CONFLICT(id) DO UPDATE SET name=excluded.name,query=excluded.query,params_json=excluded.params_json,updated_at=excluded.updated_at,generation=excluded.generation,deleted_at=excluded.deleted_at WHERE saved_searches.user_id=excluded.user_id",params![request.id.as_bytes().as_slice(),request.user.as_bytes().as_slice(),definition.name,definition.query,definition.params_json,position,old.as_ref().map_or(now,|s|s.created_at),now,generation,deleted.then_some(now)])?;
        // 別の所有者の ID は上書きせず、トランザクション全体を戻す。
        let result = load(&tx, request.user, request.id)?
            .ok_or_else(|| Error::Integrity("保存した検索の所属が一致しません".into()))?;
        tx.execute("INSERT INTO outbox(request_id,saved_search_id,user_id,operation,payload_json,created_at) VALUES(?1,?2,?3,'saved_search',?4,?5)",params![request.request_id.as_bytes().as_slice(),request.id.as_bytes().as_slice(),request.user.as_bytes().as_slice(),serde_json::json!({"request":payload,"result":result}).to_string(),now])?;
        tx.execute(
            "UPDATE library_meta SET generation=generation+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(result)
    }
}

#[cfg(test)]
#[path = "searches_tests.rs"]
mod tests;
