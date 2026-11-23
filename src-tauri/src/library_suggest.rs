//! 棚の検索欄の候補。
//!
//! 作者・シリーズ・題・タグの前方一致を集め、足りなければよく使う名前の束から正規化と
//! 作者の読みで足す（組み立ては [`crate::search_suggest`]）。候補を押した先は
//! 返す：作者・シリーズは取得元と ID、題は取得元の ID と棚の作品の ID。

use crate::library::{Library, Result};
use crate::library_entity::id;
use crate::models::SearchSuggestion;
use crate::search_suggest::{self, Candidate, POOL_PER_KIND};
use rusqlite::{params, Connection};
use std::sync::{Arc, Mutex};

/// 候補の上限。 1〜50。
pub const SUGGESTION_LIMIT_MAX: i64 = 50;

/// 正規化で足す候補の束を、棚の世代ごとに覚えておく。
#[derive(Default)]
pub struct SuggestionPool(Mutex<Option<(i64, Arc<[Candidate]>)>>);

/// 作者・シリーズの取得元の ID。複数あれば取得元・ID の順で最初のもの（一覧と同じ）。
const IDENTITY: &str = "(SELECT x.provider FROM external_identities x WHERE x.entity_id={entity} AND x.kind='{kind}' ORDER BY x.provider,x.external_key LIMIT 1),
     (SELECT x.external_key FROM external_identities x WHERE x.entity_id={entity} AND x.kind='{kind}' ORDER BY x.provider,x.external_key LIMIT 1)";

fn identity(entity: &str, kind: &str) -> String {
    IDENTITY.replace("{entity}", entity).replace("{kind}", kind)
}

/// 種類ごとの問い合わせ。`condition` は前方一致（`?1`）か条件なし、`?2` が件数。
fn sql(kind: &str, condition: bool) -> String {
    let like = |column: &str| {
        if condition {
            format!("WHERE {column} LIKE ?1 ESCAPE '\\'")
        } else {
            format!("WHERE ?1 IS NULL OR {column}<>''")
        }
    };
    match kind {
        "tag" => format!(
            "SELECT t.name,count(DISTINCT p.work_id) FROM tags t
             JOIN snapshot_tags st ON st.tag_id=t.id
             JOIN work_current_projection p ON p.snapshot_id=st.snapshot_id
             {} GROUP BY t.name ORDER BY 2 DESC,t.name LIMIT ?2",
            like("t.name")
        ),
        "author" => format!(
            "SELECT r.display_name,count(DISTINCT p.work_id),{} FROM snapshot_people r
             JOIN work_current_projection p ON p.snapshot_id=r.snapshot_id
             {} AND r.display_name<>'' GROUP BY r.display_name,r.person_id
             ORDER BY 2 DESC,r.display_name LIMIT ?2",
            identity("r.person_id", "person"),
            if condition {
                "WHERE r.display_name LIKE ?1 ESCAPE '\\' COLLATE NOCASE".to_string()
            } else {
                "WHERE ?1 IS NULL".to_string()
            }
        ),
        "series" => format!(
            "SELECT r.title,count(DISTINCT p.work_id),{} FROM snapshot_series r
             JOIN work_current_projection p ON p.snapshot_id=r.snapshot_id
             {} GROUP BY r.series_id,r.title ORDER BY 2 DESC,r.title LIMIT ?2",
            identity("r.series_id", "series"),
            like("r.title")
        ),
        _ => format!(
            "SELECT p.title,p.external_key,p.provider,p.work_id FROM work_current_projection p
             {} ORDER BY p.saved_at DESC,p.work_id DESC LIMIT ?2",
            like("p.title")
        ),
    }
}

fn collect(
    conn: &Connection,
    prefix: Option<&str>,
    limit: i64,
    out: &mut Vec<SearchSuggestion>,
) -> Result<()> {
    for kind in ["tag", "author", "series", "title"] {
        let mut statement = conn.prepare_cached(&sql(kind, prefix.is_some()))?;
        let mut rows = statement.query(params![prefix, limit])?;
        while let Some(row) = rows.next()? {
            let label: String = row.get(0)?;
            let item = match kind {
                "tag" => SearchSuggestion {
                    kind: kind.into(),
                    value: label.clone(),
                    label,
                    count: Some(row.get(1)?),
                    exact_match: false,
                    source: None,
                    source_key: None,
                },
                "author" | "series" => {
                    let provider: Option<String> = row.get(2)?;
                    let key: Option<String> = row.get(3)?;
                    // シリーズは取得元の ID が無ければ開けないので出さない。
                    let value = match (kind, &provider, &key) {
                        ("author", ..) => label.clone(),
                        (_, Some(provider), Some(key)) => format!("{provider}:{key}"),
                        _ => continue,
                    };
                    SearchSuggestion {
                        kind: kind.into(),
                        label,
                        value,
                        count: Some(row.get(1)?),
                        exact_match: false,
                        source: provider,
                        source_key: key,
                    }
                }
                _ => SearchSuggestion {
                    kind: kind.into(),
                    label,
                    value: row.get(1)?,
                    count: Some(1),
                    exact_match: false,
                    source: Some(row.get(2)?),
                    source_key: Some(id(row.get(3)?)?.to_string()),
                },
            };
            out.push(item);
        }
    }
    Ok(())
}

impl Library {
    /// 検索欄の候補（`search_suggest` の並びと形）。語が空なら何も出さない。
    /// `limit` は 1〜[`SUGGESTION_LIMIT_MAX`] に収める。
    pub fn search_suggestions(
        &self,
        text: &str,
        limit: i64,
        pool: &SuggestionPool,
    ) -> Result<Vec<SearchSuggestion>> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let limit = limit.clamp(1, SUGGESTION_LIMIT_MAX);
        let conn = self.reader()?;
        let mut items = Vec::new();
        collect(
            &conn,
            Some(&search_suggest::like_prefix(text)),
            limit,
            &mut items,
        )?;
        search_suggest::assemble(text, limit, items, || {
            let generation = self.catalogue_generation()?;
            if let Some((cached, candidates)) = pool.0.lock().ok().and_then(|g| g.clone()) {
                if cached == generation {
                    return Ok(candidates);
                }
            }
            let mut loaded = Vec::new();
            collect(&conn, None, POOL_PER_KIND, &mut loaded)?;
            let candidates: Arc<[Candidate]> = loaded.into_iter().map(Candidate::new).collect();
            if let Ok(mut guard) = pool.0.lock() {
                *guard = Some((generation, candidates.clone()));
            }
            Ok(candidates)
        })
    }
}
