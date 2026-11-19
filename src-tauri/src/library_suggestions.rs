//! コレクションの提案を、新しい棚で作る・読む・採る・断る（評価文書 4.1 の 4）。
//!
//! 提案の計算は旧棚と同じものを使う（`library_suggestion_corpus` が棚から作業用の表を組む）。
//! ここは計算の結果を棚の表（`collection_suggestions`・`collection_suggestion_seeds`・
//! `collection_suggestion_members`・`collection_pair_feedback`）へ書き、画面の形で返す。
//!
//! 画面へ返す形は旧棚と同じ（`database::CollectionSuggestion`）。候補の ID は UUID、作品は
//! `work`（作品 UUID）で指し、`download_id` は持たない。

use crate::database::queries::{
    accepted_suggestion_name_source, collection_additions, name_proposals_in, naming_works_in,
    sample_bundle_like_ids_in, seed_suggestion_plan, sweep_identity, sweep_plan, PreparedSweep,
    COLLECTION_SUGGEST_RULE_VERSION,
};
use crate::database::{
    AcceptCollectionSuggestionInput, CollectionAdditionResult, CollectionNameCandidate,
    CollectionSuggestion, CollectionSuggestionEvidence, CollectionSuggestionMember,
    CollectionSweepResult, WorkKey,
};
use crate::library::{Library, LibraryId};
use crate::library_collection_edit::CollectionEdit;
use crate::library_suggestion_corpus::ShelfSuggestions;
use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::HashSet;

type Result<T> = std::result::Result<T, String>;

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn time(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default()
}

fn uuid(bytes: &[u8]) -> Result<LibraryId> {
    LibraryId::from_slice(bytes).map_err(|_| "識別子が不正です".to_string())
}

fn parse_id(id: &str, what: &str) -> Result<LibraryId> {
    id.parse::<LibraryId>()
        .map_err(|_| format!("{what}の ID が不正です"))
}

/// 表紙の識別子（16 進）を、棚のファイルの ID に戻す。
fn preview_file(cover: Option<&str>) -> Option<Vec<u8>> {
    let hex = cover?;
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..32)
        .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
        .collect()
}

/// 候補の作品を、作業用の表の番号から棚の作品へ置き換える。
fn shelf_members(
    corpus: &ShelfSuggestions<'_>,
    members: Vec<CollectionSuggestionMember>,
) -> Result<Vec<(LibraryId, CollectionSuggestionMember)>> {
    members
        .into_iter()
        .map(|mut member| {
            let work = member
                .download_id
                .and_then(|id| corpus.work(id))
                .ok_or_else(|| "候補の作品が棚にありません".to_string())?;
            member.download_id = None;
            member.work = Some(work.to_string());
            Ok((work, member))
        })
        .collect()
}

fn bump(tx: &Transaction<'_>) -> Result<()> {
    tx.execute(
        "UPDATE library_meta SET generation=generation+1 WHERE singleton=1",
        [],
    )
    .map_err(err)?;
    Ok(())
}

fn delete_suggestion(tx: &Transaction<'_>, id: &[u8]) -> Result<usize> {
    tx.execute(
        "DELETE FROM collection_suggestion_members WHERE suggestion_id=?1",
        [id],
    )
    .map_err(err)?;
    tx.execute(
        "DELETE FROM collection_suggestion_seeds WHERE suggestion_id=?1",
        [id],
    )
    .map_err(err)?;
    tx.execute("DELETE FROM collection_suggestions WHERE id=?1", [id])
        .map_err(err)
}

struct NewSuggestion<'a> {
    id: LibraryId,
    title: &'a str,
    ordered: bool,
    track: &'a str,
    origin: &'a str,
    score: f64,
    explanation: &'a str,
    names: &'a [CollectionNameCandidate],
    seeds: &'a [LibraryId],
    members: &'a [(LibraryId, CollectionSuggestionMember)],
    now: i64,
}

