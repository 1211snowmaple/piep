//! 本文の検索の残りの機能を旧棚と突き合わせる：一致した箇所の抜粋と理由、作者・シリーズ名の
//! 解釈、過去の改稿の検索、意味検索。旧棚は写しの DB と一時の場所に作った索引で、旧棚の検索の
//! 入り口（`search_downloads_v2`）をそのまま呼ぶ。本番の索引は開かない。
//!
//! 意味検索は、旧棚の意味検索の索引の写し（`<work>/old-semantic/search/semantic-v2/index.sqlite`）と
//! モデルの写し（`<work>/old-semantic/models/`）があるときだけ比べる。新しい棚はその写しからベクトルを
//! 引き継いで作る。
//!
//! 出力は件数と問い合わせの番号だけで、問い合わせや作品の文字列は書かない。

use piep_lib::database::models::{DownloadEntry, SearchV2Params};
use piep_lib::database::queries::SearchIndexRebuildOptions;
use piep_lib::database::{semantic_index, tantivy_index, Database};
use piep_lib::library::{Library, LibraryId, WorkListQuery, WorkSort};
use piep_lib::library_search::{EntityIntent, SearchMatch, TextSearch};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::time::Instant;

type Error = Box<dyn std::error::Error>;
type Identity = (String, String);
/// 新しい棚の検索の結果: 一致の内訳・指名・一覧の順の作品。
type NewSearch = (Vec<SearchMatch>, Option<EntityIntent>, Vec<LibraryId>);

struct Notes {
    counts: BTreeMap<String, usize>,
    examples: BTreeMap<String, Vec<Value>>,
}

impl Notes {
    fn new() -> Self {
        Self {
            counts: BTreeMap::new(),
            examples: BTreeMap::new(),
        }
    }

    fn note(&mut self, kind: &str, detail: Value) {
        *self.counts.entry(kind.to_owned()).or_default() += 1;
        let list = self.examples.entry(kind.to_owned()).or_default();
        if list.len() < 6 {
            list.push(detail);
        }
    }

    fn json(&self) -> Value {
        json!({"mismatches": self.counts, "examples": self.examples})
    }
}

fn chars_at(text: &str, at: usize, length: usize) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < at + length {
        return None;
    }
    let piece: String = chars[at..at + length].iter().collect();
    (!piece.trim().is_empty()
        && piece
            .chars()
            .all(|c| !c.is_whitespace() && c != '"' && c != '-' && c != ':'))
    .then_some(piece)
}

struct Shelves {
    old: Database,
    new: Library,
    old_identity: HashMap<i64, Identity>,
    new_identity: HashMap<LibraryId, Identity>,
    old_storage: std::path::PathBuf,
}

impl Shelves {
    /// 旧棚の検索。続きのページを最後までたどる。
    fn old_search(&self, params: SearchV2Params) -> Result<(Vec<DownloadEntry>, Value), Error> {
        let mut items = Vec::new();
        let mut cursor = None;
        let mut intent = Value::Null;
        loop {
            let result = self.old.search_downloads_v2(&SearchV2Params {
                limit: Some(1000),
                cursor: cursor.clone(),
                ..params.clone()
            })?;
            if let Some(found) = &result.search_meta.exact_entity {
                intent = json!([found.kind, found.label]);
            }
            items.extend(result.items);
            match result.next_cursor {
                Some(next) if Some(&next) != cursor.as_ref() => cursor = Some(next),
                _ => break,
            }
        }
        Ok((items, intent))
    }

    /// 新しい棚の検索。続きのページを最後までたどる。
    fn new_search(&self, search: &TextSearch) -> Result<NewSearch, Error> {
        let mut query = WorkListQuery {
            sort: WorkSort::Relevance,
            limit: 500,
            ..Default::default()
        };
        let (mut matches, mut ids, mut intent) = (Vec::new(), Vec::new(), None);
        loop {
            let page = self.new.search_works(search, &query)?;
            ids.extend(page.page.items.iter().map(|card| card.id));
            matches.extend(page.matches);
            intent = intent.or(page.intent);
            match page.page.next {
                Some(next) => {
                    query.after = Some(next);
                    query.count = false;
                }
                None => break,
            }
        }
        Ok((matches, intent, ids))
    }

    fn old_set(&self, items: &[DownloadEntry]) -> BTreeSet<Identity> {
        items
            .iter()
            .filter_map(|item| self.old_identity.get(&item.id).cloned())
            .collect()
    }

