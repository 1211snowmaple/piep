//! 本文の検索の一覧に付けるもの：作者やシリーズそのものを指す問い合わせの解釈と、
//! 一致した箇所の抜粋・理由。どちらも旧棚の判定と抜粋の関数をそのまま使う。

use super::index::Space;
use super::query::parse_query;
use super::semantic::SemanticEvidence;
use crate::database::models::{ScoreReason, SearchHighlight, SearchHighlightSegment};
use crate::database::search::{make_match_highlights, match_fields_and_score, parse_search_query};
use crate::database::tantivy_index;
use crate::library::{
    Error, Library, LibraryId, Result, TextMatches, WorkListPage, WorkListQuery, WorkSort,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// 問い合わせが作者名やシリーズ名とそのまま一致したときの解釈。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum EntityIntent {
    /// この名前の作者の作品に絞った（本文に名前が出てくるだけの作品は含めない）。
    Author { label: String },
    /// このシリーズの作品に絞った。同じ題のシリーズが複数あれば（取得元の違いなど）、
    /// `series` は空で、その題のシリーズすべてに絞る（旧棚と同じ）。
    Series {
        label: String,
        series: Option<LibraryId>,
    },
}

/// どの版を探すか。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionScope {
    /// いまの版（有効な編集があれば編集後）だけ。
    #[default]
    Current,
    /// いまの版と、保存してある過去の改稿。作品ごとに一件にまとめる。
    All,
}

/// 何で一致を見るか。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    /// 語の一致（全文索引）。
    #[default]
    Words,
    /// 意味の近さ（意味検索の索引）。過去の改稿は持たないので、過去の改稿も探すときは
    /// 語の一致にする（旧棚と同じ）。
    Meaning,
}

/// 本文の検索の問い合わせ。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextSearch {
    pub text: String,
    #[serde(default)]
    pub versions: VersionScope,
    #[serde(default)]
    pub mode: SearchMode,
}

impl TextSearch {
    pub fn current(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }

    pub fn all_versions(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            versions: VersionScope::All,
            ..Default::default()
        }
    }

    pub fn meaning(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            mode: SearchMode::Meaning,
            ..Default::default()
        }
    }

    /// 実際に使う照合。意味検索は過去の改稿を持たない。
    pub fn effective_mode(&self) -> SearchMode {
        match (self.mode, self.versions) {
            (SearchMode::Meaning, VersionScope::Current) => SearchMode::Meaning,
            _ => SearchMode::Words,
        }
    }
}

/// 過去の改稿だけで一致した作品の関連度に掛ける重み。過去の改稿の索引は文書が少なく、
/// 語の珍しさの重みがいまの版の索引と揃わないので、関連度は目安でしかない。いまの版で同じ
/// くらいに一致した作品を先に出すために、半分にする（旧棚も、過去の版の一致は控えめな点に
/// していた）。
const HISTORY_WEIGHT: f32 = 0.5;

/// 本文の検索の一ページ。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchPage {
    pub page: WorkListPage,
    /// 作者・シリーズとして解釈したなら、その中身。画面はこれを「作者『…』に限定しました」と示す。
    pub intent: Option<EntityIntent>,
    /// ページの作品ごとの一致（ページと同じ順）。作者・シリーズとして解釈したときは空。
    pub matches: Vec<SearchMatch>,
}

/// 一作品の一致。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchMatch {
    pub work: LibraryId,
    /// 関連度。過去の改稿だけで一致したなら、その関連度に `HISTORY_WEIGHT` を掛けたもの。
    pub score: f64,
    /// いまの版では一致せず、過去の改稿で一致したとき、その改稿の番号（いちばん関連度の
    /// 高いもの。同点なら新しいもの）。
    pub matched_version: Option<i64>,
    /// 一致した過去の改稿の数（いまの版でも一致したときも数える）。
    pub historical_matches: u32,
    /// 意味検索で出た作品の近さと根拠。
    pub semantic: Option<SemanticEvidence>,
}