fn insert_suggestion(tx: &Transaction<'_>, s: &NewSuggestion<'_>) -> Result<()> {
    let id = s.id.as_bytes().as_slice();
    tx.execute(
        "INSERT INTO collection_suggestions(id,title,ordered,track,origin,decision,score,rule_version,
             explanation,name_options_json,created_at,updated_at,generation)
         VALUES(?1,?2,?3,?4,?5,'pending',?6,?7,?8,?9,?10,?10,1)",
        params![
            id,
            s.title,
            s.ordered,
            s.track,
            s.origin,
            s.score.clamp(0.0, 1.0),
            COLLECTION_SUGGEST_RULE_VERSION,
            Some(s.explanation).filter(|e| !e.trim().is_empty()),
            serde_json::to_string(s.names).map_err(err)?,
            s.now,
        ],
    )
    .map_err(err)?;
    let mut seen = HashSet::new();
    for (position, seed) in s.seeds.iter().filter(|w| seen.insert(**w)).enumerate() {
        tx.execute(
            "INSERT INTO collection_suggestion_seeds VALUES(?1,?2,?3)",
            params![id, seed.as_bytes().as_slice(), position as i64],
        )
        .map_err(err)?;
    }
    for (work, member) in s.members {
        let preview = preview_file(member.cover_path.as_deref());
        let name = preview.as_ref().map(|_| "cover");
        tx.execute(
            "INSERT INTO collection_suggestion_members VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                id,
                work.as_bytes().as_slice(),
                member.title,
                member.author_name,
                preview,
                name,
                member.text_length.max(0),
                member.proposed_position,
                member.score.clamp(0.0, 1.0),
                member.selected,
                serde_json::to_string(&member.evidence).map_err(err)?,
            ],
        )
        .map_err(err)?;
    }
    Ok(())
}

