//! 意味検索。意味検索の索引（`crate::database::semantic_index`、作品ごとに説明と本文の
//! ベクトルを一つずつ持つ）を使い、`<棚>/derived/search/semantic-v2/` に置く。
//!
//! 埋め込みは遅い（本文まるごとで 1 秒に 1.5 作品ほど、全棚で 5 時間）。そこで作り直すときは、
//! 同じ索引の前のベクトルのうち文字列が変わっていない部分を写す（タグを付け替えただけなら、
//! 説明の一断片だけを埋め込む）。

use super::document::SearchDocument;
use crate::database::semantic_index::{self, ReusableVectors, SemanticIndexDocument};
use crate::library::{Error, Library, LibraryId, Result};
use rusqlite::{params, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Mutex};

/// 文書の作り方。本文の検索の文書と同じものから作るので、変えるときは一緒に上げる。
const RECIPE: i64 = super::index::RECIPE;

/// 一度に埋め込む作品の数。中止の合図はこの単位で見る（押してから止まるまでを
/// 短くするために小さくする）。
const EMBED_BATCH: usize = 16;

/// 意味検索の索引の同期の結果。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SemanticIndexReport {
    pub indexed: usize,
    pub removed: usize,
    /// 文書を作れなかった作品、またはベクトルの向きがなく入れられなかった作品。次にまた試す。
    pub skipped: usize,
    /// 写した説明・本文のベクトルと、埋め込んだ断片の数。
    pub reused_metadata: usize,
    pub reused_content: usize,
    pub embedded_passages: usize,
    /// 途中で止めたか。止めるまでに入れた作品は残る。
    pub canceled: bool,
}

/// 意味検索の索引の状態。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SemanticIndexState {
    /// 今の保存状態でベクトルのある作品。
    pub indexed: u64,
    pub works: u64,
}

/// 意味検索で一致した作品の、近さとその根拠。
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SemanticEvidence {
    /// 近かったのが本文（`content`）か説明（`metadata`）か。
    pub field: String,
    /// 根拠として見せる文。本文の近さは本文全体の傾向なので、箇所は示さない。
    pub text: String,
    pub score: f64,
}

impl Library {
    /// 意味検索の索引の置き場（`semantic_index` の関数に渡す `storage_dir`）。索引そのものはこの親の
    /// `search/semantic-v2/` にできるので、`derived/search/semantic-v2/` になる。
    pub(crate) fn semantic_storage(&self) -> PathBuf {
        self.search_storage().join("semantic")
    }

