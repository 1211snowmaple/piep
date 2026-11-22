//! AI の手伝いに渡す材料と、その結果（採ったタグ・覚え書き）を棚で持つ。
//!
//! 手伝いの材料（`assist_inputs`）とタグの案（`tag_recommendations`）を、棚の今の版から作る。タグは
//! 取得元が付けたタグ（今の版のタグ）と、自分で付けたタグ（出どころが「手動」と「モデルの案を
//! 採った」）を名前で合わせる。本文を含む材料を作る関数は名前に `with_body` と付ける。
//!
//! 覚え書き（あらすじ・作風のメモ・前回のあらすじ）は作り直せるものなので、版の記録ではなく
//! 一つの表に置き換えで持つ。作品は棚の ID（文字列）で指す。

use crate::assist::{
    AssistNote, AssistProvenance, ShelfTagCount, ShelfTagPair, ShelfTagPairs, ShelfTagProfile,
    SplitWork, WorkFacts,
};
use crate::database::collection_rules::is_informative_tag;
use crate::database::queries::{AiNote, RelatedTagCandidate, TaggedName};
use crate::library::{
    EntityKind, Error, Library, LibraryId, ResourceScope, Result, TagProvenance, TagUpdate, UserTag,
};
use rusqlite::{named_params, params, OptionalExtension};
use std::collections::{BTreeSet, HashMap};

/// タグの候補として見せる語の上限。
const TAG_VOCABULARY_LIMIT: i64 = 400;
/// 一緒に付くタグの候補の数。
const MAX_RELATED_TAG_CANDIDATES: usize = 5;

/// 今の版の作品に付いたタグ（取得元のタグと自分で付けたタグ）を名前で。`:owner` は利用者。
const WORK_TAGS: &str = "work_tags(work_id,name) AS MATERIALIZED (
    SELECT p.work_id,t.name FROM work_current_projection p
    JOIN snapshot_tags st ON st.snapshot_id=p.snapshot_id JOIN tags t ON t.id=st.tag_id
    UNION
    SELECT wt.work_id,u.name FROM work_user_tags wt
    JOIN user_tags u ON u.user_id=wt.user_id AND u.id=wt.tag_id
    JOIN work_current_projection p ON p.work_id=wt.work_id
    WHERE wt.user_id=:owner)";

fn assisted_tag_name(value: &str) -> Option<&str> {
    let name = value.trim();
    (!name.is_empty() && name.chars().count() <= 100).then_some(name)
}

fn provenance_label(provenance: &str) -> &'static str {
    if provenance == "accepted_model" {
        "llm"
    } else {
        "manual"
    }
}

impl Library {
    fn owner(&self) -> Vec<u8> {
        self.id.as_bytes().to_vec()
    }

    /// 作品ひとつぶんの、本文を含まない材料（題名・作者・タグ・概要）。
    pub fn assist_work_facts(&self, work: LibraryId) -> Result<WorkFacts> {
        let card = self.work_card(work)?;
        let mut tags = card.tags.clone();
        tags.sort_by_key(|tag| tag.to_lowercase());
        Ok(WorkFacts {
            title: card.title,
            author_name: card.author_name,
            tags,
            excerpt: card.excerpt.filter(|value| !value.trim().is_empty()),
        })
    }

    /// あらすじを作るための、**本文を含む**材料。今読む版（編集があれば編集版）の本文。
    pub fn assist_work_facts_with_body(&self, work: LibraryId) -> Result<(WorkFacts, String)> {
        let card = self.work_card(work)?;
        let scope = match card.edit_id {
            Some(edit) => ResourceScope::Edit(edit),
            None => ResourceScope::Snapshot(card.snapshot_id),
        };
        let body = self.work_text(work, scope)?;
        Ok((self.assist_work_facts(work)?, body))
    }

