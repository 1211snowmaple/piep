use super::{Error, Fingerprint, Library, LibraryId, Result};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

/// モデル等の根拠は解釈し直さずに保存する。未知の属性や小数の表記も保つ。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SuggestionJson(Box<RawValue>);
impl SuggestionJson {
    pub fn original_json(&self) -> &str {
        self.0.get()
    }
}
impl PartialEq for SuggestionJson {
    fn eq(&self, other: &Self) -> bool {
        self.0.get() == other.0.get()
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionDecision {
    Pending,
    Accepted,
    Rejected,
}
impl SuggestionDecision {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "accepted" => Ok(Self::Accepted),
            "rejected" => Ok(Self::Rejected),
            _ => Err(Error::Unsupported(
                "提案の判断状態に対応していません".into(),
            )),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SuggestedMember {
    pub work: LibraryId,
    pub title: String,
    pub author: String,
    pub preview: Option<Fingerprint>,
    pub preview_name: Option<String>,
    pub text_length: i64,
    pub position: i64,
    pub score: f64,
    pub selected: bool,
    pub evidence: Vec<SuggestionJson>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CollectionSuggestion {
    pub id: LibraryId,
    pub title: String,
    pub ordered: bool,
    pub track: String,
    pub origin: String,
    pub decision: SuggestionDecision,
    pub score: f64,
    pub rule_version: String,
    pub explanation: Option<String>,
    pub name_options: Vec<SuggestionJson>,
    pub created_at: i64,
    pub updated_at: i64,
    pub generation: i64,
    pub seeds: Vec<LibraryId>,
    pub members: Vec<SuggestedMember>,
}

fn id(bytes: Vec<u8>) -> Result<LibraryId> {
    LibraryId::from_slice(&bytes).map_err(|_| Error::Integrity("提案の識別子が不正です".into()))
}

impl Library {
    /// 提案は当時のラベル・根拠・選択を返す。現在の作品情報で上書きしない。
    pub fn collection_suggestion(&self, suggestion: LibraryId) -> Result<CollectionSuggestion> {
        let conn = self.reader()?;
        let mut output = conn.query_row("SELECT title,ordered,track,origin,decision,score,rule_version,explanation,name_options_json,created_at,updated_at,generation FROM collection_suggestions WHERE id=?1",[suggestion.as_bytes().as_slice()],|r|Ok(CollectionSuggestion {
            id:suggestion,title:r.get(0)?,ordered:r.get(1)?,track:r.get(2)?,origin:r.get(3)?,decision:SuggestionDecision::parse(&r.get::<_,String>(4)?).map_err(|e|rusqlite::Error::FromSqlConversionFailure(4,rusqlite::types::Type::Text,Box::new(e)))?,score:r.get(5)?,rule_version:r.get(6)?,explanation:r.get(7)?,name_options:serde_json::from_str(&r.get::<_,String>(8)?).map_err(|e|rusqlite::Error::FromSqlConversionFailure(8,rusqlite::types::Type::Text,Box::new(e)))?,created_at:r.get(9)?,updated_at:r.get(10)?,generation:r.get(11)?,seeds:vec![],members:vec![],
        }))?;
        let mut seeds = conn.prepare("SELECT work_id FROM collection_suggestion_seeds WHERE suggestion_id=?1 ORDER BY position")?;
        output.seeds = seeds
            .query_map([suggestion.as_bytes().as_slice()], |r| {
                r.get::<_, Vec<u8>>(0)
            })?
            .map(|row| id(row?))
            .collect::<Result<_>>()?;
        let mut stmt = conn.prepare("SELECT work_id,title_at_proposal,author_at_proposal,preview_file_id,preview_name,text_length,position,score,selected,evidence_json FROM collection_suggestion_members WHERE suggestion_id=?1 ORDER BY position,work_id")?;
        let mut rows = stmt.query(params![suggestion.as_bytes().as_slice()])?;
        while let Some(row) = rows.next()? {
            let preview: Option<Vec<u8>> = row.get(3)?;
            output.members.push(SuggestedMember {
                work: id(row.get(0)?)?,
                title: row.get(1)?,
                author: row.get(2)?,
                preview: preview
                    .map(|b| {
                        Ok::<_, Error>(Fingerprint(
                            b.try_into().map_err(|_| {
                                Error::Integrity("提案の画像の識別が不正です".into())
                            })?,
                        ))
                    })
                    .transpose()?,
                preview_name: row.get(4)?,
                text_length: row.get(5)?,
                position: row.get(6)?,
                score: row.get(7)?,
                selected: row.get(8)?,
                evidence: serde_json::from_str(&row.get::<_, String>(9)?)?,
            });
        }
        Ok(output)
    }
}
