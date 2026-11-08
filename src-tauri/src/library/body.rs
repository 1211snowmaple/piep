use super::{Error, Fingerprint, Library, LibraryId, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{borrow::Cow, collections::BTreeMap};

pub(crate) const MAX_BODY_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BodyFormat {
    #[serde(rename = "plain_text_v1")]
    PlainText,
    #[serde(rename = "pixiv_text_v1")]
    PixivText,
    #[serde(rename = "fanbox_body_v1")]
    FanboxBody,
}
impl BodyFormat {
    pub fn name(self) -> &'static str {
        match self {
            Self::PlainText => "plain_text_v1",
            Self::PixivText => "pixiv_text_v1",
            Self::FanboxBody => "fanbox_body_v1",
        }
    }
    pub fn from_name(name: &str) -> Result<Self> {
        match name {
            "plain_text_v1" => Ok(Self::PlainText),
            "pixiv_text_v1" => Ok(Self::PixivText),
            "fanbox_body_v1" => Ok(Self::FanboxBody),
            _ => Err(Error::Unsupported(format!(
                "本文形式 {name} に対応していません"
            ))),
        }
    }
}

/// 元の本文と比較用の表現を分ける。構造化された本文を検索用の文章で置き換えない。
#[derive(Clone, Debug)]
pub enum SourceBody {
    PlainText(String),
    PixivText(String),
    FanboxBody(Value),
}
impl From<String> for SourceBody {
    fn from(text: String) -> Self {
        Self::PlainText(text)
    }
}
impl From<&str> for SourceBody {
    fn from(text: &str) -> Self {
        Self::PlainText(text.into())
    }
}

impl SourceBody {
    pub fn format(&self) -> BodyFormat {
        match self {
            Self::PlainText(_) => BodyFormat::PlainText,
            Self::PixivText(_) => BodyFormat::PixivText,
            Self::FanboxBody(_) => BodyFormat::FanboxBody,
        }
    }

    pub(crate) fn storage_bytes(&self) -> Result<Cow<'_, [u8]>> {
        let bytes: Cow<'_, [u8]> = match self {
            Self::PlainText(text) | Self::PixivText(text) => Cow::Borrowed(text.as_bytes()),
            Self::FanboxBody(body) if body.is_object() => Cow::Owned(serde_json::to_vec(body)?),
            Self::FanboxBody(_) => {
                return Err(Error::Integrity(
                    "記事の本文がオブジェクトではありません".into(),
                ))
            }
        };
        if bytes.len() as u64 > MAX_BODY_BYTES {
            return Err(Error::Unsupported(
                "本文が解釈の読み込み上限を超えています".into(),
            ));
        }
        Ok(bytes)
    }

    pub fn decode(format: BodyFormat, bytes: &[u8]) -> Result<Self> {
        if bytes.len() as u64 > MAX_BODY_BYTES {
            return Err(Error::Unsupported(
                "本文が解釈の読み込み上限を超えています".into(),
            ));
        }
        let text = || {
            String::from_utf8(bytes.to_vec())
                .map_err(|_| Error::Integrity("本文が UTF-8 ではありません".into()))
        };
        let body = match format {
            BodyFormat::PlainText => Self::PlainText(text()?),
            BodyFormat::PixivText => Self::PixivText(text()?),
            BodyFormat::FanboxBody => Self::FanboxBody(serde_json::from_slice(bytes)?),
        };
        body.storage_bytes()?;
        Ok(body)
    }

    /// 検索・集計用。画像等の意味を含む改稿判定には、この戻り値を使わない。
    pub fn text_content(&self) -> String {
        match self {
            Self::PlainText(text) | Self::PixivText(text) => text.clone(),
            Self::FanboxBody(body) => {
                if let Some(text) = body.get("text").and_then(Value::as_str) {
                    return text.into();
                }
                body.get("blocks")
                    .and_then(Value::as_array)
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter_map(|b| b.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default()
            }
        }
    }

    pub(crate) fn semantic_fingerprint(&self) -> Result<Fingerprint> {
        let value = match self {
            Self::PlainText(text) | Self::PixivText(text) => json!(text.replace("\r\n", "\n")),
            Self::FanboxBody(body) => {
                if !body.is_object() {
                    return Err(Error::Integrity(
                        "記事の本文がオブジェクトではありません".into(),
                    ));
                }
                let mut body = body.clone();
                normalize_text(body.get_mut("text"));
                if let Some(blocks) = body.get_mut("blocks").and_then(Value::as_array_mut) {
                    for block in blocks {
                        normalize_text(block.get_mut("text"));
                    }
                }
                // 既知の表示用 URL とアプリが注入したパスだけを除く。未知の属性は保持する。
                for key in ["imageMap", "fileMap"] {
                    if let Some(map) = body.get_mut(key).and_then(Value::as_object_mut) {
                        for file in map.values_mut().filter_map(Value::as_object_mut) {
                            file.remove("localPath");
                            if key == "imageMap" {
                                file.remove("thumbnailUrl");
                            }
                        }
                    }
                }
                canonical(body)
            }
        };
        Ok(Fingerprint::of(&serde_json::to_vec(&(
            "source-body-v1",
            self.format(),
            value,
        ))?))
    }
}

fn normalize_text(value: Option<&mut Value>) {
    if let Some(Value::String(text)) = value {
        *text = text.replace("\r\n", "\n");
    }
}
pub(super) fn canonical(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<_, _> = map
                .into_iter()
                .map(|(key, value)| (key, canonical(value)))
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonical).collect()),
        value => value,
    }
}

