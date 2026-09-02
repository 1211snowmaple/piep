//! モデルに渡す材料を、棚から組み立てる。
//!
//! ここが**送るものと送らないものの境目**である。本文を含む材料を作る関数は
//! 名前に `with_body` と付けてあり、呼び出し側は利用者の許可を確かめてから
//! しか呼べない（許可の確認は `assist` 側でも二重にやる）。

use super::*;

use crate::database::collection_rules;

/// タグの候補として見せる語の上限。
///
/// 棚には2,975語ある。全部渡すと入力が膨れるうえ、めったに使われない語まで
/// 選択肢に入って外れが増える。よく使われている語から順に渡す。
const TAG_VOCABULARY_LIMIT: usize = 400;

/// 作風をまとめるときに渡す作品数の上限。
const AUTHOR_SAMPLE_LIMIT: usize = 30;

fn assisted_tag_name(value: &str) -> Option<&str> {
    let name = value.trim();
    (!name.is_empty() && name.chars().count() <= 100).then_some(name)
}

impl Database {
    /// 棚でよく使われているタグの語彙。
    ///
    /// モデルに**この中から選ばせる**ための一覧。新語を作らせないので、
    /// 付いたタグはそのまま検索にも束ねにも効く。
    pub fn tag_vocabulary(&self) -> Result<Vec<String>, String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT t.name FROM download_tags dt
                 JOIN tags t ON t.id = dt.tag_id
                 GROUP BY t.id
                 ORDER BY COUNT(*) DESC, t.name COLLATE NOCASE ASC
                 LIMIT ?1",
            )
            .map_err(|e| format!("Failed to prepare tag vocabulary: {e}"))?;
        let rows = stmt
            .query_map(params![TAG_VOCABULARY_LIMIT as i64], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| format!("Failed to query tag vocabulary: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            let name = row.map_err(|e| format!("Failed to read tag vocabulary: {e}"))?;
            // 束も検索も言い表せない語は、選択肢にしても意味が無い。
            if collection_rules::is_informative_tag(&name) {
                out.push(name);
            }
        }
        Ok(out)
    }

    /// 検証用に、束らしい作品の id を外から取れるようにする。
    pub fn sample_bundle_like_ids_public(&self) -> Result<Vec<i64>, String> {
        self.sample_bundle_like_ids()
    }

    /// 表示名から作者の材料を集める。取得元をまたいで同じ人を拾う。
    pub fn author_facts_by_name(
        &self,
        author_name: &str,
    ) -> Result<(String, Vec<crate::assist::WorkFacts>), String> {
        let ids = {
            let conn = self.read_conn()?;
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM downloads WHERE author_name = ?1
                     ORDER BY COALESCE(source_created_at, downloaded_at) DESC LIMIT ?2",
                )
                .map_err(|e| format!("Failed to prepare author works: {e}"))?;
            let ids = stmt
                .query_map(params![author_name, AUTHOR_SAMPLE_LIMIT as i64], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(|e| format!("Failed to query author works: {e}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Failed to read author works: {e}"))?;
            ids
        };
        if ids.is_empty() {
            return Err("この作者の作品がありません".to_string());
        }
        let mut facts = Vec::new();
        for id in ids {
            facts.push(self.work_facts(id)?);
        }
        Ok((author_name.to_string(), facts))
    }

    /// 作品ひとつぶんの、本文を含まない材料。
    pub fn work_facts(&self, download_id: i64) -> Result<crate::assist::WorkFacts, String> {
        let conn = self.read_conn()?;
        let (title, author_name, excerpt) = conn
            .query_row(
                "SELECT title, author_name, excerpt FROM downloads WHERE id = ?1",
                params![download_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| format!("Failed to query work facts: {e}"))?
            .ok_or_else(|| "作品が見つかりません".to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT t.name FROM download_tags dt
                 JOIN tags t ON t.id = dt.tag_id
                 WHERE dt.download_id = ?1
                 ORDER BY t.name COLLATE NOCASE",
            )
            .map_err(|e| format!("Failed to prepare work tags: {e}"))?;
        let tags = stmt
            .query_map(params![download_id], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Failed to query work tags: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read work tags: {e}"))?;
        Ok(crate::assist::WorkFacts {
            title,
            author_name,
            tags,
            excerpt: excerpt.filter(|value| !value.trim().is_empty()),
        })
    }

    /// この作者の作品を、作風をまとめられるだけ集める。本文は含まない。
    pub fn author_facts(
        &self,
        source: &str,
        person_key: &str,
    ) -> Result<(String, Vec<crate::assist::WorkFacts>), String> {
        let ids = self.author_work_ids(source, person_key)?;
        if ids.is_empty() {
            return Err("この作者の作品がありません".to_string());
        }
        let mut facts = Vec::with_capacity(ids.len());
        for id in ids {
            facts.push(self.work_facts(id)?);
        }
        let author = facts
            .first()
            .map(|value| value.author_name.clone())
            .unwrap_or_else(|| person_key.to_string());
        Ok((author, facts))
    }

    /// この作者の作品の id を、新しい順に集める。
    fn author_work_ids(&self, source: &str, person_key: &str) -> Result<Vec<i64>, String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT d.id FROM downloads d
                 WHERE d.source = ?1 AND (d.author_id = ?2 OR d.author_name = ?2)
                 ORDER BY COALESCE(d.source_created_at, d.downloaded_at) DESC
                 LIMIT ?3",
            )
            .map_err(|e| format!("Failed to prepare author works: {e}"))?;
        let ids = stmt
            .query_map(
                params![source, person_key, AUTHOR_SAMPLE_LIMIT as i64],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|e| format!("Failed to query author works: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read author works: {e}"))?;
        Ok(ids)
    }

    /// 束のメンバーを、分割の案を出せる形で並べる。本文は含まない。
    pub fn collection_facts(
        &self,
        collection_id: &str,
    ) -> Result<Vec<crate::assist::WorkFacts>, String> {
        let collection = self.get_work_collection(collection_id)?;
        let mut facts = Vec::new();
        for member in &collection.members {
            let Some(id) = member.download_id else {
                continue;
            };
            facts.push(self.work_facts(id)?);
        }
        Ok(facts)
    }

    /// あらすじを作るための、**本文を含む**材料。
    ///
    /// 許可の確認は `assist` 側でやる。ここは読むだけだが、名前で分かるように
    /// してある — 本文が乗る経路が一目で分かることが大事である。
    pub fn work_facts_with_body(
        &self,
        download_id: i64,
    ) -> Result<(crate::assist::WorkFacts, String), String> {
        let facts = self.work_facts(download_id)?;
        let document = self.get_reader_document(download_id, None)?;
        Ok((facts, document.plain_text))
    }

    /// モデルの案から採ったタグを付ける。
    ///
    /// `tag_source = 'llm'` で入れる。**取得元が付けたタグと混ぜない** —
    /// 確からしさが違うものを見分けられなくなった時点で、両方が信用できなく
    /// なる。取り直しで消えるのも `origin` だけである。
    pub fn add_assisted_tags(
        &self,
        download_id: i64,
        tags: &[String],
    ) -> Result<Vec<String>, String> {
        if tags.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Tag transaction failed: {e}"))?;
        let mut added = Vec::new();
        for name in tags {
            let Some(name) = assisted_tag_name(name) else {
                continue;
            };
            tx.execute(
                "INSERT OR IGNORE INTO tags (name) VALUES (?1)",
                params![name],
            )
            .map_err(|e| format!("Failed to insert tag: {e}"))?;
            let tag_id: i64 = tx
                .query_row(
                    "SELECT id FROM tags WHERE name = ?1",
                    params![name],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Failed to retrieve tag id: {e}"))?;
            // すでに付いているなら触らない。取得元のタグを `llm` に書き換えない。
            let changed = tx
                .execute(
                    "INSERT OR IGNORE INTO download_tags (download_id, tag_id, tag_source)
                     VALUES (?1, ?2, 'llm')",
                    params![download_id, tag_id],
                )
                .map_err(|e| format!("Failed to attach tag: {e}"))?;
            if changed > 0 {
                added.push(name.to_string());
            }
        }
        if !added.is_empty() {
            // Tags are part of both lexical and semantic documents. Remove the
            // success markers in the same transaction as the tag mutation so
            // a later reindex failure is visible as pending, never as a
            // falsely complete index containing the previous tags.
            tx.execute(
                "DELETE FROM search_index_state WHERE download_id = ?1",
                params![download_id],
            )
            .map_err(|e| format!("Failed to invalidate search index state: {e}"))?;
            tx.execute(
                "DELETE FROM semantic_index_state WHERE download_id = ?1",
                params![download_id],
            )
            .map_err(|e| format!("Failed to invalidate semantic index state: {e}"))?;
        }
        tx.commit().map_err(|e| format!("Tag commit failed: {e}"))?;
        Ok(added)
    }

    /// この作品のタグを、出どころ付きで返す。
    pub fn work_tags_with_source(&self, download_id: i64) -> Result<Vec<TaggedName>, String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT t.name, dt.tag_source FROM download_tags dt
                 JOIN tags t ON t.id = dt.tag_id
                 WHERE dt.download_id = ?1
                 ORDER BY dt.tag_source = 'llm', t.name COLLATE NOCASE",
            )
            .map_err(|e| format!("Failed to prepare tag sources: {e}"))?;
        let rows = stmt
            .query_map(params![download_id], |row| {
                Ok(TaggedName {
                    name: row.get(0)?,
                    source: row.get(1)?,
                })
            })
            .map_err(|e| format!("Failed to query tag sources: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read tag sources: {e}"))
    }

    /// モデルの案から採ったタグを外す。取得元のタグは外せない。
    pub fn remove_assisted_tag(&self, download_id: i64, tag: &str) -> Result<bool, String> {
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Tag transaction failed: {e}"))?;
        let removed = tx
            .execute(
                "DELETE FROM download_tags
                 WHERE download_id = ?1 AND tag_source = 'llm'
                   AND tag_id = (SELECT id FROM tags WHERE name = ?2)",
                params![download_id, tag],
            )
            .map_err(|e| format!("Failed to remove tag: {e}"))?;
        if removed > 0 {
            tx.execute(
                "DELETE FROM search_index_state WHERE download_id = ?1",
                params![download_id],
            )
            .map_err(|e| format!("Failed to invalidate search index state: {e}"))?;
            tx.execute(
                "DELETE FROM semantic_index_state WHERE download_id = ?1",
                params![download_id],
            )
            .map_err(|e| format!("Failed to invalidate semantic index state: {e}"))?;
        }
        tx.commit().map_err(|e| format!("Tag commit failed: {e}"))?;
        Ok(removed > 0)
    }

    /// モデルに書いてもらった覚え書きを保存する。
    pub fn save_ai_note_with_provenance(
        &self,
        subject_type: &str,
        subject_key: &str,
        note_kind: &str,
        text: &str,
        provenance: &crate::assist::AssistProvenance,
    ) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO ai_notes (
                subject_type, subject_key, note_kind, text, model_id, feature_id,
                prompt_version, input_fingerprint, config_fingerprint, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(subject_type, subject_key, note_kind)
             DO UPDATE SET text = excluded.text, model_id = excluded.model_id,
                           feature_id = excluded.feature_id,
                           prompt_version = excluded.prompt_version,
                           input_fingerprint = excluded.input_fingerprint,
                           config_fingerprint = excluded.config_fingerprint,
                           created_at = excluded.created_at",
            params![
                subject_type,
                subject_key,
                note_kind,
                text,
                provenance.model_id,
                provenance.feature_id,
                provenance.prompt_version,
                provenance.input_fingerprint,
                provenance.config_fingerprint,
                provenance.created_at,
            ],
        )
        .map_err(|e| format!("Failed to save note: {e}"))?;
        Ok(())
    }

    /// テスト用・非LLM経路向けの簡易保存。生成経路は必ず provenance 版を使う。
    pub fn save_ai_note(
        &self,
        subject_type: &str,
        subject_key: &str,
        note_kind: &str,
        text: &str,
        model_id: &str,
    ) -> Result<(), String> {
        self.save_ai_note_with_provenance(
            subject_type,
            subject_key,
            note_kind,
            text,
            &crate::assist::AssistProvenance {
                feature_id: "legacy".to_string(),
                prompt_version: "legacy/v1".to_string(),
                model_id: model_id.to_string(),
                input_fingerprint: String::new(),
                config_fingerprint: String::new(),
                created_at: chrono::Utc::now().to_rfc3339(),
            },
        )
    }

    /// 保存してある覚え書きを読む。無ければ `None`。
    pub fn load_ai_note(
        &self,
        subject_type: &str,
        subject_key: &str,
        note_kind: &str,
    ) -> Result<Option<AiNote>, String> {
        let conn = self.read_conn()?;
        let note = conn
            .query_row(
                "SELECT text, model_id, feature_id, prompt_version, input_fingerprint,
                    config_fingerprint, created_at FROM ai_notes
             WHERE subject_type = ?1 AND subject_key = ?2 AND note_kind = ?3",
                params![subject_type, subject_key, note_kind],
                |row| {
                    Ok(AiNote {
                        text: row.get(0)?,
                        model_id: row.get(1)?,
                        feature_id: row.get(2)?,
                        prompt_version: row.get(3)?,
                        input_fingerprint: row.get(4)?,
                        config_fingerprint: row.get(5)?,
                        created_at: row.get(6)?,
                        prompt_stale: false,
                        input_stale: false,
                    })
                },
            )
            .optional()
            .map_err(|e| format!("Failed to read note: {e}"))?;
        drop(conn);
        let Some(mut note) = note else {
            return Ok(None);
        };
        note.prompt_stale =
            note.prompt_version != crate::assist::current_prompt_version(&note.feature_id);
        if let Some(current) = self.current_ai_note_input_fingerprint(
            subject_type,
            subject_key,
            note_kind,
            &note.feature_id,
        )? {
            note.input_stale = current != note.input_fingerprint;
        }
        Ok(Some(note))
    }

    fn current_ai_note_input_fingerprint(
        &self,
        subject_type: &str,
        subject_key: &str,
        note_kind: &str,
        feature_id: &str,
    ) -> Result<Option<String>, String> {
        let input = match (subject_type, note_kind, feature_id) {
            ("work", "synopsis", crate::assist::FEATURE_WORK_SYNOPSIS) => {
                let Ok(download_id) = subject_key.parse::<i64>() else {
                    return Ok(None);
                };
                let (work, body) = self.work_facts_with_body(download_id)?;
                serde_json::json!({ "work": work, "body": body })
            }
            ("person", "style", crate::assist::FEATURE_AUTHOR_STYLE) => {
                let Some((source, person_key)) = subject_key.split_once(':') else {
                    return Ok(None);
                };
                let (author, works) = self.author_facts(source, person_key)?;
                serde_json::json!({ "author": author, "works": works })
            }
            ("work", "recap", crate::assist::FEATURE_READER_RECAP) => {
                let Some((current, previous)) = subject_key.split_once(':') else {
                    return Ok(None);
                };
                let (Ok(current_download_id), Ok(previous_download_id)) =
                    (current.parse::<i64>(), previous.parse::<i64>())
                else {
                    return Ok(None);
                };
                let (work, body) = self.work_facts_with_body(previous_download_id)?;
                serde_json::json!({
                    "previousDownloadId": previous_download_id,
                    "currentDownloadId": current_download_id,
                    "work": work,
                    "body": body
                })
            }
            _ => return Ok(None),
        };
        crate::assist::generation_input_fingerprint(feature_id, &input).map(Some)
    }

    /// 覚え書きを消す。作り直したいときと、要らなくなったとき。
    pub fn delete_ai_note(
        &self,
        subject_type: &str,
        subject_key: &str,
        note_kind: &str,
    ) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let removed = conn
            .execute(
                "DELETE FROM ai_notes
                 WHERE subject_type = ?1 AND subject_key = ?2 AND note_kind = ?3",
                params![subject_type, subject_key, note_kind],
            )
            .map_err(|e| format!("Failed to delete note: {e}"))?;
        Ok(removed > 0)
    }
}

/// 出どころの付いたタグ。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaggedName {
    pub name: String,
    /// `origin`（取得元）／`manual`（利用者）／`llm`（モデルの案を採った）
    pub source: String,
}

/// モデルに書いてもらった覚え書き。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiNote {
    pub text: String,
    /// どのモデルが書いたか。モデルを替えたときに古い文が混ざるのを防ぐ。
    pub model_id: String,
    pub feature_id: String,
    pub prompt_version: String,
    pub input_fingerprint: String,
    pub config_fingerprint: String,
    pub created_at: String,
    /// 固定promptの版が現在と異なる。入力・設定のstale判定には各fingerprintを使う。
    pub prompt_stale: bool,
    /// Title, tags, body, or other feature input changed after generation.
    pub input_stale: bool,
}

#[cfg(test)]
mod tests {
    use super::assisted_tag_name;

    #[test]
    fn model_tags_are_trimmed_and_bounded_in_characters() {
        assert_eq!(assisted_tag_name("  幻想  "), Some("幻想"));
        assert!(assisted_tag_name("   ").is_none());
        assert!(assisted_tag_name(&"界".repeat(100)).is_some());
        assert!(assisted_tag_name(&"界".repeat(101)).is_none());
    }
}
