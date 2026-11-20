use super::{body::canonical, Error, Fingerprint, LibraryId, Result};
use rusqlite::{types::ValueRef, Connection};
use serde_json::{json, Value};

fn rows(conn: &Connection, snapshot: LibraryId, sql: &str) -> Result<Value> {
    let mut stmt = conn.prepare(sql)?;
    let columns = stmt.column_count();
    let mut cursor = stmt.query([snapshot.as_bytes().as_slice()])?;
    let mut records = Vec::new();
    while let Some(row) = cursor.next()? {
        let mut record = Vec::with_capacity(columns);
        for column in 0..columns {
            record.push(match row.get_ref(column)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(value) => json!(value),
                ValueRef::Text(value) => json!(std::str::from_utf8(value)
                    .map_err(|_| Error::Integrity("保存状態の文字列が不正です".into()))?),
                ValueRef::Blob(value) => json!(value),
                ValueRef::Real(_) => {
                    return Err(Error::Integrity(
                        "保存状態の参照に不正な数値があります".into(),
                    ))
                }
            });
        }
        records.push(Value::Array(record));
    }
    Ok(Value::Array(records))
}

/// 取得日時・原応答・履歴用 UUID を含めず、実際に選ばれた保存内容だけを比較する。
/// 元の応答が変わっても、この状態が同じなら取得の記録だけを増やせる。
pub(super) fn fingerprint(conn: &Connection, snapshot: LibraryId) -> Result<Fingerprint> {
    let (metadata, states): (String, String) = conn.query_row(
        "SELECT metadata_json,field_states_json FROM revision_snapshots WHERE id=?1",
        [snapshot.as_bytes().as_slice()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let metadata = canonical(serde_json::from_str(&metadata)?);
    let states = canonical(serde_json::from_str(&states)?);
    let body=rows(conn,snapshot,"SELECT body_file_id,body_format,body_semantic_rule,interpretation_version,semantic_rule FROM revision_snapshots WHERE id=?1")?;
    let media=rows(conn,snapshot,"SELECT m.slot,m.role,s.provider,s.kind,s.source_key,s.identity_rule,r.file_id,r.mime_type,r.quality,r.width,r.height,m.original_name FROM snapshot_media m JOIN source_media s ON s.id=m.media_id LEFT JOIN media_renditions r ON r.id=m.rendition_id AND r.media_id=m.media_id WHERE m.snapshot_id=?1 ORDER BY m.slot")?;
    let files=rows(conn,snapshot,"SELECT slot,file_id,original_name,mime_type,role FROM snapshot_files WHERE snapshot_id=?1 ORDER BY slot")?;
    let people=rows(conn,snapshot,"SELECT person_id,role,display_name FROM snapshot_people WHERE snapshot_id=?1 ORDER BY person_id,role")?;
    let series=rows(conn,snapshot,"SELECT series_id,title,position FROM snapshot_series WHERE snapshot_id=?1 ORDER BY series_id")?;
    let tags = rows(
        conn,
        snapshot,
        "SELECT tag_id FROM snapshot_tags WHERE snapshot_id=?1 ORDER BY tag_id",
    )?;
    Ok(Fingerprint::of(&serde_json::to_vec(&(
        "saved-state-v1",
        metadata,
        states,
        body,
        media,
        files,
        people,
        series,
        tags,
    ))?))
}

#[cfg(test)]
#[path = "saved_state_tests.rs"]
mod tests;