impl Library {
    /// 棚全体を走査して、束の候補を作り直す。走査で作った未判断の候補だけを入れ替え、
    /// 採った・断った候補と、種から作った候補には触らない。
    pub fn sweep_collection_suggestions(&self) -> Result<CollectionSweepResult> {
        let corpus = ShelfSuggestions::load(self).map_err(err)?;
        let plan = sweep_plan(&corpus)?;
        let now = chrono::Utc::now().timestamp_millis();
        let mut prepared = Vec::new();
        for PreparedSweep {
            track,
            strength,
            ids_json,
            names,
            members,
            evidence,
        } in plan.prepared
        {
            let digest = sweep_identity(COLLECTION_SUGGEST_RULE_VERSION, track, &members)?;
            let mut bytes = [0u8; 16];
            bytes.copy_from_slice(&digest[..16]);
            let id = uuid::Builder::from_custom_bytes(bytes).into_uuid();
            let seeds = serde_json::from_str::<Vec<i64>>(&ids_json)
                .map_err(err)?
                .into_iter()
                .filter_map(|id| corpus.work(id))
                .collect::<Vec<_>>();
            prepared.push((
                id,
                track,
                strength,
                seeds,
                names,
                shelf_members(&corpus, members)?,
                evidence,
            ));
        }
        drop(corpus);

        let mut conn = self.conn.lock().map_err(|_| "棚が使用中です".to_string())?;
        let tx = conn.transaction().map_err(err)?;
        // 前回の走査の未判断の候補。モデルが付けた名前の案は、同じ候補なら引き継ぐ。
        let previous = {
            let mut stmt = tx
                .prepare(
                    "SELECT id,name_options_json FROM collection_suggestions
                     WHERE origin='sweep' AND decision='pending'",
                )
                .map_err(err)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(err)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err)?;
            rows
        };
        let mut previous_names = std::collections::HashMap::new();
        for (id, names) in previous {
            let names: Vec<CollectionNameCandidate> =
                serde_json::from_str(&names).unwrap_or_default();
            delete_suggestion(&tx, &id)?;
            previous_names.insert(id, names);
        }
        let mut saved = Vec::new();
        let mut saved_ids = HashSet::new();
        for (id, track, strength, seeds, mut names, members, evidence) in prepared {
            if !saved_ids.insert(id) {
                continue;
            }
            // 同じ顔ぶれをもう採った・断ったなら、出し直さない。
            let decided: Option<String> = tx
                .query_row(
                    "SELECT decision FROM collection_suggestions WHERE id=?1",
                    [id.as_bytes().as_slice()],
                    |r| r.get(0),
                )
                .optional()
                .map_err(err)?;
            if decided.is_some() {
                continue;
            }
            if let Some(previous) = previous_names.get(id.as_bytes().as_slice()) {
                for option in previous.iter().filter(|o| o.source == "llm") {
                    if !names.iter().any(|name| name.name == option.name) {
                        names.push(option.clone());
                    }
                }
            }
            let title = names
                .first()
                .map(|n| n.name.clone())
                .unwrap_or_else(|| "読書コレクション".to_string());
            insert_suggestion(
                &tx,
                &NewSuggestion {
                    id,
                    title: &title,
                    ordered: track == "sequence",
                    track,
                    origin: "sweep",
                    score: strength,
                    explanation: &evidence,
                    names: &names,
                    seeds: &seeds,
                    members: &members,
                    now,
                },
            )?;
            saved.push(id);
        }
        bump(&tx)?;
        tx.commit().map_err(err)?;
        drop(conn);
        let bundles = saved
            .into_iter()
            .map(|id| self.suggestion_view(id))
            .collect::<Result<Vec<_>>>()?;
        Ok(CollectionSweepResult {
            bundles,
            saved_search_suggestions: plan.saved_search_suggestions,
            semantic_used: plan.semantic_used,
            note: plan.note,
        })
    }

    /// 種の作品から、一つの束の候補を作る。採るまでコレクションは増えない。
    pub fn seed_collection_suggestion(
        &self,
        seeds: &[LibraryId],
        limit: Option<i64>,
    ) -> Result<CollectionSuggestion> {
        let corpus = ShelfSuggestions::load(self).map_err(err)?;
        let ids = seeds
            .iter()
            .map(|work| {
                corpus
                    .id_of(*work)
                    .ok_or_else(|| "基準の作品が棚にありません".to_string())
            })
            .collect::<Result<Vec<_>>>()?;
        let plan = seed_suggestion_plan(&corpus, &ids, limit)?;
        let seeds = plan
            .seed_ids
            .iter()
            .filter_map(|id| corpus.work(*id))
            .collect::<Vec<_>>();
        let members = shelf_members(&corpus, plan.members)?;
        drop(corpus);
        let id = LibraryId::now_v7();
        let now = chrono::Utc::now().timestamp_millis();
        let mut conn = self.conn.lock().map_err(|_| "棚が使用中です".to_string())?;
        let tx = conn.transaction().map_err(err)?;
        insert_suggestion(
            &tx,
            &NewSuggestion {
                id,
                title: &plan.proposed_name,
                ordered: plan.collection_kind == "ordered",
                track: "sequence",
                origin: "seed",
                score: plan.score,
                explanation: &plan.evidence_summary,
                names: &plan.name_options,
                seeds: &seeds,
                members: &members,
                now,
            },
        )?;
        bump(&tx)?;
        tx.commit().map_err(err)?;
        drop(conn);
        self.suggestion_view(id)
    }

    /// 候補を画面の形で返す。作品の取得元の鍵は今の棚から引く。
    pub fn suggestion_view(&self, id: LibraryId) -> Result<CollectionSuggestion> {
        let conn = self.reader().map_err(err)?;
        let head = conn
            .query_row(
                "SELECT title,ordered,track,origin,decision,score,rule_version,explanation,
                        name_options_json,created_at,updated_at
                 FROM collection_suggestions WHERE id=?1",
                [id.as_bytes().as_slice()],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, bool>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, f64>(5)?,
                        r.get::<_, String>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, String>(8)?,
                        r.get::<_, i64>(9)?,
                        r.get::<_, i64>(10)?,
                    ))
                },
            )
            .optional()
            .map_err(err)?
            .ok_or_else(|| "コレクションの提案がありません".to_string())?;
        let mut stmt = conn
            .prepare_cached(
                "SELECT m.work_id,
                        coalesce(p.provider,(SELECT x.provider FROM external_identities x
                           WHERE x.entity_id=m.work_id AND x.kind='work' ORDER BY x.provider,x.external_key LIMIT 1)),
                        coalesce(p.external_key,(SELECT x.external_key FROM external_identities x
                           WHERE x.entity_id=m.work_id AND x.kind='work' ORDER BY x.provider,x.external_key LIMIT 1)),
                        m.title_at_proposal,m.author_at_proposal,m.preview_file_id,m.text_length,
                        m.position,m.score,m.selected,m.evidence_json
                 FROM collection_suggestion_members m
                 LEFT JOIN work_current_projection p ON p.work_id=m.work_id
                 WHERE m.suggestion_id=?1 ORDER BY m.position,m.work_id",
            )
            .map_err(err)?;
        let members = stmt
            .query_map([id.as_bytes().as_slice()], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<Vec<u8>>>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, f64>(8)?,
                    r.get::<_, bool>(9)?,
                    r.get::<_, String>(10)?,
                ))
            })
            .map_err(err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(err)?
            .into_iter()
            .map(
                |(
                    work,
                    source,
                    source_id,
                    title,
                    author,
                    preview,
                    length,
                    position,
                    score,
                    selected,
                    evidence,
                )| {
                    let work = uuid(&work)?;
                    Ok(CollectionSuggestionMember {
                        source: source.unwrap_or_default(),
                        source_id: source_id.unwrap_or_default(),
                        download_id: None,
                        work: Some(work.to_string()),
                        title,
                        author_name: author,
                        cover_path: crate::library_entity::fingerprint(preview).map_err(err)?,
                        text_length: length,
                        proposed_position: position,
                        score,
                        selected,
                        evidence: serde_json::from_str::<Vec<CollectionSuggestionEvidence>>(
                            &evidence,
                        )
                        .unwrap_or_default(),
                    })
                },
            )
            .collect::<Result<Vec<_>>>()?;
        let (
            title,
            ordered,
            track,
            origin,
            decision,
            score,
            rule,
            explanation,
            names,
            created,
            updated,
        ) = head;
        Ok(CollectionSuggestion {
            id: id.to_string(),
            proposed_name: title,
            name_options: serde_json::from_str(&names).unwrap_or_default(),
            collection_kind: if ordered { "ordered" } else { "unordered" }.to_string(),
            track,
            origin,
            evidence_summary: explanation.unwrap_or_default(),
            score,
            rule_version: rule,
            state: decision,
            members,
            created_at: time(created),
            updated_at: time(updated),
        })
    }

    /// 候補の一覧。確かなものを先に（続き物、確からしさの高い順）。
    pub fn collection_suggestion_views(
        &self,
        state: Option<&str>,
    ) -> Result<Vec<CollectionSuggestion>> {
        let state = match state.unwrap_or("pending") {
            value @ ("pending" | "accepted" | "rejected" | "all") => value,
            _ => return Err("Invalid suggestion state".to_string()),
        };
        let ids = {
            let conn = self.reader().map_err(err)?;
            let mut stmt = conn
                .prepare(
                    "SELECT id FROM collection_suggestions WHERE ?1='all' OR decision=?1
                     ORDER BY CASE WHEN track='sequence' THEN 0 ELSE 1 END,score DESC,title,id",
                )
                .map_err(err)?;
            let rows = stmt
                .query_map([state], |r| r.get::<_, Vec<u8>>(0))
                .map_err(err)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err)?;
            rows
        };
        ids.iter()
            .map(|id| self.suggestion_view(uuid(id)?))
            .collect()
    }

    fn pending_suggestion(&self, id: &str) -> Result<Option<CollectionSuggestion>> {
        let id = parse_id(id, "提案")?;
        match self.suggestion_view(id) {
            Ok(view) if view.state == "pending" => Ok(Some(view)),
            Ok(_) => Ok(None),
            Err(_) => Ok(None),
        }
    }

    /// 候補を消す（「今は要らない」）。`false` は「その候補が無かった」。
    pub fn dismiss_collection_suggestion_view(&self, id: &str) -> Result<bool> {
        let id = parse_id(id, "提案")?;
        let mut conn = self.conn.lock().map_err(|_| "棚が使用中です".to_string())?;
        let tx = conn.transaction().map_err(err)?;
        let removed = delete_suggestion(&tx, id.as_bytes().as_slice())?;
        if removed > 0 {
            bump(&tx)?;
        }
        tx.commit().map_err(err)?;
        Ok(removed > 0)
    }

    /// 走査で作った未判断の候補を一度に消す。組の採否は残さない。
    pub fn dismiss_swept_collection_suggestions(&self) -> Result<usize> {
        let mut conn = self.conn.lock().map_err(|_| "棚が使用中です".to_string())?;
        let tx = conn.transaction().map_err(err)?;
        let ids = {
            let mut stmt = tx
                .prepare(
                    "SELECT id FROM collection_suggestions WHERE origin='sweep' AND decision='pending'",
                )
                .map_err(err)?;
            let rows = stmt
                .query_map([], |r| r.get::<_, Vec<u8>>(0))
                .map_err(err)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err)?;
            rows
        };
        for id in &ids {
            delete_suggestion(&tx, id)?;
        }
        if !ids.is_empty() {
            bump(&tx)?;
        }
        tx.commit().map_err(err)?;
        Ok(ids.len())
    }

    /// 候補を断る。`member_keys` を渡すと、その作品と基準の作品の組だけを「別の束」と覚える
    /// （旧棚と同じ規則）。`false` は「未判断のその候補が無かった」。
    pub fn reject_collection_suggestion_view(
        &self,
        id: &str,
        member_keys: Option<&[WorkKey]>,
    ) -> Result<bool> {
        let Some(suggestion) = self.pending_suggestion(id)? else {
            return Ok(false);
        };
        let key = |m: &CollectionSuggestionMember| (m.source.clone(), m.source_id.clone());
        let rejected = member_keys.map(|keys| {
            keys.iter()
                .map(|k| (k.source.clone(), k.source_id.clone()))
                .collect::<HashSet<_>>()
        });
        if let Some(keys) = &rejected {
            if keys
                .iter()
                .any(|k| !suggestion.members.iter().any(|m| &key(m) == k))
            {
                return Err("Selected work is not in this suggestion".into());
            }
            if keys.is_empty() || (suggestion.origin == "sweep" && keys.len() < 2) {
                return Err("Select at least two works for this combination".into());
            }
        }
        let suggestion_id = parse_id(&suggestion.id, "提案")?;
        let seeds = {
            let conn = self.reader().map_err(err)?;
            let mut stmt = conn
                .prepare("SELECT work_id FROM collection_suggestion_seeds WHERE suggestion_id=?1 ORDER BY position")
                .map_err(err)?;
            let rows = stmt
                .query_map([suggestion_id.as_bytes().as_slice()], |r| {
                    r.get::<_, Vec<u8>>(0)
                })
                .map_err(err)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err)?;
            rows.iter()
                .map(|w| uuid(w))
                .collect::<Result<HashSet<_>>>()?
        };
        let work_of = |m: &CollectionSuggestionMember| {
            m.work.as_deref().and_then(|w| w.parse::<LibraryId>().ok())
        };
        let mut pairs = Vec::new();
        for seed in suggestion
            .members
            .iter()
            .filter(|m| work_of(m).is_some_and(|w| seeds.contains(&w)))
        {
            if suggestion.origin == "sweep"
                && rejected
                    .as_ref()
                    .is_some_and(|keys| !keys.contains(&key(seed)))
            {
                continue;
            }
            for member in &suggestion.members {
                if key(member) == key(seed) {
                    continue;
                }
                if rejected
                    .as_ref()
                    .is_some_and(|keys| !keys.contains(&key(member)))
                {
                    continue;
                }
                if let (Some(a), Some(b)) = (work_of(seed), work_of(member)) {
                    pairs.push((a, b));
                }
            }
        }
        let now = chrono::Utc::now().timestamp_millis();
        let mut conn = self.conn.lock().map_err(|_| "棚が使用中です".to_string())?;
        let tx = conn.transaction().map_err(err)?;
        for (a, b) in pairs {
            save_pair(&tx, a, b, "reject", &suggestion.rule_version, now)?;
        }
        tx.execute(
            "UPDATE collection_suggestions SET decision='rejected',updated_at=?2,generation=generation+1
             WHERE id=?1 AND decision='pending'",
            params![suggestion_id.as_bytes().as_slice(), now],
        )
        .map_err(err)?;
        bump(&tx)?;
        tx.commit().map_err(err)?;
        Ok(true)
    }

    /// 候補を採り、コレクションを作る。作ったコレクションの ID を返す。
    pub fn accept_collection_suggestion_view(
        &self,
        input: &AcceptCollectionSuggestionInput,
    ) -> Result<LibraryId> {
        let suggestion = self
            .pending_suggestion(&input.suggestion_id)?
            .ok_or_else(|| "Pending collection suggestion not found".to_string())?;
        let selected = match &input.member_keys {
            Some(keys) => {
                let mut seen = HashSet::new();
                let mut selected = Vec::new();
                for key in keys {
                    if !seen.insert((key.source.clone(), key.source_id.clone())) {
                        return Err("The same work was selected twice".into());
                    }
                    let member = suggestion
                        .members
                        .iter()
                        .find(|m| m.source == key.source && m.source_id == key.source_id)
                        .ok_or_else(|| "Selected work is not in this suggestion".to_string())?;
                    selected.push(member);
                }
                selected
            }
            None => suggestion.members.iter().filter(|m| m.selected).collect(),
        };
        if selected.is_empty() {
            return Err("Select at least one work for the collection".to_string());
        }
        let works = selected
            .iter()
            .map(|m| {
                m.work
                    .as_deref()
                    .and_then(|w| w.parse::<LibraryId>().ok())
                    .ok_or_else(|| "候補の作品が不正です".to_string())
            })
            .collect::<Result<Vec<_>>>()?;
        let title = input
            .name
            .clone()
            .unwrap_or_else(|| suggestion.proposed_name.clone());
        let ordered = input
            .collection_kind
            .as_deref()
            .unwrap_or(&suggestion.collection_kind)
            == "ordered";
        let description =
            Some(suggestion.evidence_summary.clone()).filter(|v| !v.trim().is_empty());
        let name_source = accepted_suggestion_name_source(&suggestion, input.name.as_deref());
        let collection = LibraryId::now_v7();
        self.edit_collection(
            LibraryId::now_v7(),
            collection,
            CollectionEdit::Create {
                title: title.clone(),
                description: description.clone(),
                ordered,
                works: works.clone(),
            },
        )
        .map_err(err)?;
        self.edit_collection(
            LibraryId::now_v7(),
            collection,
            CollectionEdit::Describe {
                title,
                description,
                ordered,
                name_source: Some(name_source),
            },
        )
        .map_err(err)?;
        self.edit_collection(
            LibraryId::now_v7(),
            collection,
            CollectionEdit::Look {
                cover_work: works.first().copied(),
                cover_mode: Some("mosaic".into()),
            },
        )
        .map_err(err)?;
        let suggestion_id = parse_id(&suggestion.id, "提案")?;
        let now = chrono::Utc::now().timestamp_millis();
        let mut conn = self.conn.lock().map_err(|_| "棚が使用中です".to_string())?;
        let tx = conn.transaction().map_err(err)?;
        for (index, left) in works.iter().enumerate() {
            for right in &works[index + 1..] {
                save_pair(&tx, *left, *right, "accept", &suggestion.rule_version, now)?;
            }
        }
        let changed = tx
            .execute(
                "UPDATE collection_suggestions SET decision='accepted',updated_at=?2,generation=generation+1
                 WHERE id=?1 AND decision='pending'",
                params![suggestion_id.as_bytes().as_slice(), now],
            )
            .map_err(err)?;
        if changed != 1 {
            return Err("Collection suggestion is no longer pending".to_string());
        }
        bump(&tx)?;
        tx.commit().map_err(err)?;
        Ok(collection)
    }

    /// 候補のモデルの名前の案を、最新の生成結果へ入れ替える。ほかの案は消さない。
    pub fn attach_llm_name_to_suggestion(
        &self,
        id: &str,
        named: &crate::assist::NamedBundle,
        provenance: &crate::assist::AssistProvenance,
    ) -> Result<CollectionSuggestion> {
        let id = parse_id(id, "提案")?;
        let mut view = self.suggestion_view(id)?;
        view.name_options.retain(|option| option.source != "llm");
        view.name_options.push(CollectionNameCandidate {
            source: "llm".to_string(),
            label: "モデルの案".to_string(),
            name: named.name.clone(),
            model_id: Some(provenance.model_id.clone()),
            prompt_version: Some(provenance.prompt_version.clone()),
            created_at: Some(provenance.created_at.clone()),
        });
        let encoded = serde_json::to_string(&view.name_options).map_err(err)?;
        let mut conn = self.conn.lock().map_err(|_| "棚が使用中です".to_string())?;
        let tx = conn.transaction().map_err(err)?;
        tx.execute(
            "UPDATE collection_suggestions SET name_options_json=?2,generation=generation+1 WHERE id=?1",
            params![id.as_bytes().as_slice(), encoded],
        )
        .map_err(err)?;
        bump(&tx)?;
        tx.commit().map_err(err)?;
        Ok(view)
    }

    /// 候補の作品の、命名の材料（本文は含めない）。
    pub fn suggestion_naming_works(&self, id: &str) -> Result<Vec<crate::assist::NamingWork>> {
        let view = self.suggestion_view(parse_id(id, "提案")?)?;
        let corpus = ShelfSuggestions::load(self).map_err(err)?;
        let ids = view
            .members
            .iter()
            .filter_map(|m| m.work.as_deref()?.parse::<LibraryId>().ok())
            .filter_map(|w| corpus.id_of(w))
            .collect::<Vec<_>>();
        naming_works_in(&*corpus_conn(&corpus)?, &ids)
    }

    /// コレクションの作品の、命名の材料（本文は含めない）。
    pub fn collection_naming_works_on_shelf(
        &self,
        collection: LibraryId,
    ) -> Result<Vec<crate::assist::NamingWork>> {
        let corpus = ShelfSuggestions::load(self).map_err(err)?;
        let ids = collection_member_ids(&corpus, collection)?;
        naming_works_in(&*corpus_conn(&corpus)?, &ids)
    }

    /// 試し書きの材料（束らしい作品をいくつか）。
    pub fn sample_naming_works_on_shelf(&self) -> Result<Vec<crate::assist::NamingWork>> {
        let corpus = ShelfSuggestions::load(self).map_err(err)?;
        let conn = corpus_conn(&corpus)?;
        let ids = sample_bundle_like_ids_in(&conn)?;
        naming_works_in(&conn, &ids)
    }

    /// コレクションの名前と説明の案。今の名前も案として残す。保存はしない。
    pub fn collection_name_proposals_on_shelf(
        &self,
        collection: LibraryId,
    ) -> Result<Vec<CollectionNameCandidate>> {
        let (name, source) = {
            let conn = self.reader().map_err(err)?;
            conn.query_row(
                "SELECT title,CAST(coalesce(json_extract(appearance_json,'$.name_source'),'manual') AS TEXT)
                 FROM collections WHERE id=?1",
                [collection.as_bytes().as_slice()],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(err)?
            .ok_or_else(|| "Collection not found".to_string())?
        };
        let corpus = ShelfSuggestions::load(self).map_err(err)?;
        let ids = collection_member_ids(&corpus, collection)?;
        name_proposals_in(&*corpus_conn(&corpus)?, &ids, &name, &source)
    }

    /// コレクションにあとから入れるとよさそうな作品。保存はしない。
    pub fn collection_additions_on_shelf(
        &self,
        collection: LibraryId,
    ) -> Result<CollectionAdditionResult> {
        let corpus = ShelfSuggestions::load(self).map_err(err)?;
        let mut result = collection_additions(&corpus, &collection.to_string())?;
        for candidate in &mut result.candidates {
            let work = corpus
                .work(candidate.download_id)
                .ok_or_else(|| "候補の作品が棚にありません".to_string())?;
            candidate.work = Some(work.to_string());
            candidate.download_id = 0;
        }
        Ok(result)
    }
}

