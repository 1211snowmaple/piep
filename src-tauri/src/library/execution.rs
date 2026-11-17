//! 旧キューの再確認を、移行した判断の世代へ結ぶ。原入力は書き換えない。
use super::{Error, LibraryId, Result};
use rusqlite::{Connection, OptionalExtension};

pub(crate) fn legacy_recheck_generation(
    conn: &Connection,
    work: Option<LibraryId>,
    payload: &serde_json::Value,
) -> Result<Option<i64>> {
    if payload.get("kind").and_then(serde_json::Value::as_str) != Some("permission_check") {
        return Ok(None);
    }
    let work = work.ok_or_else(|| Error::Integrity("再確認する作品の識別がありません".into()))?;
    let current:Option<(String,Option<String>,i64)>=conn.query_row("SELECT decision,hold_kind,coalesce((SELECT generation FROM discovery_state WHERE work_id=d.work_id),0) FROM discovery_candidates d WHERE work_id=?1",[work.as_bytes().as_slice()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let (decision, kind, generation) =
        current.ok_or_else(|| Error::Integrity("再確認する保留の判断がありません".into()))?;
    if decision != "held"
        || kind.as_deref() != payload.get("holdKind").and_then(serde_json::Value::as_str)
    {
        return Err(Error::Integrity(
            "再確認を依頼したときの保留と現在の判断が一致しません".into(),
        ));
    }
    match payload.get("discoveryGeneration") {
        None | Some(serde_json::Value::Null) => Ok(Some(generation)),
        Some(value) if value.as_i64() == Some(generation) => Ok(None),
        Some(_) => Err(Error::Integrity(
            "再確認を依頼した判断の世代が現在と一致しません".into(),
        )),
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExecutionMetadata {
    pub author_name: Option<String>,
    pub published_at: Option<String>,
    pub updated_at: Option<String>,
    pub saved_at: Option<String>,
}
// 表示用の解釈はここに固定し、取得 API やデスクトップのモデルへ依存させない。
pub(super) fn metadata_post(value: &serde_json::Value) -> &serde_json::Value {
    let mut current = value;
    for _ in 0..=8 {
        if current.get("id").is_some()
            && ["title", "creatorId", "publishedDatetime", "type"]
                .iter()
                .any(|key| current.get(*key).is_some())
        {
            return current;
        }
        let Some(next) = ["post", "data", "result", "body"]
            .iter()
            .find_map(|key| current.get(*key).filter(|v| v.is_object()))
        else {
            break;
        };
        current = next;
    }
    value
}
impl ExecutionMetadata {
    pub(crate) fn from_payload(payload: &serde_json::Value) -> Self {
        fn field(value: &serde_json::Value, paths: &[&str]) -> Option<String> {
            paths.iter().find_map(|path| {
                value
                    .pointer(path)
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_string)
            })
        }
        let value = payload.get("originalData").unwrap_or(payload);
        let value = metadata_post(value);
        Self {
            author_name: field(
                value,
                &[
                    "/user/name",
                    "/creator/name",
                    "/authorName",
                    "/userName",
                    "/author_name",
                    "/detail/user/name",
                    "/detail/authorName",
                ],
            ),
            published_at: field(
                value,
                &[
                    "/create_date",
                    "/createDate",
                    "/publishedDatetime",
                    "/cdate",
                    "/detail/createDate",
                    "/detail/cdate",
                ],
            ),
            updated_at: field(
                value,
                &[
                    "/uploadDate",
                    "/updateDate",
                    "/updatedDatetime",
                    "/detail/uploadDate",
                ],
            ),
            saved_at: None,
        }
    }

    pub(crate) fn execution_display(payload: &str) -> serde_json::Result<String> {
        let v: serde_json::Value = serde_json::from_str(payload)?;
        Ok(serde_json::json!({"metadata":Self::from_payload(&v),"kind":v.get("kind"),"subtitle":v.get("subtitle"),"targetLabel":v.get("targetLabel"),"holdReason":v.get("holdReason")}).to_string())
    }
}