    /// 一作品の、旧棚と新しい棚の索引の文書の本文の違い（文字は出さず、数と有無だけ）。
    fn body_facts(&self, identity: &Identity, query: &str) -> Result<Value, Error> {
        let old_id = self
            .old_identity
            .iter()
            .find(|(_, i)| *i == identity)
            .map(|(id, _)| *id);
        let new_id = self
            .new_identity
            .iter()
            .find(|(_, i)| *i == identity)
            .map(|(id, _)| *id);
        let old = match old_id {
            Some(id) => tantivy_index::load_document(&self.old_storage, id)?,
            None => None,
        };
        let new = match new_id {
            Some(id) => self.new.search_document(id)?,
            None => None,
        };
        let facts = |body: &str| {
            json!({"chars": body.chars().count(), "contains": body.contains(query),
                   "matches": body.matches(query).count()})
        };
        let first_difference = match (&old, &new) {
            (Some(old), Some(new)) => old
                .body
                .chars()
                .zip(new.body.chars())
                .position(|(a, b)| a != b),
            _ => None,
        };
        let old_fields = old.as_ref().map(|d| {
            [
                ("title", d.title.clone()),
                ("author", d.author_name.clone()),
                ("tags", d.tags.clone()),
                ("series", d.series_title.clone()),
                ("excerpt", d.excerpt.clone()),
            ]
            .into_iter()
            .filter(|(_, text)| text.contains(query))
            .map(|(name, _)| name)
            .collect::<Vec<_>>()
        });
        let new_fields = new.as_ref().map(|d| {
            [
                ("title", d.title.clone()),
                ("author", d.author_name.clone()),
                ("author_id", d.author_key.clone()),
                (
                    "url",
                    piep_lib::library_search::source_url(
                        &d.provider,
                        &d.external_key,
                        &d.author_key,
                    ),
                ),
                ("tags", d.tags.join(" ")),
                ("series", d.series_titles.join(" ")),
                ("excerpt", d.excerpt.clone()),
            ]
            .into_iter()
            .filter(|(_, text)| text.contains(query))
            .map(|(name, _)| name)
            .collect::<Vec<_>>()
        });
        let tags_same = old.as_ref().zip(new.as_ref()).map(|(o, n)| {
            let mut a: Vec<&str> = o.tags.split(' ').collect();
            let mut b: Vec<&str> = n.tags.iter().flat_map(|t| t.split(' ')).collect();
            a.sort();
            b.sort();
            a == b
        });
        // 読みの欄は、つないだ文字列の読み。タグの順で境目をまたぐ並びが変わる。
        use piep_lib::database::search_normalization::{reading_kana_text, reading_romaji_text};
        let (kana, romaji) = (reading_kana_text(query), reading_romaji_text(query));
        let reading = |text: &str| {
            json!({"kana": !kana.is_empty() && reading_kana_text(text).contains(&kana),
                   "romaji": !romaji.is_empty() && reading_romaji_text(text).contains(&romaji)})
        };
        let readings = json!({
            "old_tags": old.as_ref().map(|d| reading(&d.tags)),
            "new_tags": new.as_ref().map(|d| reading(&d.tags.join(" "))),
            "old_title": old.as_ref().map(|d| reading(&d.title)),
            "new_title": new.as_ref().map(|d| reading(&d.title)),
            "old_excerpt": old.as_ref().map(|d| reading(&d.excerpt)),
            "new_excerpt": new.as_ref().map(|d| reading(&d.excerpt)),
            "old_series": old.as_ref().map(|d| reading(&d.series_title)),
            "new_series": new.as_ref().map(|d| reading(&d.series_titles.join(" "))),
            "body": new.as_ref().map(|d| reading(&d.body)),
        });
        Ok(json!({
            "id": identity,
            "readings": readings,
            "old_fields": old_fields,
            "new_fields": new_fields,
            "tags_same_set": tags_same,
            "old_body": old.as_ref().map(|d| facts(&d.body)),
            "new_body": new.as_ref().map(|d| facts(&d.body)),
            "bodies_equal": old.as_ref().zip(new.as_ref()).map(|(o, n)| o.body == n.body),
            "first_difference_at": first_difference,
        }))
    }

    fn new_set(&self, ids: &[LibraryId]) -> BTreeSet<Identity> {
        ids.iter()
            .filter_map(|id| self.new_identity.get(id).cloned())
            .collect()
    }
}

