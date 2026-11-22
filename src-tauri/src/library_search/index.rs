//! 本文の検索の索引。`<棚>/derived/` に置く派生物で、旧棚の全文索引の実装
//! （`crate::database::tantivy_index`）をそのまま使う。重み・字句解析・問い合わせの解釈が
//! 旧棚と同じなので、同じ文書からは同じ順位が出る。
//!
//! 索引は二つある。いまの版の索引（`derived/search-index/`、文書は作品ごと）と、過去の版の
//! 索引（`derived/history/search-index/`、文書は改稿ごと）。作り方は同じで、文書の単位と
//! 対応の表だけが違う。
//!
//! 索引の文書は整数の鍵しか持てない。作品・改稿との対応と、文書をどの保存状態から作ったかは
//! 表（`search_documents`・`search_history_documents`）に持つ。同期は「状態の違うものだけ
//! 作り直す」なので、保存・編集・タグの付け替えのあとに呼べば、変わったものだけが入れ替わる。

use super::document::{source_url, SearchDocument};
use crate::database::tantivy_index::{self, Prepared, TantivyIndexDocument};
use crate::library::{Error, Library, LibraryId, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Mutex;

/// 文書の作り方。変えると全作品の文書を作り直す。
pub(super) const RECIPE: i64 = 1;

/// 一度に索引へ確定する文書の数。確定のたびに区切りができ、途中で落ちてもここまでは残る。
const COMMIT_EVERY: usize = 2000;
/// 一人の作り手が一度に受け持つ文書の数。読み取りの接続はこの単位で借り直す
/// （長い読み取りは WAL の折り返しを止める）。
const BATCH: usize = 32;

const MARKER: &str = "library-generation";

/// 同期の結果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchIndexReport {
    /// 索引を空から作り直したか（初回・形式の変更・索引の消失）。
    pub reset: bool,
    pub indexed: usize,
    pub removed: usize,
    /// 文書を作れなかったもの（棚から消えた最中など）。状態は古いまま残り、次に作り直す。
    pub skipped: usize,
    /// 過去の版の索引。数え方は同じで、単位が改稿。
    pub history: HistoryIndexReport,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryIndexReport {
    pub reset: bool,
    pub indexed: usize,
    pub removed: usize,
    pub skipped: usize,
}

/// 一致した作品と関連度。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SearchHit {
    pub work: LibraryId,
    pub score: f32,
}

/// 索引の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Space {
    Current,
    History,
}

impl Space {
    fn table(self) -> &'static str {
        match self {
            Self::Current => "search_documents",
            Self::History => "search_history_documents",
        }
    }

    fn state_table(self) -> &'static str {
        match self {
            Self::Current => "search_index_state",
            Self::History => "search_history_state",
        }
    }

    /// 文書の単位の列。
    fn unit_column(self) -> &'static str {
        match self {
            Self::Current => "work_id",
            Self::History => "revision_id",
        }
    }
}

/// 文書の単位。いまの版なら作品、過去の版なら改稿。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Unit {
    id: LibraryId,
    work: LibraryId,
}

/// 文書を作った時点の保存状態。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    snapshot: Vec<u8>,
    edit: Option<Vec<u8>>,
    user_tags: Option<String>,
}

#[derive(Default)]
struct Counts {
    reset: bool,
    indexed: usize,
    removed: usize,
    skipped: usize,
}

pub(super) const USER_TAGS_SQL: &str = "(SELECT group_concat(name, char(31)) FROM (
    SELECT u.name FROM work_user_tags wt JOIN user_tags u ON u.user_id=wt.user_id AND u.id=wt.tag_id
    WHERE wt.work_id=?1 AND wt.user_id=?2 ORDER BY u.name))";