/// 過去の改稿の一致を作品ごとにまとめたもの。
struct PastMatch {
    score: f32,
    version: i64,
    count: u32,
}

/// 一致した箇所。旧棚の検索結果の `match_fields`・`score_reasons`・`match_highlights` と同じ
/// （抜粋の `text` だけは、項目の全文でなく一致の前後の窓）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchDecoration {
    pub work: LibraryId,
    pub match_fields: Vec<String>,
    pub score_reasons: Vec<ScoreReason>,
    pub highlights: Vec<SearchHighlight>,
    /// 一致した項目の重みから出した点。旧棚は表示する関連度にこれを足していた（並びには使わない）。
    pub field_score: f64,
}

fn truncate(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

/// 作者・シリーズの名前として扱ってよい問い合わせ（旧棚と同じ）。引用符・除外・項目の指定を
/// 含むものは、名前でなく検索の式として扱う。
fn plain_entity_query(query: &str) -> bool {
    !query.is_empty()
        && query.chars().count() <= 160
        && !query.contains(':')
        && !query.contains('"')
        && !query.starts_with('-')
        && !query.contains(" -")
}

impl Library {
    /// 本文の検索で一致した作品の一覧。
    ///
    /// 問い合わせが一人の作者名、または一つのシリーズ名と大文字小文字を除いてそのまま
    /// 一致し、もう一方には一致しなければ、その作者・シリーズの作品に絞る（旧棚と同じ）。
    /// このときは本文の一致を使わないので、関連度の順は保存日の新しい順に置き換える。
    ///
    /// 過去の改稿も探すときは、作品ごとに一件にまとめる。いまの版で一致した作品はその関連度、
    /// 過去の改稿だけで一致した作品は、いちばん関連度の高い改稿の関連度に `HISTORY_WEIGHT` を
    /// 掛けたもの。
    ///
    /// 問い合わせの中の絞り込みの書き方（`tag:` など、`query.rs`）は先に一覧の絞り込みへ移す。
    /// 残る語がなければ、絞り込んだ一覧をそのまま返す（関連度の順は保存日の新しい順にする）。
    pub fn search_works(&self, search: &TextSearch, query: &WorkListQuery) -> Result<SearchPage> {
        let parts = parse_query(&search.text);
        let mut query = query.clone();
        self.apply_query_parts(&parts, &mut query.filter)?;
        let query = &mut query;
        let text = parts.text.trim();
        if text.is_empty() {
            if query.sort == WorkSort::Relevance {
                query.sort = WorkSort::Saved;
                query.descending = true;
            }
            return Ok(SearchPage {
                page: self.list_works(query)?,
                intent: None,
                matches: Vec::new(),
            });
        }
        if search.effective_mode() == SearchMode::Meaning {
            return self.search_by_meaning(text, query);
        }
        let intent = if plain_entity_query(text)
            && query.filter.authors.is_empty()
            && query.filter.person.is_none()
            && query.filter.series.is_none()
        {
            self.entity_intent(text)?
        } else {
            None
        };
        let Some(intent) = intent else {
            let mut found = self.matching_documents(text)?;
            let past = match search.versions {
                VersionScope::Current => HashMap::new(),
                VersionScope::All => self.past_matches(text)?,
            };
            let current_len = found.len();
            if !past.is_empty() {
                let current: HashSet<i64> = found.iter().map(|(key, _)| *key).collect();
                found.extend(
                    past.iter()
                        .filter(|(key, _)| !current.contains(key))
                        .map(|(key, hit)| (*key, hit.score)),
                );
            }
            let page = self.list_works_matching(query, &TextMatches::new(&found))?;
            let matches = self.page_matches(&page, &found, current_len, &past)?;
            return Ok(SearchPage {
                page,
                intent: None,
                matches,
            });
        };
        // 続きのページの位置は、最初のページを保存日の順で返したときのもの。そのまま使える。
        if query.sort == WorkSort::Relevance {
            query.sort = WorkSort::Saved;
            query.descending = true;
        }
        match &intent {
            EntityIntent::Author { label } => query.filter.authors = vec![label.clone()],
            EntityIntent::Series {
                series: Some(series),
                ..
            } => query.filter.series = Some(*series),
            EntityIntent::Series { series: None, .. } => {
                let mut works = self.works_in_series_titled(text)?;
                if let Some(only) = &query.filter.works {
                    works.retain(|work| only.contains(work));
                }
                query.filter.works = Some(works);
            }
        }
        Ok(SearchPage {
            page: self.list_works(query)?,
            intent: Some(intent),
            matches: Vec::new(),
        })
    }

    /// 意味の近い作品の一覧。近さの下限に届いた作品すべてが一致で、絞り込み・並べ替え・
    /// 続きのページは語の一致と同じ。作者・シリーズの解釈はしない（旧棚と同じ）。
    ///
    /// 除く語（`-語`）は、語の一致でその語を含む作品を除く。意味の問い合わせには除く語を
    /// 入れない（旧棚は問い合わせをそのまま埋め込んでいて、除きたい語に近い作品が上がっていた）。
    fn search_by_meaning(&self, text: &str, query: &WorkListQuery) -> Result<SearchPage> {
        let parsed = parse_search_query(text);
        let meaning = if parsed.exclude.is_empty() {
            text.to_owned()
        } else {
            parsed
                .include
                .iter()
                .map(|term| term.raw.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        };
        let mut evidence: HashMap<i64, SemanticEvidence> =
            self.semantic_matches(&meaning)?.into_iter().collect();
        for term in &parsed.exclude {
            let words = if term.is_phrase {
                format!("\"{}\"", term.raw)
            } else {
                term.raw.clone()
            };
            for (key, _) in self.matching_documents(&words)? {
                evidence.remove(&key);
            }
        }
        let found: Vec<(i64, f32)> = evidence
            .iter()
            .map(|(key, hit)| (*key, hit.score as f32))
            .collect();
        let page = self.list_works_matching(query, &TextMatches::new(&found))?;
        let mut matches = self.page_matches(&page, &found, found.len(), &HashMap::new())?;
        let conn = self.reader()?;
        let mut key =
            conn.prepare_cached("SELECT doc_key FROM search_documents WHERE work_id=?1")?;
        for hit in &mut matches {
            let doc_key: i64 = key.query_row([hit.work.as_bytes().as_slice()], |r| r.get(0))?;
            hit.semantic = evidence.remove(&doc_key);
            if let Some(semantic) = &hit.semantic {
                hit.score = semantic.score;
            }
        }
        Ok(SearchPage {
            page,
            intent: None,
            matches,
        })
    }

    /// 過去の改稿の一致を、作品のいまの版の文書の鍵ごとにまとめる。
    fn past_matches(&self, text: &str) -> Result<HashMap<i64, PastMatch>> {
        let found = self.matching_in(Space::History, text)?;
        let mut past: HashMap<i64, PastMatch> = HashMap::new();
        if found.is_empty() {
            return Ok(past);
        }
        let conn = self.reader()?;
        let mut rows = conn.prepare_cached(
            "SELECT h.doc_key,d.doc_key,r.ordinal FROM json_each(?1) j
             JOIN search_history_documents h ON h.doc_key=j.value
             JOIN search_documents d ON d.work_id=h.work_id
             JOIN source_revisions r ON r.id=h.revision_id",
        )?;
        for chunk in found.chunks(1000) {
            let scores: HashMap<i64, f32> = chunk.iter().copied().collect();
            let keys = serde_json::to_string(&scores.keys().collect::<Vec<_>>())?;
            let mut result = rows.query([keys])?;
            while let Some(row) = result.next()? {
                let score = scores[&row.get::<_, i64>(0)?] * HISTORY_WEIGHT;
                let current: i64 = row.get(1)?;
                let version: i64 = row.get(2)?;
                let entry = past.entry(current).or_insert(PastMatch {
                    score,
                    version,
                    count: 0,
                });
                entry.count += 1;
                if score > entry.score || (score == entry.score && version > entry.version) {
                    entry.score = score;
                    entry.version = version;
                }
            }
        }
        Ok(past)
    }

    /// ページの作品ごとの一致。
    fn page_matches(
        &self,
        page: &WorkListPage,
        found: &[(i64, f32)],
        current_len: usize,
        past: &HashMap<i64, PastMatch>,
    ) -> Result<Vec<SearchMatch>> {
        let conn = self.reader()?;
        let mut key =
            conn.prepare_cached("SELECT doc_key FROM search_documents WHERE work_id=?1")?;
        // `found` の先頭 `current_len` 件がいまの版の一致、残りが過去の改稿だけの一致。
        let scores: HashMap<i64, f32> = found.iter().copied().collect();
        let current: HashSet<i64> = found[..current_len].iter().map(|(key, _)| *key).collect();
        let mut matches = Vec::with_capacity(page.items.len());
        for card in &page.items {
            let doc_key: i64 = key.query_row([card.id.as_bytes().as_slice()], |r| r.get(0))?;
            let history = past.get(&doc_key);
            matches.push(SearchMatch {
                work: card.id,
                score: f64::from(scores.get(&doc_key).copied().unwrap_or_default()),
                matched_version: history
                    .filter(|_| !current.contains(&doc_key))
                    .map(|hit| hit.version),
                historical_matches: history.map_or(0, |hit| hit.count),
                semantic: None,
            });
        }
        Ok(matches)
    }

    /// 問い合わせが作者名・シリーズ名そのものか。作者名の表記はいちばん多く使われているもの。
    fn entity_intent(&self, text: &str) -> Result<Option<EntityIntent>> {
        let conn = self.reader()?;
        let author: Option<String> = conn
            .prepare_cached(
                "SELECT author_name FROM work_current_projection WHERE author_name=?1 COLLATE NOCASE
                 GROUP BY author_name ORDER BY count(*) DESC,author_name LIMIT 1",
            )?
            .query_row([text], |r| r.get(0))
            .optional()?;
        let series: Vec<(String, Vec<u8>)> = conn
            .prepare_cached(
                "SELECT min(ss.title),ss.series_id FROM snapshot_series ss
                 JOIN work_current_projection p ON p.snapshot_id=ss.snapshot_id
                 WHERE ss.title=?1 COLLATE NOCASE GROUP BY ss.series_id
                 ORDER BY count(*) DESC,ss.series_id LIMIT 2",
            )?
            .query_map([text], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(match (author, series.as_slice()) {
            (Some(label), []) => Some(EntityIntent::Author { label }),
            (None, [(label, series)]) => Some(EntityIntent::Series {
                label: label.clone(),
                series: Some(
                    LibraryId::from_slice(series)
                        .map_err(|_| Error::Integrity("シリーズの ID が不正です".into()))?,
                ),
            }),
            (None, [(label, _), _]) => Some(EntityIntent::Series {
                label: label.clone(),
                series: None,
            }),
            _ => None,
        })
    }

    /// いまの版がこの題のシリーズに入っている作品。
    fn works_in_series_titled(&self, title: &str) -> Result<Vec<LibraryId>> {
        let conn = self.reader()?;
        let mut statement = conn.prepare_cached(
            "SELECT DISTINCT p.work_id FROM snapshot_series ss
             JOIN work_current_projection p ON p.snapshot_id=ss.snapshot_id
             WHERE ss.title=?1 COLLATE NOCASE",
        )?;
        let rows = statement.query_map([title], |r| r.get::<_, Vec<u8>>(0))?;
        rows.map(|bytes| {
            LibraryId::from_slice(&bytes?)
                .map_err(|_| Error::Integrity("作品の ID が不正です".into()))
        })
        .collect()
    }

    /// 一ページぶんの一致について、問い合わせに一致した箇所の抜粋と理由を作る。文書は索引に
    /// 入っている形（旧棚と同じ文書）から読む。過去の改稿だけで一致した作品は、その改稿の
    /// 文書から作る。索引に文書がない作品は飛ばす。
    pub fn search_decorations(
        &self,
        text: &str,
        matches: &[SearchMatch],
    ) -> Result<Vec<SearchDecoration>> {
        // 絞り込みの書き方（`tag:` など）は一致した箇所を持たない。
        let parsed = parse_search_query(&parse_query(text).text);
        if parsed.include.is_empty() && matches.iter().all(|hit| hit.semantic.is_none()) {
            return Ok(Vec::new());
        }
        let conn = self.reader()?;
        let mut current =
            conn.prepare_cached("SELECT doc_key FROM search_documents WHERE work_id=?1")?;
        let mut past = conn.prepare_cached(
            "SELECT h.doc_key FROM search_history_documents h JOIN source_revisions r ON r.id=h.revision_id
             WHERE h.work_id=?1 AND r.ordinal=?2",
        )?;
        let mut decorations = Vec::with_capacity(matches.len());
        for hit in matches {
            let work = hit.work.as_bytes();
            let (space, doc_key) = match hit.matched_version {
                None => (
                    Space::Current,
                    current
                        .query_row([work.as_slice()], |r| r.get::<_, i64>(0))
                        .optional()?,
                ),
                Some(version) => (
                    Space::History,
                    past.query_row(params![work.as_slice(), version], |r| r.get::<_, i64>(0))
                        .optional()?,
                ),
            };
            let Some(doc_key) = doc_key else {
                continue;
            };
            let Some(document) = tantivy_index::load_document(&self.space_storage(space), doc_key)
                .map_err(Error::Integrity)?
            else {
                continue;
            };
            let (match_fields, reasons, field_score) = match_fields_and_score(&document, &parsed);
            let mut score_reasons: Vec<ScoreReason> = reasons
                .into_iter()
                .map(|reason| ScoreReason {
                    field: reason.field,
                    match_type: reason.match_type,
                    term: reason.term,
                    contribution: reason.contribution,
                    detail: reason.detail,
                })
                .collect();
            let mut highlights: Vec<SearchHighlight> = make_match_highlights(&document, &parsed)
                .into_iter()
                .map(|mut highlight| {
                    // 旧棚は項目の全文（本文なら作品まるごと）を `text` に入れていた。
                    // 一覧の一件ごとに本文を運ぶことになるので、一致の前後の窓だけにする。
                    highlight.text = highlight
                        .segments
                        .iter()
                        .map(|segment| segment.text.as_str())
                        .collect();
                    highlight
                })
                .collect();
            // 意味検索で出た作品は、近さを理由の一つに、根拠の文を抜粋の先頭に置く（旧棚と同じ）。
            if let Some(semantic) = &hit.semantic {
                score_reasons.push(ScoreReason {
                    field: semantic.field.clone(),
                    match_type: "semantic".into(),
                    term: parsed
                        .include
                        .first()
                        .map(|term| term.raw.clone())
                        .unwrap_or_default(),
                    contribution: semantic.score * 120.0,
                    detail: Some(format!("semantic work score {:.3}", semantic.score)),
                });
                let shown = truncate(&semantic.text, 220);
                highlights.insert(
                    0,
                    SearchHighlight {
                        field: semantic.field.clone(),
                        text: shown.clone(),
                        // 作品全体との近さで、一致した箇所はない。
                        segments: vec![SearchHighlightSegment {
                            text: shown,
                            matched: false,
                        }],
                        source_chunk_id: Some("work".into()),
                        match_type: Some("semantic".into()),
                    },
                );
                highlights.truncate(4);
            }
            decorations.push(SearchDecoration {
                work: hit.work,
                match_fields,
                score_reasons,
                highlights,
                field_score,
            });
        }
        Ok(decorations)
    }
}

#[cfg(test)]
#[path = "results_tests.rs"]
mod tests;