pub fn run(old_db: &Path, work: &Path, new_root: &Path) -> Result<Value, Error> {
    let started = Instant::now();
    let old_storage = work.join("old").join("downloads");
    std::fs::create_dir_all(&old_storage)?;
    semantic_index::set_model_cache_dir(&work.join("old-semantic").join("downloads"));
    let old = Database::open(old_db, &old_storage)?;
    let t = Instant::now();
    old.rebuild_search_index(
        SearchIndexRebuildOptions {
            include_semantic: false,
            ..Default::default()
        },
        &|| false,
        |_| {},
    )?;
    let old_build_seconds = t.elapsed().as_secs_f64();
    let new = Library::open(new_root)?;
    let t = Instant::now();
    let report = new.sync_search_index(&mut |_, _| {})?;
    let new_build_seconds = t.elapsed().as_secs_f64();

    let conn =
        rusqlite::Connection::open_with_flags(old_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let old_identity: HashMap<i64, Identity> = conn
        .prepare("SELECT id,source,source_id FROM downloads")?
        .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?
        .collect::<Result<_, _>>()?;
    let new_conn = rusqlite::Connection::open_with_flags(
        new_root.join("library.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let new_identity: HashMap<LibraryId, Identity> = new_conn
        .prepare("SELECT work_id,provider,external_key FROM work_current_projection")?
        .query_map([], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                (r.get::<_, String>(1)?, r.get::<_, String>(2)?),
            ))
        })?
        .map(|row| {
            let (id, identity) = row?;
            Ok((LibraryId::from_slice(&id).expect("ID"), identity))
        })
        .collect::<Result<_, rusqlite::Error>>()?;
    let shelves = Shelves {
        old,
        new,
        old_identity,
        new_identity,
        old_storage: old_storage.clone(),
    };

    let decorations = compare_decorations(&shelves, &new_conn)?;
    let intents = compare_intents(&shelves, &new_conn)?;
    let history = compare_history(&shelves, &new_conn, new_root)?;
    let structured = compare_structured(&shelves, &new_conn)?;
    // 旧棚の意味検索の写しは、旧棚の索引づくり（意味検索を切って作る）が消さない場所に置く。
    let semantic_storage = work.join("old-semantic").join("downloads");
    let semantic = if work
        .join("old-semantic")
        .join("search")
        .join("semantic-v2")
        .join("index.sqlite")
        .exists()
    {
        compare_semantic(&shelves, &semantic_storage, &new_conn)?
    } else {
        Value::Null
    };
    Ok(json!({
        "old_build_seconds": old_build_seconds,
        "new_build_seconds": new_build_seconds,
        "new_report": format!("{report:?}"),
        "decorations": decorations,
        "intents": intents,
        "history": history,
        "structured": structured,
        "semantic": semantic,
        "seconds": started.elapsed().as_secs_f64(),
    }))
}