/// 根拠から関係の状態を決め直す（旧棚の移行と同じ規則）。
const LINK_STATUS: &str = "UPDATE work_links SET status=CASE WHEN EXISTS(SELECT 1 FROM link_evidence WHERE link_id=?1 AND decision='accepted') AND NOT EXISTS(SELECT 1 FROM link_evidence WHERE link_id=?1 AND decision='rejected') THEN 'accepted' WHEN EXISTS(SELECT 1 FROM link_evidence WHERE link_id=?1 AND decision='rejected') AND NOT EXISTS(SELECT 1 FROM link_evidence WHERE link_id=?1 AND decision='accepted') THEN 'rejected' ELSE 'observed' END WHERE id=?1";

impl Library {
    /// 作品の今の版の本文・説明から本文リンクを取り直す（旧棚の `refresh_work_links` と同じ規則）。
    ///
    /// 取り直すのは本文から拾った未判断の根拠（`content_link` の `observed`）だけ。利用者が採った・
    /// 断った根拠と、ほかの種類の根拠は残す。根拠の無くなった関係は消す。返すのは拾ったリンクの数。
    pub fn refresh_shelf_work_links(&self, work: LibraryId) -> Result<usize> {
        let Some(doc) = self.search_document(work).map_err(err)? else {
            return Ok(0);
        };
        let snapshot: Vec<u8> = {
            let conn = self.reader().map_err(err)?;
            conn.query_row(
                "SELECT snapshot_id FROM work_current_projection WHERE work_id=?1",
                [work.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .map_err(err)?
        };
        let scope = crate::library::ResourceScope::Snapshot(uuid(&snapshot)?);
        let mut text = match self.reading_document(work, scope, &doc.provider) {
            Ok((document, _)) => crate::library_view::render_html(&document).map_err(err)?,
            Err(crate::library::Error::ResourceUnavailable) => String::new(),
            Err(error) => return Err(err(error)),
        };
        for part in [&doc.body, &doc.excerpt] {
            text.push('\n');
            text.push_str(part);
        }
        let found = crate::database::queries::extract_work_link_evidence(
            &text,
            &doc.provider,
            &doc.external_key,
        );
        let now = chrono::Utc::now().timestamp_millis();
        let mut conn = self.conn.lock().map_err(|_| "棚が使用中です".to_string())?;
        let tx = conn.transaction().map_err(err)?;
        let stale = {
            let mut stmt = tx
                .prepare(
                    "SELECT e.id,e.link_id FROM link_evidence e JOIN work_links l ON l.id=e.link_id
                     WHERE l.from_work_id=?1 AND e.kind='content_link' AND e.decision='observed'",
                )
                .map_err(err)?;
            let rows = stmt
                .query_map([work.as_bytes().as_slice()], |r| {
                    Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?))
                })
                .map_err(err)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(err)?;
            rows
        };
        let mut touched = HashSet::new();
        for (evidence, link) in &stale {
            tx.execute("DELETE FROM link_evidence WHERE id=?1", [evidence])
                .map_err(err)?;
            touched.insert(link.clone());
        }
        for link in &found {
            let to = Library::relation_entity(
                &tx,
                &link.to_source,
                crate::library::EntityKind::Work,
                &link.to_source_id,
                now,
            )
            .map_err(err)?;
            if to == work {
                continue;
            }
            let existing: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT id FROM work_links WHERE from_work_id=?1 AND to_work_id=?2 AND relation=?3",
                    params![
                        work.as_bytes().as_slice(),
                        to.as_bytes().as_slice(),
                        link.relation_type
                    ],
                    |r| r.get(0),
                )
                .optional()
                .map_err(err)?;
            let id = match existing {
                Some(id) => id,
                None => {
                    let id = LibraryId::now_v7().as_bytes().to_vec();
                    tx.execute(
                        "INSERT INTO work_links VALUES(?1,?2,?3,?4,'observed',?5,?5)",
                        params![
                            id,
                            work.as_bytes().as_slice(),
                            to.as_bytes().as_slice(),
                            link.relation_type,
                            now
                        ],
                    )
                    .map_err(err)?;
                    id
                }
            };
            tx.execute(
                "INSERT INTO link_evidence VALUES(?1,?2,'content_link','observed',?3,?4,?5,?6)",
                params![
                    LibraryId::now_v7().as_bytes().as_slice(),
                    id,
                    link.anchor_text,
                    link.context_text,
                    link.confidence.clamp(0.0, 1.0),
                    now
                ],
            )
            .map_err(err)?;
            tx.execute(
                "UPDATE work_links SET updated_at=?2 WHERE id=?1",
                params![id, now],
            )
            .map_err(err)?;
            touched.insert(id);
        }
        for link in &touched {
            tx.execute(
                "DELETE FROM work_links WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM link_evidence WHERE link_id=?1)",
                [link],
            )
            .map_err(err)?;
            tx.execute(LINK_STATUS, [link]).map_err(err)?;
        }
        if !touched.is_empty() {
            bump(&tx)?;
        }
        tx.commit().map_err(err)?;
        Ok(found.len())
    }
}