/// 過去の改稿と、それを表す保存版（改稿の最後の保存版）。いまの版の改稿は含めない。
const PAST_SQL: &str = "SELECT r.id AS revision_id,r.entity_id AS work_id,
    (SELECT s.id FROM revision_snapshots s WHERE s.revision_id=r.id AND s.entity_id=r.entity_id AND s.published=1
     ORDER BY s.sequence DESC LIMIT 1) AS snapshot_id
  FROM work_current_projection p
  JOIN revision_snapshots cs ON cs.id=p.snapshot_id
  JOIN source_revisions r ON r.entity_id=p.work_id AND r.id<>cs.revision_id";

fn library_id(bytes: &[u8]) -> Result<LibraryId> {
    LibraryId::from_slice(bytes).map_err(|_| Error::Integrity("ID が不正です".into()))
}

impl Library {
    /// 索引の置き場（旧棚の関数に渡す `storage_dir`）。
    pub(crate) fn search_storage(&self) -> PathBuf {
        self.root().join("derived")
    }

    pub(super) fn space_storage(&self, space: Space) -> PathBuf {
        match space {
            Space::Current => self.search_storage(),
            Space::History => self.search_storage().join("history"),
        }
    }

    /// 索引を棚に合わせる。状態の違う作品・改稿の文書を作り直し、棚にないものの文書を消す。
    /// いまの版の索引を先に、過去の版の索引をあとに合わせる（進み具合はそれぞれ 0 から数える）。
    pub fn sync_search_index(
        &self,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<SearchIndexReport> {
        let current = self.sync_space(Space::Current, progress)?;
        let history = self.sync_space(Space::History, progress)?;
        Ok(SearchIndexReport {
            reset: current.reset,
            indexed: current.indexed,
            removed: current.removed,
            skipped: current.skipped,
            history: HistoryIndexReport {
                reset: history.reset,
                indexed: history.indexed,
                removed: history.removed,
                skipped: history.skipped,
            },
        })
    }

    fn sync_space(&self, space: Space, progress: &mut dyn FnMut(usize, usize)) -> Result<Counts> {
        let storage = self.space_storage(space);
        let table = space.table();
        let mut report = Counts {
            reset: self.prepare_space(space, &storage)?,
            ..Default::default()
        };

        // 棚にない作品・もう過去でない改稿の文書を消す。行は索引から消したあとで消す。
        let orphans: Vec<i64> = {
            let conn = self.reader()?;
            let sql = match space {
                Space::Current => "SELECT doc_key FROM search_documents d
                    WHERE NOT EXISTS(SELECT 1 FROM library_items li WHERE li.work_id=d.work_id)"
                    .to_owned(),
                Space::History => format!(
                    "SELECT doc_key FROM search_history_documents h
                     WHERE h.revision_id NOT IN (SELECT revision_id FROM ({PAST_SQL}))"
                ),
            };
            let mut statement = conn.prepare(&sql)?;
            let rows = statement
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            rows
        };
        if !orphans.is_empty() {
            tantivy_index::delete_documents(&storage, &orphans).map_err(Error::Integrity)?;
            let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
            let tx = conn.transaction()?;
            for key in &orphans {
                tx.execute(&format!("DELETE FROM {table} WHERE doc_key=?1"), [key])?;
            }
            tx.commit()?;
            report.removed = orphans.len();
        }

        // 状態の違うもの。鍵を先に割り当てる（状態は空のままなので、落ちても古いと分かる）。
        let stale = self.stale_units(space)?;
        if stale.is_empty() {
            return Ok(report);
        }
        let keys: HashMap<LibraryId, i64> = {
            let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
            let tx = conn.transaction()?;
            let mut keys = HashMap::with_capacity(stale.len());
            let unit_column = space.unit_column();
            for unit in &stale {
                match space {
                    Space::Current => tx.execute(
                        "INSERT OR IGNORE INTO search_documents(work_id) VALUES(?1)",
                        [unit.id.as_bytes().as_slice()],
                    )?,
                    Space::History => tx.execute(
                        "INSERT OR IGNORE INTO search_history_documents(revision_id,work_id) VALUES(?1,?2)",
                        [unit.id.as_bytes().as_slice(), unit.work.as_bytes().as_slice()],
                    )?,
                };
                let key: i64 = tx.query_row(
                    &format!("SELECT doc_key FROM {table} WHERE {unit_column}=?1"),
                    [unit.id.as_bytes().as_slice()],
                    |r| r.get(0),
                )?;
                keys.insert(unit.id, key);
            }
            tx.commit()?;
            keys
        };

        let total = stale.len();
        progress(0, total);
        let batches: Vec<&[Unit]> = stale.chunks(BATCH).collect();
        let next = AtomicUsize::new(0);
        let failure: Mutex<Option<Error>> = Mutex::new(None);
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(1, 8);
        let (sender, receiver) = mpsc::sync_channel::<Option<(i64, Prepared, Stamp)>>(256);
        // 作り直しと大きな同期だけ排他の書き手で書き、最後に一つにまとめる。保存のたびの
        // 少しずつの同期は、使い回す書き手で書いて統合を既定の方針に任せる。排他の書き手は
        // 使い終わると捨てるので、少しずつの同期に使うと統合が毎回打ち切られ、セグメントが
        // 保存の数だけたまっていく（`tantivy_index::upsert_prepared`）。
        let small = !report.reset && total < COMMIT_EVERY;
        let mut writer = if small {
            None
        } else {
            Some(tantivy_index::bulk_writer(&storage).map_err(Error::Integrity)?)
        };
        let mut collected: Vec<Prepared> = Vec::new();
        let mut done = 0usize;
        let mut pending: Vec<(i64, Stamp)> = Vec::new();
        std::thread::scope(|scope| -> Result<()> {
            for _ in 0..workers {
                let sender = sender.clone();
                let (batches, next, failure, keys, storage) =
                    (&batches, &next, &failure, &keys, &storage);
                scope.spawn(move || {
                    let run = || -> Result<()> {
                        loop {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some(batch) = batches.get(index) else {
                                return Ok(());
                            };
                            let conn = self.reader()?;
                            for unit in batch.iter() {
                                let key = keys[&unit.id];
                                let item = match self.search_entry(&conn, space, *unit)? {
                                    Some((stamp, doc)) => {
                                        let prepared = tantivy_index::prepare_document(
                                            storage,
                                            &index_document(key, &doc),
                                        )
                                        .map_err(Error::Integrity)?;
                                        Some((key, prepared, stamp))
                                    }
                                    None => None,
                                };
                                if sender.send(item).is_err() {
                                    return Ok(());
                                }
                            }
                        }
                    };
                    if let Err(error) = run() {
                        let mut slot = failure.lock().unwrap_or_else(|e| e.into_inner());
                        slot.get_or_insert(error);
                        // 書き手を止めるために、残りを待たずに抜ける。
                        next.store(usize::MAX / 2, Ordering::Relaxed);
                    }
                });
            }
            drop(sender);
            for item in receiver {
                done += 1;
                match item {
                    Some((key, prepared, stamp)) => {
                        match writer.as_mut() {
                            Some(writer) => writer.upsert(prepared).map_err(Error::Integrity)?,
                            None => collected.push(prepared),
                        }
                        pending.push((key, stamp));
                    }
                    None => report.skipped += 1,
                }
                if let Some(writer) = writer.as_mut().filter(|_| pending.len() >= COMMIT_EVERY) {
                    writer.commit().map_err(Error::Integrity)?;
                    self.record_stamps(space, &mut pending)?;
                }
                if done.is_multiple_of(256) {
                    progress(done, total);
                }
            }
            Ok(())
        })?;
        if let Some(error) = failure.into_inner().unwrap_or_else(|e| e.into_inner()) {
            // 確定していない分は捨てる。状態は古いままなので、次の同期で作り直す。
            if let Some(writer) = writer.as_mut() {
                let _ = writer.rollback();
            }
            return Err(error);
        }
        match writer.as_mut() {
            Some(writer) => writer.commit().map_err(Error::Integrity)?,
            None => {
                tantivy_index::upsert_prepared(&storage, collected).map_err(Error::Integrity)?
            }
        }
        self.record_stamps(space, &mut pending)?;
        report.indexed = total - report.skipped;
        progress(total, total);
        drop(writer);
        // 鍵の対応のない文書（消し損ねや、作り直しの前の残り）を落とす。
        let alive: std::collections::HashSet<i64> = {
            let conn = self.reader()?;
            let mut statement = conn.prepare(&format!("SELECT doc_key FROM {table}"))?;
            let rows = statement
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            rows
        };
        tantivy_index::prune_missing_documents(&storage, &alive).map_err(Error::Integrity)?;
        // まとめて作ったあとは区切りが作り手の数だけでき、問い合わせが区切りごとに回る。
        // 少しずつの同期は既定の統合に任せ、まとめて作ったときだけ一つにまとめる。
        if report.reset || report.indexed >= COMMIT_EVERY {
            tantivy_index::optimize_segments(&storage).map_err(Error::Integrity)?;
        }
        Ok(report)
    }

    /// 索引の形式と世代を確かめ、合わなければ空から作り直す準備をする。作り直すなら `true`。
    fn prepare_space(&self, space: Space, storage: &Path) -> Result<bool> {
        // 旧棚と同じ形式に、新しい棚だけの作り方（タグごとの読み）の版を足す。
        let format = format!("{}+tag-readings-1", tantivy_index::index_format_version());
        let marker_path = storage.join("search-index").join(MARKER);
        let state = space.state_table();
        let recorded: Option<(String, Vec<u8>)> = self
            .reader()?
            .query_row(
                &format!("SELECT format,generation FROM {state} WHERE singleton=1"),
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let marker = std::fs::read(&marker_path).ok();
        if let Some((recorded_format, generation)) = &recorded {
            if *recorded_format == format && marker.as_deref() == Some(generation.as_slice()) {
                return Ok(false);
            }
        }
        // 索引の場所ごと消してから作る。別の世代の文書が残ると、鍵の違うものとして引かれる。
        tantivy_index::release_runtime(storage);
        match std::fs::remove_dir_all(storage.join("search-index")) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        tantivy_index::ensure_index(storage).map_err(Error::Integrity)?;
        let generation = LibraryId::now_v7();
        std::fs::write(&marker_path, generation.as_bytes())?;
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        tx.execute(&format!("DELETE FROM {}", space.table()), [])?;
        tx.execute(
            &format!(
                "INSERT INTO {state}(singleton,format,generation) VALUES(1,?1,?2)
                 ON CONFLICT(singleton) DO UPDATE SET format=excluded.format,generation=excluded.generation"
            ),
            params![format, generation.as_bytes().as_slice()],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// 文書が無いか、作った時点の保存状態が今と違うもの。
    /// いまの版の索引が追いついていない作品の数（診断・ホームの表示）。
    pub fn search_index_pending(&self) -> Result<usize> {
        Ok(self.stale_units(Space::Current)?.len())
    }

    fn stale_units(&self, space: Space) -> Result<Vec<Unit>> {
        let conn = self.reader()?;
        let sql = match space {
            Space::Current => format!(
                "SELECT li.work_id,li.work_id FROM library_items li
                 LEFT JOIN work_current_projection p ON p.work_id=li.work_id
                 LEFT JOIN search_documents d ON d.work_id=li.work_id
                 WHERE d.work_id IS NULL OR d.recipe<>?3 OR d.snapshot_id IS NOT p.snapshot_id
                    OR d.edit_id IS NOT p.edit_id
                    OR d.user_tags IS NOT {}
                 ORDER BY li.work_id",
                USER_TAGS_SQL.replace("?1", "li.work_id")
            ),
            Space::History => format!(
                "SELECT past.revision_id,past.work_id FROM ({PAST_SQL}) past
                 LEFT JOIN search_history_documents h ON h.revision_id=past.revision_id
                 WHERE past.snapshot_id IS NOT NULL
                   AND (h.revision_id IS NULL OR h.recipe<>?3 OR h.snapshot_id IS NOT past.snapshot_id
                        OR h.user_tags IS NOT {})
                 ORDER BY past.revision_id",
                USER_TAGS_SQL.replace("?1", "past.work_id")
            ),
        };
        let mut statement = conn.prepare(&sql)?;
        let rows = statement
            .query_map(
                params![None::<Vec<u8>>, self.id.as_bytes().as_slice(), RECIPE],
                |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?)),
            )?
            .map(|row| {
                let (id, work) = row?;
                Ok(Unit {
                    id: library_id(&id)?,
                    work: library_id(&work)?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(rows)
    }

    /// 一つの文書と、それを作った保存状態。同じ読み取りの中で読むので食い違わない。
    fn search_entry(
        &self,
        conn: &Connection,
        space: Space,
        unit: Unit,
    ) -> Result<Option<(Stamp, SearchDocument)>> {
        let stamp = match space {
            Space::Current => conn
                .query_row(
                    &format!(
                        "SELECT p.snapshot_id,p.edit_id,{USER_TAGS_SQL}
                         FROM work_current_projection p WHERE p.work_id=?1"
                    ),
                    params![
                        unit.work.as_bytes().as_slice(),
                        self.id.as_bytes().as_slice()
                    ],
                    |r| {
                        Ok(Stamp {
                            snapshot: r.get(0)?,
                            edit: r.get(1)?,
                            user_tags: r.get(2)?,
                        })
                    },
                )
                .optional()?,
            Space::History => conn
                .query_row(
                    &format!(
                        "SELECT past.snapshot_id,{USER_TAGS_SQL} FROM ({PAST_SQL}) past
                         WHERE past.revision_id=?3 AND past.snapshot_id IS NOT NULL"
                    ),
                    params![
                        unit.work.as_bytes().as_slice(),
                        self.id.as_bytes().as_slice(),
                        unit.id.as_bytes().as_slice()
                    ],
                    |r| {
                        Ok(Stamp {
                            snapshot: r.get(0)?,
                            edit: None,
                            user_tags: r.get(1)?,
                        })
                    },
                )
                .optional()?,
        };
        let Some(stamp) = stamp else {
            return Ok(None);
        };
        let doc = match space {
            Space::Current => self.search_document_from_conn(conn, unit.work)?,
            Space::History => self.past_search_document(conn, unit.work, &stamp.snapshot)?,
        };
        Ok(doc.map(|doc| (stamp, doc)))
    }

    fn record_stamps(&self, space: Space, pending: &mut Vec<(i64, Stamp)>) -> Result<()> {
        if pending.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        match space {
            Space::Current => {
                let mut update = tx.prepare_cached(
                    "UPDATE search_documents SET snapshot_id=?2,edit_id=?3,user_tags=?4,recipe=?5 WHERE doc_key=?1",
                )?;
                for (key, stamp) in pending.drain(..) {
                    update.execute(params![
                        key,
                        stamp.snapshot,
                        stamp.edit,
                        stamp.user_tags,
                        RECIPE
                    ])?;
                }
            }
            Space::History => {
                let mut update = tx.prepare_cached(
                    "UPDATE search_history_documents SET snapshot_id=?2,user_tags=?3,recipe=?4 WHERE doc_key=?1",
                )?;
                for (key, stamp) in pending.drain(..) {
                    update.execute(params![key, stamp.snapshot, stamp.user_tags, RECIPE])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 問い合わせに一致する索引の文書の鍵と関連度（順は決まらない）。
    pub(super) fn matching_in(&self, space: Space, query: &str) -> Result<Vec<(i64, f32)>> {
        let storage = self.space_storage(space);
        let found: std::sync::Arc<Mutex<Vec<(i64, f32)>>> = Default::default();
        let sink = found.clone();
        tantivy_index::visit_matching_download_scores(&storage, query, move |batch| {
            sink.lock()
                .map_err(|_| "検索の結果を集められません".to_string())?
                .extend_from_slice(batch);
            Ok(())
        })
        .map_err(Error::Integrity)?;
        let found = std::mem::take(&mut *found.lock().map_err(|_| Error::Busy)?);
        Ok(found)
    }

    pub(super) fn matching_documents(&self, query: &str) -> Result<Vec<(i64, f32)>> {
        self.matching_in(Space::Current, query)
    }

    /// 問い合わせに一致する作品を、関連度の高い順に返す（同点は作品の鍵の順）。
    /// 問い合わせの書き方は旧棚と同じ（語の AND、`-` で除外、同義語の展開）。
    pub fn search_text(&self, query: &str) -> Result<Vec<SearchHit>> {
        let found = self.matching_documents(query)?;
        let conn = self.reader()?;
        // 鍵は千件ずつまとめて引く（一件ずつ引くと、一致の多い語で問い合わせより遅くなる）。
        let mut works = conn.prepare_cached(
            "SELECT d.doc_key,d.work_id FROM json_each(?1) j JOIN search_documents d ON d.doc_key=j.value",
        )?;
        let mut hits = Vec::with_capacity(found.len());
        for chunk in found.chunks(1000) {
            let scores: HashMap<i64, f32> = chunk.iter().copied().collect();
            let keys = serde_json::to_string(&scores.keys().collect::<Vec<_>>())?;
            let mut rows = works.query([keys])?;
            // 対応のない鍵は、同期の途中か消し損ねの文書。棚の作品ではないので出さない。
            while let Some(row) = rows.next()? {
                let key: i64 = row.get(0)?;
                let bytes: Vec<u8> = row.get(1)?;
                hits.push((
                    key,
                    SearchHit {
                        work: library_id(&bytes)?,
                        score: scores[&key],
                    },
                ));
            }
        }
        hits.sort_by(|(a_key, a), (b_key, b)| b.score.total_cmp(&a.score).then(a_key.cmp(b_key)));
        Ok(hits.into_iter().map(|(_, hit)| hit).collect())
    }
}

#[cfg(test)]
#[path = "index_tests.rs"]
mod tests;

/// 旧棚の索引に入れる形。
fn index_document(key: i64, doc: &SearchDocument) -> TantivyIndexDocument {
    let date = |at: i64| {
        chrono::DateTime::from_timestamp_millis(at)
            .map(|at| at.to_rfc3339())
            .unwrap_or_default()
    };
    TantivyIndexDocument {
        download_id: key,
        source: doc.provider.clone(),
        source_id: doc.external_key.clone(),
        source_url: source_url(&doc.provider, &doc.external_key, &doc.author_key),
        title: doc.title.clone(),
        author_name: doc.author_name.clone(),
        author_id: doc.author_key.clone(),
        tags: doc.tags.join(" "),
        tag_list: doc.tags.clone(),
        series_title: doc.series_titles.join(" "),
        excerpt: doc.excerpt.clone(),
        body: doc.body.clone(),
        published_at: doc.published_at.map(date).unwrap_or_default(),
        downloaded_at: date(doc.saved_at),
        // お気に入りと更新の監視は一覧の絞り込みが持つ。索引に写すと、押すたびに作り直しになる。
        favorite: false,
        watch_updates: false,
        asset_kinds: crate::database::search::normalize_search_text(&doc.asset_kinds.join(" ")),
        text_length: doc.text_length,
    }
}
