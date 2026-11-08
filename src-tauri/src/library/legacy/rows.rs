use super::LegacyImporter;
use crate::library::{Error, LibraryId, Result};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) struct LegacyRow(pub BTreeMap<String, Value>);
impl LegacyRow {
    pub fn text(&self, key: &str) -> Result<&str> {
        self.0
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Integrity(format!("旧記録の {key} が文字列ではありません")))
    }
    pub fn optional_text(&self, key: &str) -> Result<Option<&str>> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            _ => Err(Error::Integrity(format!(
                "旧記録の {key} が文字列ではありません"
            ))),
        }
    }
    pub fn integer(&self, key: &str) -> Result<i64> {
        self.0
            .get(key)
            .and_then(Value::as_i64)
            .ok_or_else(|| Error::Integrity(format!("旧記録の {key} が整数ではありません")))
    }
    pub fn optional_integer(&self, key: &str) -> Result<Option<i64>> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value
                .as_i64()
                .map(Some)
                .ok_or_else(|| Error::Integrity(format!("旧記録の {key} が整数ではありません"))),
        }
    }
    pub fn boolean(&self, key: &str) -> Result<bool> {
        match self.integer(key)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::Integrity(format!(
                "旧記録の {key} が真偽値ではありません"
            ))),
        }
    }
    pub fn optional_time(&self, key: &str) -> Result<Option<i64>> {
        let Some(value) = self.optional_text(key)?.filter(|s| !s.is_empty()) else {
            return Ok(None);
        };
        parse_time(value).map(Some)
    }
    pub fn time(&self, key: &str) -> Result<i64> {
        self.optional_time(key)?
            .ok_or_else(|| Error::Integrity(format!("旧記録の {key} の日時がありません")))
    }
}

impl LegacyImporter {
    pub(super) fn has_column(&self, table: &str, column: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name=?2)",
            params![table, column],
            |r| r.get(0),
        )?)
    }
    pub(super) fn each_row(
        &self,
        table: &str,
        callback: impl FnMut(LegacyRow) -> Result<()>,
    ) -> Result<usize> {
        self.read_rows(table, None, callback)
    }

    pub(super) fn each_row_matching(
        &self,
        table: &str,
        column: &str,
        value: i64,
        callback: impl FnMut(LegacyRow) -> Result<()>,
    ) -> Result<usize> {
        if column.is_empty()
            || !column
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(Error::Unsupported("旧列名が不正です".into()));
        }
        self.read_rows(table, Some((column, value)), callback)
    }

    fn read_rows(
        &self,
        table: &str,
        filter: Option<(&str, i64)>,
        mut callback: impl FnMut(LegacyRow) -> Result<()>,
    ) -> Result<usize> {
        if table.is_empty()
            || !table
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(Error::Unsupported("旧テーブル名が不正です".into()));
        }
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [table],
            |r| r.get(0),
        )?;
        if !exists {
            return Ok(0);
        }
        let query = match filter {
            Some((column, _)) => format!("SELECT * FROM \"{table}\" WHERE \"{column}\"=?1"),
            None => format!("SELECT * FROM \"{table}\""),
        };
        let mut statement = self.conn.prepare(&query)?;
        let names = statement
            .column_names()
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let mut rows = match filter {
            Some((_, value)) => statement.query([value])?,
            None => statement.query([])?,
        };
        let mut count = 0;
        while let Some(row) = rows.next()? {
            let mut values = BTreeMap::new();
            for (index, name) in names.iter().enumerate() {
                use rusqlite::types::ValueRef;
                let value = match row.get_ref(index)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(v) => Value::from(v),
                    ValueRef::Real(v) if v.is_finite() => Value::from(v),
                    ValueRef::Text(v) if v.len() <= 16 * 1024 * 1024 => Value::String(
                        std::str::from_utf8(v)
                            .map_err(|_| {
                                Error::Integrity(format!("{table}.{name}: UTF-8 ではない記録です"))
                            })?
                            .into(),
                    ),
                    _ => {
                        return Err(Error::Integrity(format!(
                            "{table}.{name}: 正規化できない型・大きさです。原記録は保持しています"
                        )))
                    }
                };
                values.insert(name.clone(), value);
            }
            callback(LegacyRow(values))?;
            count += 1;
        }
        Ok(count)
    }
}

pub(super) fn parse_time(value: &str) -> Result<i64> {
    if let Ok(date) = chrono::DateTime::parse_from_rfc3339(value) {
        return Ok(date.timestamp_millis());
    }
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"] {
        if let Ok(date) = chrono::NaiveDateTime::parse_from_str(value, format) {
            return Ok(date.and_utc().timestamp_millis());
        }
    }
    Err(Error::Integrity("旧記録の日時を解釈できません".into()))
}

pub(super) fn mapped_id(tx: &Transaction<'_>, table: &str, key: &str) -> Result<LibraryId> {
    let existing: Option<Vec<u8>> = tx
        .query_row(
            "SELECT target_id FROM migration_records WHERE source_table=?1 AND source_key=?2",
            params![table, key],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    match existing {
        Some(id) => LibraryId::from_slice(&id)
            .map_err(|_| Error::Integrity("移行対応表の ID が不正です".into())),
        None => Ok(LibraryId::now_v7()),
    }
}

pub(super) fn mark_row(tx: &Transaction<'_>, table: &str, key: &str, id: LibraryId) -> Result<()> {
    tx.execute("INSERT INTO migration_records(source_table,source_key,target_id,status) VALUES(?1,?2,?3,'imported') ON CONFLICT(source_table,source_key) DO UPDATE SET target_id=excluded.target_id,status=excluded.status",params![table,key,id.as_bytes().as_slice()])?;
    Ok(())
}

pub(super) fn marked(tx: &rusqlite::Connection, table: &str) -> Result<bool> {
    Ok(tx.query_row("SELECT status='imported' FROM migration_records WHERE source_table=?1 AND source_key='__all_rows__'",[table],|r|r.get(0)).optional()?.unwrap_or(false))
}

pub(super) fn mark_table(tx: &Transaction<'_>, table: &str) -> Result<()> {
    tx.execute("UPDATE migration_records SET status='imported' WHERE source_table=?1 AND source_key='__all_rows__'",[table])?;
    Ok(())
}