fn corpus_conn<'a>(
    corpus: &'a ShelfSuggestions<'_>,
) -> Result<crate::database::queries::Corpus<'a>> {
    use crate::database::queries::SuggestionSource;
    corpus.corpus()
}

fn collection_member_ids(corpus: &ShelfSuggestions<'_>, collection: LibraryId) -> Result<Vec<i64>> {
    let conn = corpus_conn(corpus)?;
    let mut stmt = conn
        .prepare(
            "SELECT download_id FROM work_collection_members
             WHERE collection_id=?1 AND download_id IS NOT NULL ORDER BY position",
        )
        .map_err(err)?;
    let rows = stmt
        .query_map([collection.to_string()], |r| r.get::<_, i64>(0))
        .map_err(err)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(err);
    rows
}

/// 組の採否を覚える（左が小さい ID）。同じ組は新しい判断で上書きする。
fn save_pair(
    tx: &Transaction<'_>,
    a: LibraryId,
    b: LibraryId,
    decision: &str,
    rule: &str,
    now: i64,
) -> Result<()> {
    if a == b {
        return Ok(());
    }
    let (left, right) = if a < b { (a, b) } else { (b, a) };
    tx.execute(
        "INSERT INTO collection_pair_feedback VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(left_work_id,right_work_id) DO UPDATE SET decision=excluded.decision,
             rule_version=excluded.rule_version,updated_at=excluded.updated_at",
        params![
            left.as_bytes().as_slice(),
            right.as_bytes().as_slice(),
            decision,
            rule,
            now
        ],
    )
    .map_err(err)?;
    Ok(())
}

#[cfg(test)]
#[path = "library_suggestions_tests.rs"]
mod tests;