impl Library {
    fn decode_body_file(
        &self,
        conn: &rusqlite::Connection,
        id: Fingerprint,
        format: BodyFormat,
    ) -> Result<SourceBody> {
        let file = Self::file_from_conn(conn, id)?;
        if file.length > MAX_BODY_BYTES {
            return Err(Error::Unsupported(
                "本文が解釈の読み込み上限を超えています".into(),
            ));
        }
        let mut bytes = Vec::new();
        self.objects.export(&file, &mut bytes)?;
        SourceBody::decode(format, &bytes)
    }

    pub fn source_body(
        &self,
        entity: LibraryId,
        snapshot: LibraryId,
    ) -> Result<Option<SourceBody>> {
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        self.source_body_from_conn(&conn, entity, snapshot)
    }

    pub(super) fn source_body_from_conn(
        &self,
        conn: &rusqlite::Connection,
        entity: LibraryId,
        snapshot: LibraryId,
    ) -> Result<Option<SourceBody>> {
        let (file,format):(Option<Vec<u8>>,Option<String>)=conn.query_row("SELECT body_file_id,body_format FROM revision_snapshots WHERE entity_id=?1 AND id=?2 AND published=1",rusqlite::params![entity.as_bytes().as_slice(),snapshot.as_bytes().as_slice()],|r|Ok((r.get(0)?,r.get(1)?)))?;
        match (file, format) {
            (None, None) => Ok(None),
            (Some(file), Some(format)) => self
                .decode_body_file(
                    conn,
                    Fingerprint(
                        file.try_into()
                            .map_err(|_| Error::Integrity("本文のハッシュが不正です".into()))?,
                    ),
                    BodyFormat::from_name(&format)?,
                )
                .map(Some),
            _ => Err(Error::Integrity(
                "本文の形式とファイルの対応が不正です".into(),
            )),
        }
    }

    pub(crate) fn verify_source_bodies(&self, conn: &rusqlite::Connection) -> Result<()> {
        let mut stmt=conn.prepare("SELECT body_file_id,body_format,body_semantic_hash,body_semantic_rule FROM revision_snapshots WHERE published=1 AND body_file_id IS NOT NULL")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            if row.get::<_, i64>(3)? != 1 {
                return Err(Error::Unsupported(
                    "本文の比較規則に対応していません".into(),
                ));
            }
            let file: Vec<u8> = row.get(0)?;
            let format: String = row.get(1)?;
            let expected: Vec<u8> = row.get(2)?;
            let body = self.decode_body_file(
                conn,
                Fingerprint(
                    file.try_into()
                        .map_err(|_| Error::Integrity("本文のハッシュが不正です".into()))?,
                ),
                BodyFormat::from_name(&format)?,
            )?;
            if body.semantic_fingerprint()?.0.as_slice() != expected {
                return Err(Error::Integrity("本文が比較用の指紋と一致しません".into()));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rich_content_order_and_attributes_are_part_of_the_source_and_preview_paths_are_not() {
        let body = SourceBody::FanboxBody(
            json!({"blocks":[{"type":"p","text":"文章"},{"type":"image","imageId":"image"}],"imageMap":{"image":{"originalUrl":"https://example.test/original.png","thumbnailUrl":"https://example.test/thumb.png","localPath":"old/path.png"}}}),
        );
        let SourceBody::FanboxBody(mut changed) = body.clone() else {
            unreachable!()
        };
        changed["imageMap"]["image"]["thumbnailUrl"] =
            json!("https://example.test/other-thumb.png");
        changed["imageMap"]["image"]["localPath"] = json!("different/path.png");
        let preview = SourceBody::FanboxBody(changed.clone());
        assert_eq!(
            body.semantic_fingerprint().unwrap(),
            preview.semantic_fingerprint().unwrap()
        );
        assert_ne!(
            body.storage_bytes().unwrap(),
            preview.storage_bytes().unwrap()
        );
        changed["blocks"].as_array_mut().unwrap().reverse();
        let reordered = SourceBody::FanboxBody(changed);
        assert_eq!(body.text_content(), reordered.text_content());
        assert_ne!(
            body.semantic_fingerprint().unwrap(),
            reordered.semantic_fingerprint().unwrap()
        );
        let bytes = body.storage_bytes().unwrap();
        let restored = SourceBody::decode(body.format(), &bytes).unwrap();
        assert_eq!(bytes, restored.storage_bytes().unwrap());
    }

    #[test]
    fn comparison_can_normalize_line_endings_without_changing_the_stored_bytes() {
        let crlf = SourceBody::PixivText("　字下げ\r\n本文  \r\n".into());
        let lf = SourceBody::PixivText("　字下げ\n本文  \n".into());
        assert_eq!(
            crlf.semantic_fingerprint().unwrap(),
            lf.semantic_fingerprint().unwrap()
        );
        assert_ne!(crlf.storage_bytes().unwrap(), lf.storage_bytes().unwrap());
        assert_eq!(
            crlf.storage_bytes().unwrap().as_ref(),
            "　字下げ\r\n本文  \r\n".as_bytes()
        );
        assert_ne!(
            lf.semantic_fingerprint().unwrap(),
            SourceBody::PixivText("字下げ\n本文\n".into())
                .semantic_fingerprint()
                .unwrap()
        );
    }
}