    /// 意味検索の索引を棚に合わせる。状態の違う作品のベクトルを作り直し、棚にない作品の
    /// ベクトルを消す。モデルを読み込むので、意味検索を有効にしたときだけ呼ぶ。
    pub fn sync_semantic_index(
        &self,
        should_cancel: &(dyn Fn() -> bool + Sync),
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<SemanticIndexReport> {
        let storage = self.semantic_storage();
        let mut report = SemanticIndexReport::default();
        if !semantic_index::index_exists(&storage) {
            // 記録だけ残っていても、ベクトルはない。
            let conn = self.conn.lock().map_err(|_| Error::Busy)?;
            conn.execute("DELETE FROM semantic_documents", [])?;
        }

        let orphans: Vec<i64> = {
            let conn = self.reader()?;
            let mut statement = conn.prepare(
                "SELECT doc_key FROM semantic_documents d
                 WHERE NOT EXISTS(SELECT 1 FROM library_items li WHERE li.work_id=d.work_id)",
            )?;
            let rows = statement
                .query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            rows
        };
        if !orphans.is_empty() {
            semantic_index::clear_documents(&storage, &orphans).map_err(Error::Integrity)?;
            let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
            let tx = conn.transaction()?;
            for key in &orphans {
                tx.execute("DELETE FROM semantic_documents WHERE doc_key=?1", [key])?;
            }
            tx.commit()?;
            report.removed = orphans.len();
        }

        // 状態の違う作品と、写せる前のベクトル。同じ索引に前の行があれば
        // それを写す候補にする。
        let stale = self.stale_semantic_works()?;
        if stale.is_empty() {
            return Ok(report);
        }
        let mut units = Vec::with_capacity(stale.len());
        {
            let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
            let tx = conn.transaction()?;
            for work in &stale {
                let existing: Option<(i64, i64)> = tx
                    .query_row(
                        "SELECT doc_key,recipe FROM semantic_documents WHERE work_id=?1",
                        [work.as_bytes().as_slice()],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                let (key, own) = match existing {
                    Some((key, recipe)) => (key, recipe > 0),
                    None => {
                        tx.execute(
                            "INSERT INTO semantic_documents(work_id) VALUES(?1)",
                            [work.as_bytes().as_slice()],
                        )?;
                        (tx.last_insert_rowid(), false)
                    }
                };
                units.push(Unit {
                    work: *work,
                    key,
                    reuse: own,
                });
            }
            tx.commit()?;
        }

        let total = units.len();
        progress(0, total);
        // 文書づくりは作り手に分け、埋め込みと書き込みは一人で順に行う。
        let next = AtomicUsize::new(0);
        let stop = AtomicBool::new(false);
        let failure: Mutex<Option<Error>> = Mutex::new(None);
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .clamp(1, 8);
        let (sender, receiver) = mpsc::sync_channel::<(Unit, Option<Built>)>(EMBED_BATCH * 8);
        let mut done = 0usize;
        std::thread::scope(|scope| -> Result<()> {
            for _ in 0..workers {
                let sender = sender.clone();
                let (units, next, stop, failure) = (&units, &next, &stop, &failure);
                scope.spawn(move || {
                    let run = || -> Result<()> {
                        let mut conn = None;
                        loop {
                            if stop.load(Ordering::Relaxed) {
                                return Ok(());
                            }
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some(unit) = units.get(index) else {
                                return Ok(());
                            };
                            // 読み取りの接続は数十件ごとに借り直す（長い読み取りは WAL を止める）。
                            if index % 32 == 0 {
                                conn = None;
                            }
                            if conn.is_none() {
                                conn = Some(self.reader()?);
                            }
                            let built = self.semantic_entry(conn.as_ref().unwrap(), unit)?;
                            if sender.send((*unit, built)).is_err() {
                                return Ok(());
                            }
                        }
                    };
                    if let Err(error) = run() {
                        let mut slot = failure.lock().unwrap_or_else(|e| e.into_inner());
                        slot.get_or_insert(error);
                        stop.store(true, Ordering::Relaxed);
                    }
                });
            }
            drop(sender);
            let mut batch: Vec<(Unit, Built)> = Vec::with_capacity(EMBED_BATCH);
            let flush =
                |batch: &mut Vec<(Unit, Built)>, report: &mut SemanticIndexReport| -> Result<()> {
                    if batch.is_empty() {
                        return Ok(());
                    }
                    let result = self.embed_batch(&storage, batch, report);
                    batch.clear();
                    result
                };
            for (unit, built) in receiver.iter() {
                if stop.load(Ordering::Relaxed) {
                    continue;
                }
                done += 1;
                match built {
                    Some(built) => batch.push((unit, built)),
                    None => report.skipped += 1,
                }
                if batch.len() >= EMBED_BATCH {
                    if should_cancel() {
                        report.canceled = true;
                        stop.store(true, Ordering::Relaxed);
                        batch.clear();
                        continue;
                    }
                    if let Err(error) = flush(&mut batch, &mut report) {
                        failure
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .get_or_insert(error);
                        stop.store(true, Ordering::Relaxed);
                        continue;
                    }
                    progress(done, total);
                }
            }
            if !stop.load(Ordering::Relaxed) {
                if should_cancel() {
                    report.canceled = true;
                } else {
                    flush(&mut batch, &mut report)?;
                }
            }
            Ok(())
        })?;
        if let Some(error) = failure.into_inner().unwrap_or_else(|e| e.into_inner()) {
            return Err(error);
        }
        if !report.canceled {
            progress(total, total);
        }
        Ok(report)
    }

    /// ベクトルが無いか、作った時点の保存状態が今と違う作品。
    fn stale_semantic_works(&self) -> Result<Vec<LibraryId>> {
        let conn = self.reader()?;
        let mut statement = conn.prepare(&format!(
            "SELECT li.work_id FROM library_items li
             LEFT JOIN work_current_projection p ON p.work_id=li.work_id
             LEFT JOIN semantic_documents d ON d.work_id=li.work_id
             WHERE d.work_id IS NULL OR d.recipe<>?3 OR d.snapshot_id IS NOT p.snapshot_id
                OR d.edit_id IS NOT p.edit_id
                OR d.user_tags IS NOT {}
             ORDER BY li.work_id",
            super::index::USER_TAGS_SQL.replace("?1", "li.work_id")
        ))?;
        let rows = statement
            .query_map(
                params![None::<Vec<u8>>, self.id.as_bytes().as_slice(), RECIPE],
                |r| r.get::<_, Vec<u8>>(0),
            )?
            .map(|row| {
                LibraryId::from_slice(&row?).map_err(|_| Error::Integrity("ID が不正です".into()))
            })
            .collect::<Result<_>>()?;
        Ok(rows)
    }

    fn semantic_entry(&self, conn: &rusqlite::Connection, unit: &Unit) -> Result<Option<Built>> {
        let stamp = conn
            .query_row(
                &format!(
                    "SELECT p.snapshot_id,p.edit_id,{}
                     FROM work_current_projection p WHERE p.work_id=?1",
                    super::index::USER_TAGS_SQL
                ),
                params![
                    unit.work.as_bytes().as_slice(),
                    self.id.as_bytes().as_slice()
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some(stamp) = stamp else {
            return Ok(None);
        };
        Ok(self
            .search_document_from_conn(conn, unit.work)?
            .map(|doc| Built {
                document: semantic_document(unit.key, doc),
                stamp,
            }))
    }

    /// 一まとまりを埋め込んで入れ、入った作品の状態を書く。前のベクトルはこの索引から写す。
    fn embed_batch(
        &self,
        storage: &Path,
        batch: &[(Unit, Built)],
        report: &mut SemanticIndexReport,
    ) -> Result<()> {
        let docs: Vec<SemanticIndexDocument> = batch
            .iter()
            .map(|(_, built)| built.document.clone())
            .collect();
        let previous_ids: HashMap<i64, i64> = batch
            .iter()
            .filter(|(unit, _)| unit.reuse)
            .map(|(unit, _)| (unit.key, unit.key))
            .collect();
        let result = semantic_index::upsert_documents_reusing(
            storage,
            &docs,
            Some(ReusableVectors {
                storage_dir: storage,
                previous_ids: &previous_ids,
            }),
        )
        .map_err(Error::Integrity)?;
        report.reused_metadata += result.reused_metadata;
        report.reused_content += result.reused_content;
        report.embedded_passages += result.embedded_passages;
        let indexed: HashSet<i64> = result.indexed.into_iter().collect();
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        {
            let mut update = tx.prepare_cached(
                "UPDATE semantic_documents SET snapshot_id=?2,edit_id=?3,user_tags=?4,recipe=?5 WHERE doc_key=?1",
            )?;
            for (unit, built) in batch {
                if indexed.contains(&unit.key) {
                    let (snapshot, edit, user_tags) = &built.stamp;
                    update.execute(params![unit.key, snapshot, edit, user_tags, RECIPE])?;
                    report.indexed += 1;
                } else {
                    report.skipped += 1;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 意味検索の索引を空にする（意味検索を切ったとき。計算もディスクも使わない）。
    pub fn clear_semantic_index(&self) -> Result<()> {
        let storage = self.semantic_storage();
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        if semantic_index::index_exists(&storage) {
            semantic_index::clear_all_works(&storage).map_err(Error::Integrity)?;
        }
        conn.execute("DELETE FROM semantic_documents", [])?;
        Ok(())
    }

    /// 意味検索の索引の状態。
    pub fn semantic_index_state(&self) -> Result<SemanticIndexState> {
        let conn = self.reader()?;
        let works: i64 = conn.query_row("SELECT count(*) FROM library_items", [], |r| r.get(0))?;
        let indexed: i64 = if semantic_index::index_exists(&self.semantic_storage()) {
            conn.query_row(
                "SELECT count(*) FROM semantic_documents d JOIN work_current_projection p ON p.work_id=d.work_id
                 WHERE d.recipe=?1 AND d.snapshot_id IS p.snapshot_id AND d.edit_id IS p.edit_id",
                [RECIPE],
                |r| r.get(0),
            )?
        } else {
            0
        };
        Ok(SemanticIndexState {
            indexed: indexed as u64,
            works: works as u64,
        })
    }

    /// 意味の近い作品を、本文の検索の索引の文書の鍵と近さで返す（順は決まらない）。
    /// 近さが下限に届かない作品は入らない。
    pub(super) fn semantic_matches(&self, text: &str) -> Result<Vec<(i64, SemanticEvidence)>> {
        let storage = self.semantic_storage();
        if !semantic_index::index_exists(&storage) {
            return Err(Error::ResourceUnavailable);
        }
        let hits = semantic_index::search(&storage, text, usize::MAX).map_err(Error::Integrity)?;
        let by_key: HashMap<i64, semantic_index::SemanticSearchHit> =
            hits.into_iter().map(|hit| (hit.download_id, hit)).collect();
        let conn = self.reader()?;
        let mut rows = conn.prepare_cached(
            "SELECT s.doc_key,d.doc_key FROM json_each(?1) j
             JOIN semantic_documents s ON s.doc_key=j.value
             JOIN search_documents d ON d.work_id=s.work_id",
        )?;
        let keys: Vec<i64> = by_key.keys().copied().collect();
        let mut found = Vec::with_capacity(keys.len());
        for chunk in keys.chunks(1000) {
            let mut result = rows.query([serde_json::to_string(chunk)?])?;
            while let Some(row) = result.next()? {
                let hit = &by_key[&row.get::<_, i64>(0)?];
                found.push((
                    row.get::<_, i64>(1)?,
                    SemanticEvidence {
                        field: hit.field.clone(),
                        text: hit.text.clone(),
                        score: hit.score,
                    },
                ));
            }
        }
        Ok(found)
    }
}

#[cfg(test)]
#[path = "semantic_tests.rs"]
mod tests;

/// 埋め込む一作品。
#[derive(Clone, Copy, Debug)]
struct Unit {
    work: LibraryId,
    key: i64,
    /// この索引に前のベクトルがあり、変わっていない部分を写せるか。
    reuse: bool,
}

struct Built {
    document: SemanticIndexDocument,
    stamp: (Vec<u8>, Option<Vec<u8>>, Option<String>),
}

/// 意味検索の文書の形。説明の断片の文字列（題・作者・タグ・シリーズ・説明）は
/// ここで決まり、前と同じなら前のベクトルを写せる。
fn semantic_document(key: i64, doc: SearchDocument) -> SemanticIndexDocument {
    SemanticIndexDocument {
        download_id: key,
        title: doc.title,
        author_name: doc.author_name,
        tags: doc.tags.join(" "),
        series_title: doc.series_titles.join(" "),
        excerpt: doc.excerpt,
        body: doc.body,
    }
}
