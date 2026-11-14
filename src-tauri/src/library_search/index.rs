//! 本文の検索の索引。`<棚>/derived/search-index/` に置く派生物で、旧棚の全文索引の実装
//! （`crate::database::tantivy_index`）をそのまま使う。重み・字句解析・問い合わせの解釈が
//! 旧棚と同じなので、同じ文書からは同じ順位が出る。
//!
//! 索引の文書は整数の鍵しか持てない。作品との対応と、文書をどの保存状態から作ったかは
//! `search_documents` に持つ。同期は「状態の違う作品だけ作り直す」なので、保存・編集・
//! タグの付け替えのあとに呼べば、変わった作品だけが入れ替わる。

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
/// 一人の作り手が一度に受け持つ作品の数。読み取りの接続はこの単位で借り直す
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
    /// 文書を作れなかった作品（棚から消えた最中など）。状態は古いまま残り、次に作り直す。
    pub skipped: usize,
}

/// 一致した作品と関連度。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SearchHit {
    pub work: LibraryId,
    pub score: f32,
}

/// 文書を作った時点の保存状態。
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    snapshot: Vec<u8>,
    edit: Option<Vec<u8>>,
    user_tags: Option<String>,
}

const USER_TAGS_SQL: &str = "(SELECT group_concat(name, char(31)) FROM (
    SELECT u.name FROM work_user_tags wt JOIN user_tags u ON u.user_id=wt.user_id AND u.id=wt.tag_id
    WHERE wt.work_id=?1 AND wt.user_id=?2 ORDER BY u.name))";

impl Library {
    /// 索引の置き場（旧棚の関数に渡す `storage_dir`）。
    fn search_storage(&self) -> PathBuf {
        self.root().join("derived")
    }

