//! 実行キューに載せる作品の、表示用の情報（作者・公開日など）。本文は含めない。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionMetadata {
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
    pub fn from_payload(payload: &serde_json::Value) -> Self {
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

    pub fn execution_display(payload: &str) -> serde_json::Result<String> {
        let v: serde_json::Value = serde_json::from_str(payload)?;
        Ok(serde_json::json!({"metadata":Self::from_payload(&v),"kind":v.get("kind"),"subtitle":v.get("subtitle"),"targetLabel":v.get("targetLabel"),"holdReason":v.get("holdReason")}).to_string())
    }
}