/// 一致した箇所と理由。旧棚の検索結果に付いたものと、新しい棚の `search_decorations` を、
/// 両方に出た作品ごとに比べる。抜粋は区切り（一致・非一致の断片）の文字列で比べる
/// （新しい棚は `text` を窓だけにしたので、`text` は比べない）。
fn compare_decorations(shelves: &Shelves, conn: &rusqlite::Connection) -> Result<Value, Error> {
    let mut queries = Vec::new();
    let mut statement =
        conn.prepare("SELECT p.work_id FROM work_current_projection p ORDER BY p.work_id")?;
    let works: Vec<Vec<u8>> = statement
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    for (index, work) in works.iter().enumerate() {
        if index % 6 != 0 {
            continue;
        }
        let Some(doc) = shelves
            .new
            .search_document(LibraryId::from_slice(work).expect("ID"))?
        else {
            continue;
        };
        if let Some(tag) = doc.tags.first() {
            queries.push(tag.clone());
        }
        let length = doc.body.chars().count();
        for (at, size) in [(length / 3, 2), (length / 2, 4)] {
            if let Some(piece) = chars_at(&doc.body, at, size) {
                queries.push(piece);
            }
        }
        if let Some(piece) = chars_at(&doc.title, 0, 2) {
            queries.push(piece);
        }
    }
    queries.sort();
    queries.dedup();
    let mut notes = Notes::new();
    let (mut compared, mut old_ms, mut new_ms) = (0usize, 0.0, 0.0);
    for (number, query) in queries.iter().enumerate() {
        let t = Instant::now();
        let (old_items, old_intent) = shelves.old_search(SearchV2Params {
            query: Some(query.clone()),
            ..Default::default()
        })?;
        old_ms += t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let (matches, intent, ids) = shelves.new_search(&TextSearch::current(query.clone()))?;
        let marks = shelves.new.search_decorations(query, &matches)?;
        new_ms += t.elapsed().as_secs_f64() * 1000.0;
        if !old_intent.is_null() || intent.is_some() {
            notes.note("skipped_entity_query", json!({"query": number}));
            continue;
        }
        let (old_set, new_set) = (shelves.old_set(&old_items), shelves.new_set(&ids));
        if old_set != new_set {
            // 問い合わせの文字列は作業フォルダーの JSON にだけ残す（返答には写さない）。
            notes.note(
                "set",
                json!({"query": number, "text": query, "old": old_set.len(), "new": new_set.len(),
                       "only_old": old_set.difference(&new_set).map(|i| shelves.body_facts(i, query)).collect::<Result<Vec<_>, _>>()?,
                       "only_new": new_set.difference(&old_set).map(|i| shelves.body_facts(i, query)).collect::<Result<Vec<_>, _>>()?}),
            );
            continue;
        }
        let marks: HashMap<&Identity, _> = marks
            .iter()
            .filter_map(|mark| shelves.new_identity.get(&mark.work).map(|i| (i, mark)))
            .collect();
        for item in &old_items {
            let Some(identity) = shelves.old_identity.get(&item.id) else {
                continue;
            };
            let Some(mark) = marks.get(identity) else {
                notes.note("missing_new_decoration", json!({"query": number}));
                continue;
            };
            compared += 1;
            if item.match_fields != mark.match_fields {
                notes.note(
                    "match_fields",
                    json!({"query": number, "old": item.match_fields, "new": mark.match_fields}),
                );
            }
            let reasons = |list: &[piep_lib::database::models::ScoreReason]| {
                list.iter()
                    .map(|r| {
                        (
                            r.field.clone(),
                            r.match_type.clone(),
                            r.term.clone(),
                            (r.contribution * 1000.0).round() as i64,
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let not_tags = |list: &[piep_lib::database::models::ScoreReason]| {
                let mut kept = reasons(list);
                kept.retain(|r| r.0 != "tags");
                kept
            };
            if reasons(&item.score_reasons) != reasons(&mark.score_reasons)
                && not_tags(&item.score_reasons) == not_tags(&mark.score_reasons)
            {
                // ローマ字の一致はつないだタグの文字列で見るので、タグの順でまたがる一致が変わる。
                notes.note("score_reasons_tags_only", json!({"query": number}));
            } else if reasons(&item.score_reasons) != reasons(&mark.score_reasons) {
                let shape = |list: &[piep_lib::database::models::ScoreReason]| {
                    list.iter()
                        .map(|r| json!([r.field, r.match_type, r.contribution]))
                        .collect::<Vec<_>>()
                };
                notes.note(
                    "score_reasons",
                    json!({"query": number, "old": shape(&item.score_reasons), "new": shape(&mark.score_reasons)}),
                );
            }
            let highlights = |list: &[piep_lib::database::models::SearchHighlight]| {
                list.iter()
                    .map(|h| {
                        (
                            h.field.clone(),
                            h.match_type.clone(),
                            h.segments
                                .iter()
                                .map(|s| (s.text.clone(), s.matched))
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let without_tags = |list: &[piep_lib::database::models::SearchHighlight]| {
                let kept: Vec<_> = list.iter().filter(|h| h.field != "tags").cloned().collect();
                highlights(&kept)
            };
            // 一致した部分だけ（窓の前後の文脈を除く）。
            let matched_only = |list: &[piep_lib::database::models::SearchHighlight]| {
                list.iter()
                    .map(|h| {
                        (
                            h.field.clone(),
                            h.match_type.clone(),
                            h.segments
                                .iter()
                                .filter(|s| s.matched)
                                .map(|s| s.text.clone())
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let differs = highlights(&item.match_highlights) != highlights(&mark.highlights);
            let context_fields_only =
                item.match_highlights
                    .iter()
                    .zip(&mark.highlights)
                    .all(|(old, new)| {
                        highlights(std::slice::from_ref(old))
                            == highlights(std::slice::from_ref(new))
                            || matches!(old.field.as_str(), "tags" | "excerpt")
                    });
            if differs && without_tags(&item.match_highlights) == without_tags(&mark.highlights) {
                // 旧棚のタグは内部番号の順につないだ文字列で、新しい棚は名前の順。抜粋の窓の
                // 位置が変わるだけで、一致したタグは同じ。
                notes.note("highlights_tag_order_only", json!({"query": number}));
            } else if differs
                && context_fields_only
                && matched_only(&item.match_highlights) == matched_only(&mark.highlights)
            {
                // タグの順に加えて、説明の前後の空白（旧棚は残し、新しい棚は落とす）で
                // 窓の文脈の長さだけが違う。一致した部分は同じ。
                notes.note("highlights_context_only", json!({"query": number}));
            } else if differs {
                let shape = |list: &[piep_lib::database::models::SearchHighlight]| {
                    list.iter()
                        .map(|h| {
                            json!({"field": h.field, "type": h.match_type,
                                   "segments": h.segments.iter().map(|s| json!([s.text.chars().count(), s.matched])).collect::<Vec<_>>()})
                        })
                        .collect::<Vec<_>>()
                };
                notes.note(
                    "highlights",
                    json!({"query": number, "old": shape(&item.match_highlights), "new": shape(&mark.highlights)}),
                );
            }
            // 新しい棚の `text` は区切りをつないだ窓そのもの。
            if mark.highlights.iter().any(|h| {
                h.segments
                    .iter()
                    .map(|s| s.text.as_str())
                    .collect::<String>()
                    != h.text
            }) {
                notes.note("highlight_window_text", json!({"query": number}));
            }
        }
    }
    let mut out = notes.json();
    out["queries"] = json!(queries.len());
    out["works_compared"] = json!(compared);
    out["old_ms_total"] = json!(old_ms);
    out["new_ms_total"] = json!(new_ms);
    Ok(out)
}

/// 作者名・シリーズ名そのものの問い合わせ。旧棚の解釈（種類と表記）と、出る作品の集まりを比べる。
fn compare_intents(shelves: &Shelves, conn: &rusqlite::Connection) -> Result<Value, Error> {
    let mut names: Vec<String> = conn
        .prepare("SELECT DISTINCT author_name FROM work_current_projection WHERE author_name<>''")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let series: Vec<String> = conn
        .prepare(
            "SELECT DISTINCT ss.title FROM snapshot_series ss
             JOIN work_current_projection p ON p.snapshot_id=ss.snapshot_id WHERE ss.title<>''",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    names.extend(series);
    // 大文字小文字の違いでも同じに解釈するか。
    let lowered: Vec<String> = names
        .iter()
        .filter(|name| name.to_lowercase() != **name)
        .map(|name| name.to_lowercase())
        .collect();
    names.extend(lowered);
    names.sort();
    names.dedup();
    let mut notes = Notes::new();
    let (mut kinds, mut old_ms, mut new_ms) = (BTreeMap::<String, usize>::new(), 0.0, 0.0);
    for (number, name) in names.iter().enumerate() {
        let t = Instant::now();
        let (old_items, old_intent) = shelves.old_search(SearchV2Params {
            query: Some(name.clone()),
            ..Default::default()
        })?;
        old_ms += t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let (_, intent, ids) = shelves.new_search(&TextSearch::current(name.clone()))?;
        new_ms += t.elapsed().as_secs_f64() * 1000.0;
        let new_intent = match &intent {
            Some(EntityIntent::Author { label }) => json!(["author", label]),
            Some(EntityIntent::Series { label, .. }) => json!(["series", label]),
            None => Value::Null,
        };
        *kinds
            .entry(
                new_intent
                    .get(0)
                    .and_then(Value::as_str)
                    .unwrap_or("none")
                    .to_owned(),
            )
            .or_default() += 1;
        if old_intent != new_intent {
            let same_kind = old_intent.get(0) == new_intent.get(0);
            notes.note(
                "intent",
                json!({"query": number, "old": old_intent.get(0), "new": new_intent.get(0),
                       "same_kind_other_label": same_kind,
                       "labels_differ_only_in_case": same_kind && old_intent.get(1).and_then(Value::as_str).map(str::to_lowercase)
                           == new_intent.get(1).and_then(Value::as_str).map(str::to_lowercase)}),
            );
            if !same_kind {
                continue;
            }
        }
        let (old_set, new_set) = (shelves.old_set(&old_items), shelves.new_set(&ids));
        if old_set != new_set {
            notes.note(
                "set",
                json!({"query": number, "kind": new_intent.get(0), "old": old_set.len(), "new": new_set.len(),
                       "only_old": old_set.difference(&new_set).count(), "only_new": new_set.difference(&old_set).count()}),
            );
        }
    }
    let mut out = notes.json();
    out["queries"] = json!(names.len());
    out["new_kinds"] = json!(kinds);
    out["old_ms_total"] = json!(old_ms);
    out["new_ms_total"] = json!(new_ms);
    Ok(out)
}

/// 過去の改稿も探す検索。問い合わせは過去の改稿の本文の一部から作る。
fn compare_history(
    shelves: &Shelves,
    conn: &rusqlite::Connection,
    new_root: &Path,
) -> Result<Value, Error> {
    let rows: Vec<(i64, Vec<u8>)> = conn
        .prepare("SELECT doc_key,work_id FROM search_history_documents ORDER BY doc_key")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let keys: Vec<i64> = rows.iter().map(|(key, _)| *key).collect();
    let storage = new_root.join("derived").join("history");
    // 過去の改稿にだけある語（今の本文にない、文字と数字だけの断片）を問い合わせにする。
    // 区切りの記号を含む断片は全文索引の語にならず、旧棚の過去の版の照合（文字列の包含）
    // とだけ一致して、仕組みの違いしか見えなくなる。
    let mut queries = Vec::new();
    let mut punctuation_queries = Vec::new();
    for (key, work) in &rows {
        let Some(doc) = tantivy_index::load_document(&storage, *key)? else {
            continue;
        };
        let current = shelves
            .new
            .search_document(LibraryId::from_slice(work).expect("ID"))?
            .map(|doc| format!("{}\n{}\n{}", doc.title, doc.excerpt, doc.body))
            .unwrap_or_default();
        // 改稿の差は小さいことが多いので、本文を細かく刻み、題と説明も候補にする。
        let mut taken = 0;
        'fields: for text in [&doc.body, &doc.title, &doc.excerpt] {
            let length = text.chars().count();
            let steps = length.clamp(1, 400);
            for step in 0..steps {
                let at = length * step / steps;
                let Some(piece) = chars_at(text, at, 3) else {
                    continue;
                };
                if current.contains(&piece) {
                    continue;
                }
                if !piece.chars().all(char::is_alphanumeric) {
                    punctuation_queries.push(piece);
                    continue;
                }
                queries.push(piece);
                taken += 1;
                if taken == 2 {
                    break 'fields;
                }
            }
        }
    }
    queries.sort();
    queries.dedup();
    punctuation_queries.sort();
    punctuation_queries.dedup();
    let mut notes = Notes::new();
    let (mut old_ms, mut new_ms, mut old_past_only, mut new_past_only) = (0.0, 0.0, 0usize, 0usize);
    let mut version_compared = 0usize;
    let mut raw_history_hits = 0usize;
    for (number, query) in queries.iter().enumerate() {
        let t = Instant::now();
        let (old_items, old_intent) = shelves.old_search(SearchV2Params {
            query: Some(query.clone()),
            version_scope: Some("all".into()),
            ..Default::default()
        })?;
        old_ms += t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let (matches, intent, ids) =
            shelves.new_search(&TextSearch::all_versions(query.clone()))?;
        new_ms += t.elapsed().as_secs_f64() * 1000.0;
        let raw = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sink = raw.clone();
        tantivy_index::visit_matching_download_scores(&storage, query, move |batch| {
            sink.fetch_add(batch.len(), std::sync::atomic::Ordering::Relaxed);
            Ok(())
        })?;
        raw_history_hits += raw.load(std::sync::atomic::Ordering::Relaxed);
        if !old_intent.is_null() || intent.is_some() {
            notes.note("skipped_entity_query", json!({"query": number}));
            continue;
        }
        old_past_only += old_items
            .iter()
            .filter(|i| i.matched_version.is_some())
            .count();
        new_past_only += matches
            .iter()
            .filter(|m| m.matched_version.is_some())
            .count();
        let (old_set, new_set) = (shelves.old_set(&old_items), shelves.new_set(&ids));
        if old_set != new_set {
            // 旧棚にだけある作品について、新しい棚の過去の改稿の文書に問い合わせの文字列が
            // そのまま入っているか（入っていれば索引の引き方、なければ文書の違い）。
            let mut only_old = Vec::new();
            for identity in old_set.difference(&new_set) {
                let work = shelves
                    .new_identity
                    .iter()
                    .find(|(_, i)| *i == identity)
                    .map(|(id, _)| *id);
                let old = old_items
                    .iter()
                    .find(|item| shelves.old_identity.get(&item.id) == Some(identity));
                let mut docs = Vec::new();
                if let Some(work) = work {
                    for (key, owner) in &rows {
                        if owner.as_slice() != work.as_bytes() {
                            continue;
                        }
                        if let Some(doc) = tantivy_index::load_document(&storage, *key)? {
                            docs.push(json!({
                                "body_contains": doc.body.contains(query.as_str()),
                                "title_contains": doc.title.contains(query.as_str()),
                                "body_chars": doc.body.chars().count(),
                            }));
                        }
                    }
                }
                only_old.push(json!({
                    "old_matched_version": old.and_then(|i| i.matched_version),
                    "old_historical": old.map(|i| i.historical_match_count),
                    "new_history_docs": docs,
                }));
            }
            notes.note(
                "set",
                json!({"query": number, "old": old_set.len(), "new": new_set.len(),
                       "only_old": only_old, "only_new": new_set.difference(&old_set).count()}),
            );
            continue;
        }
        let new_by: HashMap<&Identity, &SearchMatch> = matches
            .iter()
            .filter_map(|m| shelves.new_identity.get(&m.work).map(|i| (i, m)))
            .collect();
        for item in &old_items {
            let Some(found) = shelves
                .old_identity
                .get(&item.id)
                .and_then(|i| new_by.get(i))
            else {
                continue;
            };
            version_compared += 1;
            if item.matched_version.is_some() != found.matched_version.is_some() {
                notes.note("current_or_past", json!({"query": number}));
            } else if item.matched_version != found.matched_version {
                notes.note("matched_version", json!({"query": number}));
            }
            if item.historical_match_count as u32 != found.historical_matches {
                notes.note("historical_match_count", json!({"query": number}));
            }
        }
    }
    let mut out = notes.json();
    out["history_documents"] = json!(keys.len());
    out["queries"] = json!(queries.len());
    out["query_texts"] = json!(queries);
    out["works_compared"] = json!(version_compared);
    out["old_past_only_total"] = json!(old_past_only);
    out["new_past_only_total"] = json!(new_past_only);
    out["raw_history_index_hits_total"] = json!(raw_history_hits);
    out["punctuation_pieces_left_out"] = json!(punctuation_queries.len());
    out["old_ms_total"] = json!(old_ms);
    out["new_ms_total"] = json!(new_ms);
    Ok(out)
}

/// 意味検索。新しい棚は旧棚の索引の写しからベクトルを引き継いで作り、同じ問い合わせで近さを比べる。
fn compare_semantic(
    shelves: &Shelves,
    old_storage: &Path,
    conn: &rusqlite::Connection,
) -> Result<Value, Error> {
    let t = Instant::now();
    let report = shelves
        .new
        .sync_semantic_index(Some(old_storage), &|| false, &mut |_, _| {})?;
    let sync_seconds = t.elapsed().as_secs_f64();
    let state = shelves.new.semantic_index_state()?;
    let mut queries: Vec<String> = conn
        .prepare(
            "SELECT DISTINCT t.name FROM snapshot_tags st JOIN tags t ON t.id=st.tag_id
             JOIN work_current_projection p ON p.snapshot_id=st.snapshot_id ORDER BY t.name",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    queries = queries.into_iter().step_by(25).take(40).collect();
    let mut notes = Notes::new();
    let (mut max_delta, mut old_ms, mut new_ms, mut top_same) = (0f64, 0.0, 0.0, 0usize);
    for (number, query) in queries.iter().enumerate() {
        let t = Instant::now();
        let old_hits = semantic_index::search(old_storage, query, usize::MAX)?;
        old_ms += t.elapsed().as_secs_f64() * 1000.0;
        let mut old_scores: Vec<(Identity, f64)> = old_hits
            .iter()
            .filter_map(|hit| {
                shelves
                    .old_identity
                    .get(&hit.download_id)
                    .map(|i| (i.clone(), hit.score))
            })
            .collect();
        let t = Instant::now();
        let (matches, _, _) = shelves.new_search(&TextSearch::meaning(query.clone()))?;
        new_ms += t.elapsed().as_secs_f64() * 1000.0;
        let new_scores: Vec<(Identity, f64)> = matches
            .iter()
            .filter_map(|m| {
                shelves
                    .new_identity
                    .get(&m.work)
                    .map(|i| (i.clone(), m.score))
            })
            .collect();
        old_scores.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let old_map: HashMap<&Identity, f64> = old_scores.iter().map(|(i, s)| (i, *s)).collect();
        let new_map: HashMap<&Identity, f64> = new_scores.iter().map(|(i, s)| (i, *s)).collect();
        let only_old = old_map.keys().filter(|i| !new_map.contains_key(*i)).count();
        let only_new = new_map.keys().filter(|i| !old_map.contains_key(*i)).count();
        if only_old + only_new > 0 {
            notes.note(
                "set",
                json!({"query": number, "old": old_map.len(), "new": new_map.len(), "only_old": only_old, "only_new": only_new}),
            );
        }
        for (identity, score) in &new_map {
            if let Some(old) = old_map.get(identity) {
                max_delta = max_delta.max((old - score).abs());
            }
        }
        let top = |list: &[(Identity, f64)]| {
            list.iter()
                .take(10)
                .map(|(i, _)| i.clone())
                .collect::<Vec<_>>()
        };
        if top(&old_scores) == top(&new_scores) {
            top_same += 1;
        } else {
            notes.note("top10_order", json!({"query": number}));
        }
    }
    let mut out = notes.json();
    out["sync_seconds"] = json!(sync_seconds);
    out["report"] = json!(format!("{report:?}"));
    out["state"] = json!(format!("{state:?}"));
    out["queries"] = json!(queries.len());
    out["top10_same"] = json!(top_same);
    out["max_score_delta"] = json!(max_delta);
    out["old_ms_total"] = json!(old_ms);
    out["new_ms_total"] = json!(new_ms);
    Ok(out)
}

/// 問い合わせの中の絞り込みの書き方（`tag:` `author:` `series:` `source:` `id:`、URL）。
/// 見本の作品から作り、一致する作品の集まりを比べる。
fn compare_structured(shelves: &Shelves, conn: &rusqlite::Connection) -> Result<Value, Error> {
    let works: Vec<Vec<u8>> = conn
        .prepare("SELECT work_id FROM work_current_projection ORDER BY work_id")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut queries: Vec<(&str, String)> = Vec::new();
    for (index, work) in works.iter().enumerate() {
        if index % 6 != 0 {
            continue;
        }
        let Some(doc) = shelves
            .new
            .search_document(LibraryId::from_slice(work).expect("ID"))?
        else {
            continue;
        };
        let quoted = |value: &str| format!("\"{}\"", value.replace('"', ""));
        if let Some(tag) = doc.tags.first() {
            queries.push(("tag", format!("tag:{}", quoted(tag))));
            queries.push((
                "not_tag",
                format!("source:{} -tag:{}", doc.provider, quoted(tag)),
            ));
        }
        queries.push(("author", format!("author:{}", quoted(&doc.author_name))));
        if let Some(series) = doc.series_titles.first() {
            queries.push(("series_title", format!("series:{}", quoted(series))));
        }
        queries.push(("id", format!("id:{}", doc.external_key)));
        queries.push((
            "url",
            piep_lib::library_search::source_url(&doc.provider, &doc.external_key, &doc.author_key),
        ));
        if let Some(piece) = chars_at(&doc.body, doc.body.chars().count() / 2, 2) {
            queries.push((
                "word_and_author",
                format!("{piece} author:{}", quoted(&doc.author_name)),
            ));
        }
    }
    let series_keys: Vec<(String, String)> = conn
        .prepare(
            "SELECT DISTINCT e.provider,e.external_key FROM external_identities e
             JOIN snapshot_series ss ON ss.series_id=e.entity_id
             JOIN work_current_projection p ON p.snapshot_id=ss.snapshot_id
             WHERE e.kind='series' ORDER BY 1,2",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    for (index, (provider, key)) in series_keys.iter().enumerate() {
        if index % 4 == 0 {
            queries.push(("series_key", format!("series:{provider}:{key}")));
        }
    }
    queries.push(("source", "source:pixiv".into()));
    queries.push(("source", "source:fanbox".into()));
    queries.sort();
    queries.dedup();
    let mut notes = Notes::new();
    let mut by_kind: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let (mut old_ms, mut new_ms) = (0.0, 0.0);
    for (number, (kind, query)) in queries.iter().enumerate() {
        let t = Instant::now();
        let (old_items, _) = shelves.old_search(SearchV2Params {
            query: Some(query.clone()),
            ..Default::default()
        })?;
        old_ms += t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let (_, _, ids) = shelves.new_search(&TextSearch::current(query.clone()))?;
        new_ms += t.elapsed().as_secs_f64() * 1000.0;
        let (old_set, new_set) = (shelves.old_set(&old_items), shelves.new_set(&ids));
        let entry = by_kind.entry(kind).or_default();
        entry.0 += 1;
        if old_set != new_set {
            entry.1 += 1;
            notes.note(
                &format!("set_{kind}"),
                json!({"query": number, "text": query, "old": old_set.len(), "new": new_set.len(),
                       "only_old": old_set.difference(&new_set).take(5).collect::<Vec<_>>(),
                       "only_new": new_set.difference(&old_set).take(5).collect::<Vec<_>>()}),
            );
        }
    }
    let mut out = notes.json();
    out["queries"] = json!(queries.len());
    out["by_kind"] = json!(by_kind
        .iter()
        .map(|(kind, (count, differ))| (
            kind.to_string(),
            json!({"queries": count, "differ": differ})
        ))
        .collect::<serde_json::Map<_, _>>());
    out["old_ms_total"] = json!(old_ms);
    out["new_ms_total"] = json!(new_ms);
    Ok(out)
}