    /// 索引を棚に合わせる。状態の違う作品の文書を作り直し、棚にない作品の文書を消す。
    pub fn sync_search_index(
        &self,
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<SearchIndexReport> {
        let storage = self.search_storage();
        let mut report = SearchIndexReport {
            reset: self.prepare_search_index(&storage)?,
            ..Default::default()
        };

        // 棚にない作品の文書を消す。行は索引から消したあとで消す。
        let orphans: Vec<i64> = {
            let conn = self.reader()?;
            let mut statement = conn.prepare(
                "SELECT doc_key FROM search_documents d
                 WHERE NOT EXISTS(SELECT 1 FROM library_items li WHERE li.work_id=d.work_id)",
            )?;
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
                tx.execute("DELETE FROM search_documents WHERE doc_key=?1", [key])?;
            }
            tx.commit()?;
            report.removed = orphans.len();
        }

        // 状態の違う作品。鍵を先に割り当てる（状態は空のままなので、落ちても古いと分かる）。
        let stale = self.stale_search_works()?;
        if stale.is_empty() {
            return Ok(report);
        }
        let keys: HashMap<LibraryId, i64> = {
            let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
            let tx = conn.transaction()?;
            let mut keys = HashMap::with_capacity(stale.len());
            for work in &stale {
                tx.execute(
                    "INSERT OR IGNORE INTO search_documents(work_id) VALUES(?1)",
                    [work.as_bytes().as_slice()],
                )?;
                let key: i64 = tx.query_row(
                    "SELECT doc_key FROM search_documents WHERE work_id=?1",
                    [work.as_bytes().as_slice()],
                    |r| r.get(0),
                )?;
                keys.insert(*work, key);
            }
            tx.commit()?;
            keys
        };

        let total = stale.len();
        progress(0, total);
        let batches: Vec<&[LibraryId]> = stale.chunks(BATCH).collect();
        let next = AtomicUsize::new(0);
        let failure: Mutex<Option<Error>> = Mutex::new(None);
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(1, 8);
        let (sender, receiver) = mpsc::sync_channel::<Option<(i64, Prepared, Stamp)>>(256);
        let mut writer = tantivy_index::bulk_writer(&storage).map_err(Error::Integrity)?;
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
                            for work in batch.iter() {
                                let key = keys[work];
                                let item = match self.search_entry(&conn, *work)? {
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
                        writer.upsert(prepared).map_err(Error::Integrity)?;
                        pending.push((key, stamp));
                    }
                    None => report.skipped += 1,
                }
                if pending.len() >= COMMIT_EVERY {
                    writer.commit().map_err(Error::Integrity)?;
                    self.record_search_stamps(&mut pending)?;
                }
                if done.is_multiple_of(256) {
                    progress(done, total);
                }
            }
            Ok(())
        })?;
        if let Some(error) = failure.into_inner().unwrap_or_else(|e| e.into_inner()) {
            // 確定していない分は捨てる。状態は古いままなので、次の同期で作り直す。
            let _ = writer.rollback();
            return Err(error);
        }
        writer.commit().map_err(Error::Integrity)?;
        self.record_search_stamps(&mut pending)?;
        report.indexed = total - report.skipped;
        progress(total, total);
        drop(writer);
        // 鍵の対応のない文書（消し損ねや、作り直しの前の残り）を落とす。
        let alive: std::collections::HashSet<i64> = {
            let conn = self.reader()?;
            let mut statement = conn.prepare("SELECT doc_key FROM search_documents")?;
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
    fn prepare_search_index(&self, storage: &Path) -> Result<bool> {
        let format = tantivy_index::index_format_version();
        let marker_path = storage.join("search-index").join(MARKER);
        let recorded: Option<(String, Vec<u8>)> = self
            .reader()?
            .query_row(
                "SELECT format,generation FROM search_index_state WHERE singleton=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let marker = std::fs::read(&marker_path).ok();
        if let Some((recorded_format, generation)) = &recorded {
            if recorded_format == format && marker.as_deref() == Some(generation.as_slice()) {
                return Ok(false);
            }
        }
        // 索引の場所ごと消してから作る。別の世代の文書が残ると、鍵の違う作品として引かれる。
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
        tx.execute("DELETE FROM search_documents", [])?;
        tx.execute(
            "INSERT INTO search_index_state(singleton,format,generation) VALUES(1,?1,?2)
             ON CONFLICT(singleton) DO UPDATE SET format=excluded.format,generation=excluded.generation",
            params![format, generation.as_bytes().as_slice()],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// 文書が無いか、作った時点の保存状態が今と違う作品。
    fn stale_search_works(&self) -> Result<Vec<LibraryId>> {
        let conn = self.reader()?;
        let mut statement = conn.prepare(&format!(
            "SELECT li.work_id FROM library_items li
             LEFT JOIN work_current_projection p ON p.work_id=li.work_id
             LEFT JOIN search_documents d ON d.work_id=li.work_id
             WHERE d.work_id IS NULL OR d.recipe<>?3 OR d.snapshot_id IS NOT p.snapshot_id
                OR d.edit_id IS NOT p.edit_id
                OR d.user_tags IS NOT {}
             ORDER BY li.work_id",
            USER_TAGS_SQL.replace("?1", "li.work_id")
        ))?;
        let rows = statement
            .query_map(
                params![None::<Vec<u8>>, self.id.as_bytes().as_slice(), RECIPE],
                |r| r.get::<_, Vec<u8>>(0),
            )?
            .map(|row| {
                let bytes = row?;
                LibraryId::from_slice(&bytes).map_err(|_| Error::Integrity("ID が不正です".into()))
            })
            .collect::<Result<_>>()?;
        Ok(rows)
    }

    /// 一作品の文書と、それを作った保存状態。同じ読み取りの中で読むので食い違わない。
    fn search_entry(
        &self,
        conn: &Connection,
        work: LibraryId,
    ) -> Result<Option<(Stamp, SearchDocument)>> {
        let key = work.as_bytes().to_vec();
        let stamp = conn
            .query_row(
                &format!(
                    "SELECT p.snapshot_id,p.edit_id,{USER_TAGS_SQL}
                     FROM work_current_projection p WHERE p.work_id=?1"
                ),
                params![key, self.id.as_bytes().as_slice()],
                |r| {
                    Ok(Stamp {
                        snapshot: r.get(0)?,
                        edit: r.get(1)?,
                        user_tags: r.get(2)?,
                    })
                },
            )
            .optional()?;
        let Some(stamp) = stamp else {
            return Ok(None);
        };
        Ok(self
            .search_document_from_conn(conn, work)?
            .map(|doc| (stamp, doc)))
    }

    fn record_search_stamps(&self, pending: &mut Vec<(i64, Stamp)>) -> Result<()> {
        if pending.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        {
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
        tx.commit()?;
        Ok(())
    }

    /// 問い合わせに一致する作品を、関連度の高い順に返す（同点は作品の鍵の順）。
    /// 問い合わせの書き方は旧棚と同じ（語の AND、`-` で除外、同義語の展開）。
    pub fn search_text(&self, query: &str) -> Result<Vec<SearchHit>> {
        let storage = self.search_storage();
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
                let work = LibraryId::from_slice(&bytes)
                    .map_err(|_| Error::Integrity("ID が不正です".into()))?;
                hits.push((
                    key,
                    SearchHit {
                        work,
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