    /// 棚でよく使われているタグの語彙。モデルに**この中から選ばせる**。
    pub fn assist_tag_vocabulary(&self) -> Result<Vec<String>> {
        let conn = self.reader()?;
        let mut statement = conn.prepare(&format!(
            "WITH {WORK_TAGS} SELECT name FROM work_tags GROUP BY name
             ORDER BY count(*) DESC,name COLLATE NOCASE ASC LIMIT :limit"
        ))?;
        let names = statement
            .query_map(
                named_params! {":owner": self.owner(), ":limit": TAG_VOCABULARY_LIMIT},
                |r| r.get::<_, String>(0),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(names
            .into_iter()
            .filter(|name| is_informative_tag(name))
            .collect())
    }

    /// この作者の作品の材料（新しい順）。本文・概要は含まない。
    pub fn assist_author_facts(
        &self,
        provider: &str,
        key: &str,
    ) -> Result<(String, Vec<WorkFacts>)> {
        let head = self
            .entity_head(EntityKind::Person, provider, key)?
            .ok_or_else(|| Error::Integrity("この作者の作品がありません".into()))?;
        let works: Vec<LibraryId> = {
            let conn = self.reader()?;
            let mut statement = conn.prepare_cached(
                "SELECT p.work_id FROM work_current_projection p
                 WHERE p.snapshot_id IN (SELECT snapshot_id FROM snapshot_people WHERE person_id=?1)
                 ORDER BY coalesce(p.published_at,p.saved_at) DESC,p.work_id DESC",
            )?;
            let rows = statement
                .query_map([head.id.as_bytes().as_slice()], |r| r.get::<_, Vec<u8>>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows.iter()
                .map(|id| crate::library_entity::id(id.clone()))
                .collect::<Result<_>>()?
        };
        let facts: Vec<WorkFacts> = self
            .work_cards(&works)?
            .into_iter()
            .flatten()
            .map(|card| {
                let mut tags = card.tags;
                tags.sort_by_key(|tag| tag.to_lowercase());
                WorkFacts {
                    title: card.title,
                    author_name: card.author_name,
                    tags,
                    excerpt: None,
                }
            })
            .collect();
        if facts.is_empty() {
            return Err(Error::Integrity("この作者の作品がありません".into()));
        }
        let author = facts
            .first()
            .map(|value| value.author_name.clone())
            .unwrap_or(head.name);
        Ok((author, facts))
    }

    /// コレクションの回を、分け方の案を出せる形で並べる。棚に無い回は飛ばす（位置は回の順番）。
    pub fn assist_collection_facts(&self, collection: LibraryId) -> Result<Vec<SplitWork>> {
        let detail = self.collection_detail(collection)?;
        let mut facts = Vec::new();
        for (position, member) in detail.members.iter().enumerate() {
            let Some(work) = &member.work else {
                continue;
            };
            facts.push(SplitWork {
                position: position as i64,
                facts: self.assist_work_facts(work.id)?,
            });
        }
        Ok(facts)
    }

    /// 対象の作品のタグと、棚で取得元が一緒に付けたタグの併用統計。
    ///
    /// 対象の作品と、同じ作者の作品は数えない。対象の種は自分で付けたタグも含め、数える側は
    /// 取得元のタグだけにする（採ったタグが統計を自己増幅しないように）。
    pub fn related_tag_candidates(&self, work: LibraryId) -> Result<Vec<RelatedTagCandidate>> {
        let conn = self.reader()?;
        let key = work.as_bytes().to_vec();
        let seed_count: i64 = conn.query_row(
            "SELECT count(*) FROM (
               SELECT t.name FROM work_current_projection p
               JOIN snapshot_tags st ON st.snapshot_id=p.snapshot_id JOIN tags t ON t.id=st.tag_id
               WHERE p.work_id=:work
               UNION
               SELECT u.name FROM work_user_tags wt JOIN user_tags u ON u.user_id=wt.user_id AND u.id=wt.tag_id
               WHERE wt.work_id=:work AND wt.user_id=:owner)",
            named_params! {":work": key, ":owner": self.owner()},
            |r| r.get(0),
        )?;
        if seed_count == 0 {
            return Ok(Vec::new());
        }
        let mut statement = conn.prepare(
            "WITH target AS MATERIALIZED (
                 SELECT work_id,snapshot_id FROM work_current_projection WHERE work_id=:work
             ),
             target_people AS MATERIALIZED (
                 SELECT DISTINCT sp.person_id FROM snapshot_people sp JOIN target t ON t.snapshot_id=sp.snapshot_id
                 WHERE sp.role IN ('author','creator')
             ),
             target_tags AS MATERIALIZED (
                 SELECT tg.name FROM snapshot_tags st JOIN target t ON t.snapshot_id=st.snapshot_id
                 JOIN tags tg ON tg.id=st.tag_id
                 UNION
                 SELECT u.name FROM work_user_tags wt JOIN user_tags u ON u.user_id=wt.user_id AND u.id=wt.tag_id
                 WHERE wt.work_id=:work AND wt.user_id=:owner
             ),
             eligible AS MATERIALIZED (
                 SELECT p.work_id AS id,p.snapshot_id FROM work_current_projection p
                 WHERE p.work_id<>:work AND NOT EXISTS (
                     SELECT 1 FROM snapshot_people sp WHERE sp.snapshot_id=p.snapshot_id
                       AND sp.person_id IN (SELECT person_id FROM target_people))
             ),
             origin AS MATERIALIZED (
                 SELECT DISTINCT e.id AS work_id,tg.name FROM eligible e
                 JOIN snapshot_tags st ON st.snapshot_id=e.snapshot_id JOIN tags tg ON tg.id=st.tag_id
             ),
             seed_works AS MATERIALIZED (
                 SELECT DISTINCT o.work_id FROM target_tags s JOIN origin o ON o.name=s.name
             ),
             seed_df AS (
                 SELECT s.name,count(DISTINCT o.work_id) AS df FROM target_tags s
                 JOIN origin o ON o.name=s.name GROUP BY s.name
             ),
             candidate_df AS (
                 SELECT name,count(DISTINCT work_id) AS df FROM origin GROUP BY name
             ),
             pair_stats AS (
                 SELECT seed.name AS seed_name,c.name AS candidate_name,count(DISTINCT c.work_id) AS co_count
                 FROM target_tags seed JOIN origin sa ON sa.name=seed.name
                 JOIN origin c ON c.work_id=sa.work_id
                 WHERE NOT EXISTS (SELECT 1 FROM target_tags x WHERE x.name=c.name)
                 GROUP BY seed.name,c.name
             ),
             candidate_common AS (
                 SELECT o.name AS candidate_name,count(DISTINCT o.work_id) AS co_work_count
                 FROM origin o JOIN seed_works sw ON sw.work_id=o.work_id
                 WHERE NOT EXISTS (SELECT 1 FROM target_tags x WHERE x.name=o.name)
                 GROUP BY o.name HAVING count(DISTINCT o.work_id)>=2
             )
             SELECT p.candidate_name,sd.df,cd.df,p.co_count,(SELECT count(*) FROM seed_works),cc.co_work_count
             FROM pair_stats p
             JOIN seed_df sd ON sd.name=p.seed_name
             JOIN candidate_df cd ON cd.name=p.candidate_name
             JOIN candidate_common cc ON cc.candidate_name=p.candidate_name
             ORDER BY p.candidate_name,p.seed_name",
        )?;
        let rows = statement
            .query_map(named_params! {":work": key, ":owner": self.owner()}, |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut scores: HashMap<String, (RelatedTagCandidate, f64)> = HashMap::new();
        for (name, seed_df, candidate_df, co_count, matching, co_works) in rows {
            if seed_df <= 0 || candidate_df <= 0 || co_count <= 0 {
                continue;
            }
            let cosine = co_count as f64 / ((seed_df as f64) * (candidate_df as f64)).sqrt();
            let entry = scores.entry(name.clone()).or_insert_with(|| {
                (
                    RelatedTagCandidate {
                        tag: name.clone(),
                        reason: format!(
                            "同じタグを持つ{matching}作品のうち{co_works}作品に一緒に付いています"
                        ),
                        matching_work_count: matching,
                        cooccurring_work_count: co_works,
                    },
                    0.0,
                )
            });
            entry.1 += cosine;
        }
        let mut candidates: Vec<(RelatedTagCandidate, f64)> = scores
            .into_values()
            .filter(|(candidate, _)| {
                let trimmed = candidate.tag.trim();
                !trimmed.is_empty() && trimmed.chars().count() <= 100 && trimmed == candidate.tag
            })
            .map(|(candidate, score)| (candidate, score / seed_count as f64))
            .collect();
        candidates.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| {
                    right
                        .0
                        .cooccurring_work_count
                        .cmp(&left.0.cooccurring_work_count)
                })
                .then_with(|| left.0.tag.cmp(&right.0.tag))
        });
        candidates.truncate(MAX_RELATED_TAG_CANDIDATES);
        Ok(candidates
            .into_iter()
            .map(|(candidate, _)| candidate)
            .collect())
    }

    /// 棚のタグを部分一致で引く（作品数の多い順）。モデルが表記揺れを確かめる道具。
    pub fn assist_tags_containing(&self, needle: &str, limit: usize) -> Result<Vec<ShelfTagCount>> {
        let needle = needle.trim();
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.reader()?;
        let mut statement = conn.prepare(&format!(
            "WITH {WORK_TAGS} SELECT name,count(DISTINCT work_id) AS works FROM work_tags
             WHERE instr(lower(name),lower(:needle))>0
             GROUP BY name ORDER BY works DESC,name COLLATE NOCASE ASC LIMIT :limit"
        ))?;
        let rows = statement
            .query_map(
                named_params! {":owner": self.owner(), ":needle": needle, ":limit": (limit * 2) as i64},
                |r| Ok(ShelfTagCount { name: r.get(0)?, works: r.get(1)? }),
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .filter(|tag| is_informative_tag(&tag.name))
            .take(limit)
            .collect())
    }

    /// 棚に実在するタグ名だけを返す（完全一致）。取得元のタグか自分で付けたタグ。
    pub fn assist_existing_tags(&self, names: &[String]) -> Result<Vec<String>> {
        let conn = self.reader()?;
        let mut statement = conn.prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM tags WHERE name=?1)
                 OR EXISTS(SELECT 1 FROM user_tags WHERE user_id=?2 AND name=?1)",
        )?;
        let mut found = Vec::new();
        for name in names {
            if statement.query_row(params![name, self.owner()], |r| r.get::<_, bool>(0))? {
                found.push(name.clone());
            }
        }
        Ok(found)
    }

    /// タグの条件だけで絞ったときに一覧に出る作品の数（一覧と同じ絞り込み）。
    pub fn assist_count_works(
        &self,
        include: &[String],
        or_mode: bool,
        exclude: &[String],
    ) -> Result<i64> {
        let filter = crate::library::WorkFilter {
            tags: include.to_vec(),
            any_tag: or_mode,
            excluded_tags: exclude.to_vec(),
            ..crate::library::WorkFilter::default()
        };
        Ok(self.count_works(filter)? as i64)
    }

    /// 一つのタグと、ほかのタグそれぞれが同じ作品にどれだけ付いているか。棚に無ければ None。
    pub fn assist_tag_pairs(&self, name: &str, others: &[String]) -> Result<Option<ShelfTagPairs>> {
        let conn = self.reader()?;
        let owner = self.owner();
        let works_of = |tag: &str| -> rusqlite::Result<i64> {
            conn.query_row(
                &format!("WITH {WORK_TAGS} SELECT count(DISTINCT work_id) FROM work_tags WHERE name=:name"),
                named_params! {":owner": owner, ":name": tag},
                |r| r.get(0),
            )
        };
        let works = works_of(name)?;
        if works == 0 {
            return Ok(None);
        }
        let total: i64 = conn.query_row(
            &format!("WITH {WORK_TAGS} SELECT count(DISTINCT work_id) FROM work_tags"),
            named_params! {":owner": owner},
            |r| r.get(0),
        )?;
        let mut pairs = Vec::new();
        for other in others.iter().filter(|other| other.as_str() != name) {
            let other_works = works_of(other)?;
            if other_works == 0 {
                continue;
            }
            let together: i64 = conn.query_row(
                &format!(
                    "WITH {WORK_TAGS} SELECT count(DISTINCT a.work_id) FROM work_tags a
                     JOIN work_tags b ON b.work_id=a.work_id WHERE a.name=:name AND b.name=:other"
                ),
                named_params! {":owner": owner, ":name": name, ":other": other},
                |r| r.get(0),
            )?;
            pairs.push(ShelfTagPair {
                name: other.clone(),
                works: other_works,
                together,
            });
        }
        Ok(Some(ShelfTagPairs {
            total,
            works,
            others: pairs,
        }))
    }

    /// 一つのタグが棚でどう使われているか（一緒に付くタグと作品の題名）。棚に無ければ None。
    pub fn assist_tag_profile(&self, name: &str) -> Result<Option<ShelfTagProfile>> {
        let conn = self.reader()?;
        let owner = self.owner();
        let works: i64 = conn.query_row(
            &format!(
                "WITH {WORK_TAGS} SELECT count(DISTINCT work_id) FROM work_tags WHERE name=:name"
            ),
            named_params! {":owner": owner, ":name": name},
            |r| r.get(0),
        )?;
        if works == 0 {
            return Ok(None);
        }
        let related: Vec<ShelfTagCount> = conn
            .prepare(&format!(
                "WITH {WORK_TAGS} SELECT b.name,count(DISTINCT b.work_id) AS works FROM work_tags a
                 JOIN work_tags b ON b.work_id=a.work_id AND b.name<>a.name
                 WHERE a.name=:name GROUP BY b.name ORDER BY works DESC,b.name COLLATE NOCASE ASC LIMIT 30"
            ))?
            .query_map(named_params! {":owner": owner, ":name": name}, |r| {
                Ok(ShelfTagCount { name: r.get(0)?, works: r.get(1)? })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|tag| is_informative_tag(&tag.name))
            .take(12)
            .collect();
        let titles: Vec<String> = conn
            .prepare(&format!(
                "WITH {WORK_TAGS} SELECT p.title FROM work_tags w JOIN work_current_projection p ON p.work_id=w.work_id
                 WHERE w.name=:name ORDER BY p.saved_at DESC,p.work_id DESC LIMIT 8"
            ))?
            .query_map(named_params! {":owner": owner, ":name": name}, |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Some(ShelfTagProfile {
            name: name.to_string(),
            works,
            related,
            titles,
        }))
    }

    /// この作品のタグを出どころ付きで（取得元のタグが先）。
    pub fn work_tags_with_source(&self, work: LibraryId) -> Result<Vec<TaggedName>> {
        let conn = self.reader()?;
        let mut statement = conn.prepare_cached(
            "SELECT name,source FROM (
               SELECT t.name AS name,'origin' AS source FROM work_current_projection p
               JOIN snapshot_tags st ON st.snapshot_id=p.snapshot_id JOIN tags t ON t.id=st.tag_id
               WHERE p.work_id=?1
               UNION
               SELECT u.name,wt.provenance FROM work_user_tags wt
               JOIN user_tags u ON u.user_id=wt.user_id AND u.id=wt.tag_id
               WHERE wt.work_id=?1 AND wt.user_id=?2)
             ORDER BY source<>'origin',name COLLATE NOCASE,name",
        )?;
        let rows = statement
            .query_map(params![work.as_bytes().as_slice(), self.owner()], |r| {
                let source: String = r.get(1)?;
                Ok(TaggedName {
                    name: r.get(0)?,
                    source: if source == "origin" {
                        source
                    } else {
                        provenance_label(&source).to_string()
                    },
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        // 取得元にも自分にもある名前は、取得元の一件だけにする（同じタグを二度付けない）。
        let mut seen = BTreeSet::new();
        Ok(rows
            .into_iter()
            .filter(|tag| seen.insert(tag.name.clone()))
            .collect())
    }

    /// 自分のタグを足す。`existing_only` なら棚にある名前だけ（一緒に付くタグの候補）。
    /// すでに付いている名前（取得元のタグも）は触らない。足した名前を返す。
    pub fn add_user_tags(
        &self,
        work: LibraryId,
        names: &[String],
        provenance: TagProvenance,
        existing_only: bool,
    ) -> Result<Vec<String>> {
        let present: BTreeSet<String> = self
            .work_tags_with_source(work)?
            .into_iter()
            .map(|tag| tag.name)
            .collect();
        let mut added = Vec::new();
        for name in names {
            let Some(name) = assisted_tag_name(name) else {
                if existing_only {
                    return Err(Error::Integrity("採用するタグの名前が不正です".into()));
                }
                continue;
            };
            if existing_only && self.assist_existing_tags(&[name.to_string()])?.is_empty() {
                return Err(Error::Integrity(
                    "棚に無いタグは、一緒に付くタグの候補から追加できません".into(),
                ));
            }
            if !present.contains(name) && !added.iter().any(|known: &String| known == name) {
                added.push(name.to_string());
            }
        }
        if added.is_empty() {
            return Ok(added);
        }
        let current = self.user_tags(work, self.id)?;
        let mut tags = current.tags;
        tags.extend(added.iter().map(|name| UserTag {
            name: name.clone(),
            provenance,
        }));
        self.replace_user_tags(TagUpdate {
            request_id: LibraryId::now_v7(),
            work,
            user: self.id,
            expected_generation: current.generation,
            tags,
        })?;
        Ok(added)
    }

    /// 自分のタグのうち、その出どころのものだけを外す（取得元のタグは外せない）。
    pub fn remove_user_tag(
        &self,
        work: LibraryId,
        name: &str,
        provenance: TagProvenance,
    ) -> Result<bool> {
        let current = self.user_tags(work, self.id)?;
        let before = current.tags.len();
        let tags: Vec<UserTag> = current
            .tags
            .into_iter()
            .filter(|tag| !(tag.name == name && tag.provenance == provenance))
            .collect();
        if tags.len() == before {
            return Ok(false);
        }
        self.replace_user_tags(TagUpdate {
            request_id: LibraryId::now_v7(),
            work,
            user: self.id,
            expected_generation: current.generation,
            tags,
        })?;
        Ok(true)
    }

    /// 覚え書きを置き換えで保存する。
    pub fn save_ai_note(
        &self,
        subject_type: &str,
        subject_key: &str,
        note_kind: &str,
        note: &AssistNote,
        provenance: &AssistProvenance,
    ) -> Result<()> {
        let claims = note
            .claims
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        conn.execute(
            "INSERT INTO ai_notes(subject_type,subject_key,note_kind,text,source_tail,claims_json,model_id,
                                  feature_id,prompt_version,input_fingerprint,config_fingerprint,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
             ON CONFLICT(subject_type,subject_key,note_kind) DO UPDATE SET text=excluded.text,
               source_tail=excluded.source_tail,claims_json=excluded.claims_json,model_id=excluded.model_id,
               feature_id=excluded.feature_id,prompt_version=excluded.prompt_version,
               input_fingerprint=excluded.input_fingerprint,config_fingerprint=excluded.config_fingerprint,
               created_at=excluded.created_at",
            params![
                subject_type,
                subject_key,
                note_kind,
                note.text,
                note.source_tail,
                claims,
                provenance.model_id,
                provenance.feature_id,
                provenance.prompt_version,
                provenance.input_fingerprint,
                provenance.config_fingerprint,
                provenance.created_at,
            ],
        )?;
        Ok(())
    }

    /// 保存してある覚え書き。材料が変わっていれば `input_stale`、文の作り方が変わっていれば
    /// `prompt_stale`。モデルの設定の違いは呼ぶ側が比べる。
    pub fn load_ai_note(
        &self,
        subject_type: &str,
        subject_key: &str,
        note_kind: &str,
    ) -> Result<Option<AiNote>> {
        let note = self
            .reader()?
            .query_row(
                "SELECT text,model_id,feature_id,prompt_version,input_fingerprint,config_fingerprint,
                        created_at,source_tail,claims_json FROM ai_notes
                 WHERE subject_type=?1 AND subject_key=?2 AND note_kind=?3",
                params![subject_type, subject_key, note_kind],
                |r| {
                    Ok(AiNote {
                        text: r.get(0)?,
                        model_id: r.get(1)?,
                        feature_id: r.get(2)?,
                        prompt_version: r.get(3)?,
                        input_fingerprint: r.get(4)?,
                        config_fingerprint: r.get(5)?,
                        created_at: r.get(6)?,
                        source_tail: r.get(7)?,
                        claims: r
                            .get::<_, Option<String>>(8)?
                            .and_then(|json| serde_json::from_str(&json).ok()),
                        prompt_stale: false,
                        input_stale: false,
                        config_stale: None,
                    })
                },
            )
            .optional()?;
        let Some(mut note) = note else {
            return Ok(None);
        };
        note.prompt_stale =
            note.prompt_version != crate::assist::current_prompt_version(&note.feature_id);
        if let Some(current) =
            self.note_input_fingerprint(subject_type, subject_key, note_kind, &note.feature_id)?
        {
            note.input_stale = current != note.input_fingerprint;
        }
        Ok(Some(note))
    }

    /// 今の材料から作ると、どの指紋になるか（材料が変わったかを見る）。読めなければ None。
    fn note_input_fingerprint(
        &self,
        subject_type: &str,
        subject_key: &str,
        note_kind: &str,
        feature_id: &str,
    ) -> Result<Option<String>> {
        let work = |key: &str| key.parse::<LibraryId>().ok();
        let input = match (subject_type, note_kind, feature_id) {
            ("work", "synopsis", crate::assist::FEATURE_WORK_SYNOPSIS) => {
                let Some(id) = work(subject_key) else {
                    return Ok(None);
                };
                let Ok((facts, body)) = self.assist_work_facts_with_body(id) else {
                    return Ok(None);
                };
                serde_json::json!({ "work": facts, "body": body })
            }
            ("person", "style", crate::assist::FEATURE_AUTHOR_STYLE) => {
                let Some((provider, key)) = subject_key.split_once(':') else {
                    return Ok(None);
                };
                let Ok((author, works)) = self.assist_author_facts(provider, key) else {
                    return Ok(None);
                };
                serde_json::json!({ "author": author, "works": works })
            }
            ("work", "recap", crate::assist::FEATURE_READER_RECAP) => {
                let Some((current, previous)) = subject_key.split_once(':') else {
                    return Ok(None);
                };
                let (Some(_), Some(previous_id)) = (work(current), work(previous)) else {
                    return Ok(None);
                };
                let Ok((facts, body)) = self.assist_work_facts_with_body(previous_id) else {
                    return Ok(None);
                };
                serde_json::json!({
                    "previousWork": previous,
                    "currentWork": current,
                    "work": facts,
                    "body": body
                })
            }
            _ => return Ok(None),
        };
        crate::assist::generation_input_fingerprint(feature_id, &input)
            .map(Some)
            .map_err(Error::Integrity)
    }

    /// 覚え書きを消す。消したら true。
    pub fn delete_ai_note(
        &self,
        subject_type: &str,
        subject_key: &str,
        note_kind: &str,
    ) -> Result<bool> {
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        Ok(conn.execute(
            "DELETE FROM ai_notes WHERE subject_type=?1 AND subject_key=?2 AND note_kind=?3",
            params![subject_type, subject_key, note_kind],
        )? > 0)
    }

    /// その話を読む前に作った「前回のあらすじ」（`読む話:前の話` の `読む話` がこの話）のうち新しいもの。
    pub fn recap_memory_before(&self, chapter: LibraryId) -> Result<Option<String>> {
        Ok(self
            .reader()?
            .query_row(
                "SELECT text FROM ai_notes WHERE subject_type='work' AND note_kind='recap'
                   AND subject_key LIKE ?1 || ':%' ORDER BY created_at DESC LIMIT 1",
                [chapter.to_string()],
                |r| r.get(0),
            )
            .optional()?)
    }
}

#[cfg(test)]
#[path = "library_assist_tests.rs"]
mod tests;
