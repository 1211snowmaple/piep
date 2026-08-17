use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, OnceLock};

use parking_lot::{Condvar, Mutex};
use rusqlite::{Connection, OpenFlags};
use tantivy::collector::{Count, TopDocs};
use tantivy::query::QueryParser;
use tantivy::schema::Value as _;
use tantivy::schema::{Field, Schema, FAST, INDEXED, STORED, STRING, TEXT};
use tantivy::tokenizer::{LowerCaser, NgramTokenizer, RemoveLongFilter, TextAnalyzer};
use tantivy::{Index, IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term};

use url::Url;

use super::search::{parse_search_query, ParsedSearchQuery, SearchDocument, SearchTerm};
use super::search_normalization::{
    expand_query_for_tantivy, index_text, normalize_for_search, query_variants,
};

const INDEX_DIR_NAME: &str = "search-index";
// v4 drops the body field that duplicated the body n-grams verbatim, stops
// storing the derived forms that are only ever matched against, and no longer
// folds the readings into the n-gram field. Existing v3 directories are left
// alone; the app rebuilds into the new one.
const INDEX_VERSION_DIR: &str = "v4";
const TOKENIZER_NAME: &str = "default";

static RUNTIMES: OnceLock<Mutex<HashMap<PathBuf, Arc<TantivyRuntime>>>> = OnceLock::new();

struct TantivyRuntime {
    index: Index,
    reader: IndexReader,
    fields: TantivyFields,
    writer_state: Mutex<WriterState>,
    writer_available: Condvar,
    content_generation: AtomicU64,
}

struct WriterState {
    ordinary: Option<IndexWriter<TantivyDocument>>,
    exclusive: bool,
}

#[derive(Debug, Clone)]
pub struct TantivyIndexDocument {
    pub download_id: i64,
    pub source: String,
    pub source_id: String,
    pub source_url: String,
    pub title: String,
    pub author_name: String,
    pub author_id: String,
    pub tags: String,
    pub series_title: String,
    pub excerpt: String,
    pub body: String,
    pub published_at: String,
    pub downloaded_at: String,
    pub favorite: bool,
    pub watch_updates: bool,
    pub asset_kinds: String,
    pub text_length: i64,
}

#[derive(Debug, Clone)]
pub struct TantivySearchHit {
    pub download_id: i64,
    pub score: f32,
    pub document: Arc<SearchDocument>,
}

#[derive(Debug, Clone, Default)]
pub struct TantivySearchResult {
    pub hits: Vec<TantivySearchHit>,
    pub total_hits: usize,
}

#[derive(Clone, Copy)]
struct TantivyFields {
    download_id: Field,
    source_exact: Field,
    source_id_exact: Field,
    source_id_lower: Field,
    source_id_ngram: Field,
    source_id_reading_kana: Field,
    source_id_reading_romaji: Field,
    source_url_exact: Field,
    source_url_lower: Field,
    source_url_ngram: Field,
    source_url_reading_kana: Field,
    source_url_reading_romaji: Field,
    title_exact: Field,
    title_lower: Field,
    title_ngram: Field,
    title_reading_kana: Field,
    title_reading_romaji: Field,
    author_name_exact: Field,
    author_name_lower: Field,
    author_name_ngram: Field,
    author_name_reading_kana: Field,
    author_name_reading_romaji: Field,
    author_id_exact: Field,
    author_id_lower: Field,
    author_id_ngram: Field,
    author_id_reading_kana: Field,
    author_id_reading_romaji: Field,
    tags_exact: Field,
    tags_lower: Field,
    tags_ngram: Field,
    tags_reading_kana: Field,
    tags_reading_romaji: Field,
    series_title_exact: Field,
    series_title_lower: Field,
    series_title_ngram: Field,
    series_title_reading_kana: Field,
    series_title_reading_romaji: Field,
    excerpt_exact: Field,
    excerpt_lower: Field,
    excerpt_ngram: Field,
    excerpt_reading_kana: Field,
    excerpt_reading_romaji: Field,
    body_exact: Field,
    body_ngram: Field,
    body_reading_kana: Field,
    body_reading_romaji: Field,
    published_at: Field,
    downloaded_at: Field,
    favorite: Field,
    watch_updates: Field,
    asset_kinds: Field,
    text_length: Field,
}

/// The on-disk index format. Bookkeeping in SQLite is keyed to this, so that a
/// format change forces a rebuild instead of leaving the app searching an index
/// that no longer holds anything.
pub fn index_format_version() -> &'static str {
    INDEX_VERSION_DIR
}

/// Removes index directories left behind by older formats.
///
/// The index is entirely derived from the library, so an obsolete copy is dead
/// weight - and at several times the size of the current one, it is worth
/// reclaiming rather than leaving on disk forever.
fn remove_obsolete_index_versions(index_root: &Path) {
    let Ok(entries) = std::fs::read_dir(index_root) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().to_string();
        let is_version_dir = name.len() > 1
            && name.starts_with('v')
            && name[1..].chars().all(|c| c.is_ascii_digit());
        if !is_version_dir || name == INDEX_VERSION_DIR {
            continue;
        }
        if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
            match std::fs::remove_dir_all(entry.path()) {
                Ok(()) => log::info!("Removed obsolete search index format {name}"),
                // Windows can hold an mmap open; a later run collects it.
                Err(error) => log::warn!("Could not remove obsolete search index {name}: {error}"),
            }
        }
    }
}

pub fn ensure_index(storage_dir: &Path) -> Result<Index, String> {
    let index_root = storage_dir.join(INDEX_DIR_NAME);
    let index_dir = index_root.join(INDEX_VERSION_DIR);
    std::fs::create_dir_all(&index_dir)
        .map_err(|e| format!("Tantivy index dir creation failed: {}", e))?;
    remove_obsolete_index_versions(&index_root);

    match Index::open_in_dir(&index_dir) {
        Ok(index) => {
            register_tokenizer(&index)?;
            if schema_has_required_fields(&index.schema()) {
                Ok(index)
            } else {
                reset_index_dir(&index_dir)
            }
        }
        Err(_) => reset_index_dir(&index_dir),
    }
}

fn reset_index_dir(index_dir: &Path) -> Result<Index, String> {
    if index_dir.exists() {
        let _ = std::fs::remove_dir_all(index_dir);
    }
    std::fs::create_dir_all(index_dir).map_err(|e| format!("Tantivy index reset failed: {}", e))?;
    let index = Index::create_in_dir(index_dir, build_schema())
        .map_err(|e| format!("Tantivy index creation failed: {}", e))?;
    register_tokenizer(&index)?;
    Ok(index)
}

fn runtime(storage_dir: &Path) -> Result<Arc<TantivyRuntime>, String> {
    let key = storage_dir.join(INDEX_DIR_NAME).join(INDEX_VERSION_DIR);
    let runtimes = RUNTIMES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(runtime) = runtimes.lock().get(&key).cloned() {
        return Ok(runtime);
    }

    let index = ensure_index(storage_dir)?;
    let schema = index.schema();
    let fields = fields(&schema)?;
    let reader = index
        .reader_builder()
        .reload_policy(ReloadPolicy::OnCommitWithDelay)
        .try_into()
        .map_err(|e| format!("Tantivy reader creation failed: {}", e))?;
    let runtime = Arc::new(TantivyRuntime {
        index,
        reader,
        fields,
        writer_state: Mutex::new(WriterState {
            ordinary: None,
            exclusive: false,
        }),
        writer_available: Condvar::new(),
        content_generation: AtomicU64::new(1),
    });
    runtimes.lock().insert(key, runtime.clone());
    Ok(runtime)
}

pub fn upsert_documents(storage_dir: &Path, docs: &[TantivyIndexDocument]) -> Result<(), String> {
    if docs.is_empty() {
        return Ok(());
    }
    let runtime = runtime(storage_dir)?;
    with_ordinary_writer(&runtime, |writer| {
        for doc in docs {
            writer.delete_term(Term::from_field_u64(
                runtime.fields.download_id,
                doc.download_id as u64,
            ));
            writer
                .add_document(tantivy_document(&runtime.fields, doc))
                .map_err(|e| format!("Tantivy document insert failed: {e}"))?;
        }
        writer
            .commit()
            .map_err(|e| format!("Tantivy commit failed: {e}"))?;
        runtime
            .content_generation
            .fetch_add(1, AtomicOrdering::AcqRel);
        runtime
            .reader
            .reload()
            .map_err(|e| format!("Tantivy reader reload failed: {e}"))?;
        Ok(())
    })
}

const ORDINARY_WRITER_MEMORY_BUDGET: usize = 64_000_000;

fn with_ordinary_writer<T>(
    runtime: &Arc<TantivyRuntime>,
    operation: impl FnOnce(&mut IndexWriter<TantivyDocument>) -> Result<T, String>,
) -> Result<T, String> {
    let mut state = runtime.writer_state.lock();
    while state.exclusive {
        runtime.writer_available.wait(&mut state);
    }
    if state.ordinary.is_none() {
        state.ordinary = Some(
            runtime
                .index
                .writer(ORDINARY_WRITER_MEMORY_BUDGET)
                .map_err(|e| format!("Tantivy writer creation failed: {e}"))?,
        );
    }
    let result = operation(state.ordinary.as_mut().expect("writer initialized"));
    if result.is_err() {
        // A failed add/commit may leave uncommitted state. Dropping the writer
        // is the safest rollback boundary; the next operation recreates it.
        state.ordinary.take();
    }
    result
}

#[cfg(test)]
pub(crate) fn ordinary_writer_is_cached(storage_dir: &Path) -> Result<bool, String> {
    let runtime = runtime(storage_dir)?;
    let is_cached = runtime.writer_state.lock().ordinary.is_some();
    Ok(is_cached)
}

fn exclusive_writer(
    runtime: &Arc<TantivyRuntime>,
    create: impl FnOnce() -> Result<IndexWriter<TantivyDocument>, String>,
) -> Result<ExclusiveWriter, String> {
    let mut state = runtime.writer_state.lock();
    while state.exclusive {
        runtime.writer_available.wait(&mut state);
    }
    state.exclusive = true;
    // Tantivy permits one IndexWriter per directory. Release the cached
    // ordinary writer before creating a rebuild/merge writer.
    state.ordinary.take();
    let writer = match create() {
        Ok(writer) => writer,
        Err(error) => {
            state.exclusive = false;
            runtime.writer_available.notify_all();
            return Err(error);
        }
    };
    drop(state);
    Ok(ExclusiveWriter {
        runtime: runtime.clone(),
        writer: Some(writer),
    })
}

struct ExclusiveWriter {
    runtime: Arc<TantivyRuntime>,
    writer: Option<IndexWriter<TantivyDocument>>,
}

impl std::ops::Deref for ExclusiveWriter {
    type Target = IndexWriter<TantivyDocument>;

    fn deref(&self) -> &Self::Target {
        self.writer.as_ref().expect("exclusive writer active")
    }
}

impl std::ops::DerefMut for ExclusiveWriter {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.writer.as_mut().expect("exclusive writer active")
    }
}

impl Drop for ExclusiveWriter {
    fn drop(&mut self) {
        // Drop the Tantivy filesystem lock before waking an ordinary writer.
        self.writer.take();
        let mut state = self.runtime.writer_state.lock();
        state.exclusive = false;
        self.runtime.writer_available.notify_all();
    }
}

/// Turns a source document into its indexable form without touching the index.
///
/// This is the expensive half of indexing - morphological analysis of the whole
/// body - and it is pure, so a rebuild can run it across every core and hand
/// the writer nothing but finished documents.
pub fn prepare_document(
    storage_dir: &Path,
    doc: &TantivyIndexDocument,
) -> Result<Prepared, String> {
    let runtime = runtime(storage_dir)?;
    Ok(Prepared {
        download_id: doc.download_id,
        document: tantivy_document(&runtime.fields, doc),
    })
}

pub struct Prepared {
    download_id: i64,
    document: TantivyDocument,
}

/// A writer held open for the length of a rebuild.
///
/// Creating one costs a thread pool and a memory arena, and every commit ends a
/// segment, so doing both once per small batch was most of the cost of a
/// rebuild and left the index split into hundreds of segments.
pub struct BulkWriter {
    runtime: Arc<TantivyRuntime>,
    writer: ExclusiveWriter,
    uncommitted: usize,
}

pub fn bulk_writer(storage_dir: &Path) -> Result<BulkWriter, String> {
    let runtime = runtime(storage_dir)?;
    let memory_budget = super::resource_budget::tantivy_writer_bytes();
    // Keep at least 48 MiB per indexing worker. The old fixed 900 MB arena
    // could exhaust a small machine before a single document was committed.
    let memory_threads = (memory_budget / (48 * 1024 * 1024)).max(1);
    let threads = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4)
        .clamp(1, 8)
        .min(memory_threads);
    let writer = exclusive_writer(&runtime, || {
        runtime
            .index
            .writer_with_num_threads(threads, memory_budget)
            .map_err(|e| format!("Tantivy bulk writer creation failed: {e}"))
    })?;
    Ok(BulkWriter {
        runtime,
        writer,
        uncommitted: 0,
    })
}

impl BulkWriter {
    pub fn upsert(&mut self, prepared: Prepared) -> Result<(), String> {
        self.writer.delete_term(Term::from_field_u64(
            self.runtime.fields.download_id,
            prepared.download_id as u64,
        ));
        self.writer
            .add_document(prepared.document)
            .map_err(|e| format!("Tantivy document insert failed: {e}"))?;
        self.uncommitted += 1;
        Ok(())
    }

    pub fn uncommitted(&self) -> usize {
        self.uncommitted
    }

    pub fn commit(&mut self) -> Result<(), String> {
        if self.uncommitted == 0 {
            return Ok(());
        }
        self.writer
            .commit()
            .map_err(|e| format!("Tantivy commit failed: {e}"))?;
        self.runtime
            .content_generation
            .fetch_add(1, AtomicOrdering::AcqRel);
        self.runtime
            .reader
            .reload()
            .map_err(|e| format!("Tantivy reader reload failed: {e}"))?;
        self.uncommitted = 0;
        Ok(())
    }

    /// Abandons everything added since the last commit. Used when a rebuild is
    /// cancelled, so the index keeps the last consistent state instead of a
    /// half-written batch.
    pub fn rollback(&mut self) -> Result<(), String> {
        self.writer
            .rollback()
            .map_err(|e| format!("Tantivy rollback failed: {e}"))?;
        self.uncommitted = 0;
        Ok(())
    }
}

pub fn delete_document(storage_dir: &Path, download_id: i64) -> Result<(), String> {
    delete_documents(storage_dir, &[download_id])
}

/// Deletes a group of works with one writer commit and one reader reload.
/// Tantivy commits are intentionally expensive durability boundaries, so they
/// must not be paid once per item by bulk library operations.
pub fn delete_documents(storage_dir: &Path, download_ids: &[i64]) -> Result<(), String> {
    if download_ids.is_empty() {
        return Ok(());
    }

    let runtime = runtime(storage_dir)?;
    with_ordinary_writer(&runtime, |writer| {
        for download_id in download_ids {
            writer.delete_term(Term::from_field_u64(
                runtime.fields.download_id,
                *download_id as u64,
            ));
        }
        writer
            .commit()
            .map_err(|e| format!("Tantivy delete commit failed: {e}"))?;
        runtime
            .content_generation
            .fetch_add(1, AtomicOrdering::AcqRel);
        runtime
            .reader
            .reload()
            .map_err(|e| format!("Tantivy reader reload failed: {e}"))?;
        Ok(())
    })
}

/// Deletes every id stored in a disk-backed bulk-selection snapshot while
/// holding one Tantivy writer. This avoids materializing an O(N) id vector and
/// still pays only one commit/reload boundary.
pub fn delete_documents_from_snapshot(
    storage_dir: &Path,
    snapshot_path: &Path,
) -> Result<(), String> {
    let selection = Connection::open_with_flags(
        snapshot_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("Bulk selection snapshot open failed: {e}"))?;
    let runtime = runtime(storage_dir)?;
    with_ordinary_writer(&runtime, |writer| {
        let mut statement = selection
            .prepare("SELECT id FROM bulk_matches ORDER BY id")
            .map_err(|e| format!("Bulk lexical delete prepare failed: {e}"))?;
        let rows = statement
            .query_map([], |row| row.get::<_, i64>(0))
            .map_err(|e| format!("Bulk lexical delete query failed: {e}"))?;
        for row in rows {
            let download_id =
                row.map_err(|e| format!("Bulk lexical delete id read failed: {e}"))?;
            if download_id <= 0 {
                return Err(format!(
                    "Bulk lexical delete snapshot contained invalid id {download_id}"
                ));
            }
            writer.delete_term(Term::from_field_u64(
                runtime.fields.download_id,
                download_id as u64,
            ));
        }
        writer
            .commit()
            .map_err(|e| format!("Tantivy bulk delete commit failed: {e}"))?;
        runtime
            .content_generation
            .fetch_add(1, AtomicOrdering::AcqRel);
        runtime
            .reader
            .reload()
            .map_err(|e| format!("Tantivy reader reload failed: {e}"))?;
        Ok(())
    })
}

pub fn searchable_segment_count(storage_dir: &Path) -> Result<usize, String> {
    let runtime = runtime(storage_dir)?;
    runtime
        .index
        .searchable_segment_ids()
        .map(|segments| segments.len())
        .map_err(|e| format!("Tantivy segment inspection failed: {e}"))
}

/// Identifies logical index content for query-result caches. Segment-only
/// merges deliberately preserve this value because stable download-id ranking
/// remains valid across a changed physical layout.
pub fn index_generation(storage_dir: &Path) -> Result<u64, String> {
    let runtime = runtime(storage_dir)?;
    Ok(runtime.content_generation.load(AtomicOrdering::Acquire))
}

/// Atomically merges every searchable segment into one segment. Tantivy keeps
/// the old segment set authoritative until the new one is fully written and
/// its meta file is committed, so an interruption cannot replace a valid index
/// with a partial merge. Unreferenced files are collected only afterwards.
pub fn optimize_segments(storage_dir: &Path) -> Result<(usize, usize), String> {
    let runtime = runtime(storage_dir)?;
    let segment_ids = runtime
        .index
        .searchable_segment_ids()
        .map_err(|e| format!("Tantivy segment inspection failed: {e}"))?;
    let before = segment_ids.len();
    let mut writer = exclusive_writer(&runtime, || {
        runtime
            .index
            .writer(64_000_000)
            .map_err(|e| format!("Tantivy optimization writer creation failed: {e}"))
    })?;
    writer.set_merge_policy(Box::new(tantivy::merge_policy::NoMergePolicy));
    if before <= 1 {
        // A prior merge can leave obsolete files temporarily undeletable on
        // Windows while another reader still owns an mmap. Even when no merge
        // is needed, an explicit optimization must retry that safe cleanup.
        if let Err(error) = writer.garbage_collect_files().wait() {
            log::warn!("Tantivy obsolete file collection deferred: {error}");
        }
        return Ok((before, before));
    }

    writer
        .merge(&segment_ids)
        .wait()
        .map_err(|e| format!("Tantivy segment merge failed: {e}"))?;
    runtime
        .reader
        .reload()
        .map_err(|e| format!("Tantivy reader reload after merge failed: {e}"))?;
    if let Err(error) = writer.garbage_collect_files().wait() {
        // The merged index is already committed and searchable. On Windows an
        // in-flight reader may keep an obsolete mmap alive; a later writer or
        // optimization run can safely retry collecting those files.
        log::warn!("Tantivy obsolete file collection deferred: {error}");
    }
    let after = runtime
        .index
        .searchable_segment_ids()
        .map_err(|e| format!("Tantivy post-merge inspection failed: {e}"))?
        .len();
    Ok((before, after))
}

/// Returns the current top candidates together with the complete number of
/// lexical matches. Callers can grow `limit` as a relevance cursor advances,
/// instead of silently making everything below a fixed top-N unreachable.
pub fn search_with_total(
    storage_dir: &Path,
    query: &str,
    limit: usize,
) -> Result<TantivySearchResult, String> {
    if query.trim().is_empty() || limit == 0 {
        return Ok(TantivySearchResult::default());
    }

    let runtime = runtime(storage_dir)?;
    let fields = runtime.fields;
    let searcher = runtime.reader.searcher();
    let mut parser = QueryParser::for_index(&runtime.index, search_fields(&fields));
    configure_field_boosts(&mut parser, &fields);
    let parsed_query = parse_search_query(query);
    let Some(query_expr) = tantivy_query(&parsed_query) else {
        return Ok(TantivySearchResult::default());
    };
    let expanded_query = expand_query_for_tantivy(query);
    let parsed = parser
        .parse_query(&query_expr)
        .or_else(|_| parser.parse_query(&escape_query(&expanded_query)))
        .map_err(|e| format!("Tantivy query parse failed: {}", e))?;

    let (top_docs, total_hits) = searcher
        .search(
            &parsed,
            &(TopDocs::with_limit(limit).order_by_score(), Count),
        )
        .map_err(|e| format!("Tantivy search failed: {}", e))?;

    let mut hits = Vec::with_capacity(top_docs.len());
    for (score, address) in top_docs {
        let retrieved = searcher
            .doc::<TantivyDocument>(address)
            .map_err(|e| format!("Tantivy document read failed: {}", e))?;
        if let Some(download_id) = retrieved
            .get_first(fields.download_id)
            .and_then(|value| value.as_u64())
            .map(|value| value as i64)
        {
            hits.push(TantivySearchHit {
                download_id,
                score,
                document: Arc::new(search_document_from_tantivy(&retrieved, &fields)),
            });
        }
    }

    Ok(TantivySearchResult { hits, total_hits })
}

const MATCH_ID_BATCH_SIZE: usize = 512;

/// Streams every matching id to a bounded callback batch.
///
/// This is used by column-sorted full-text search. Keeping the callback in the
/// collector avoids constructing both a million-element Vec and its much
/// larger JSON representation before SQLite can start filtering.
pub fn visit_matching_download_ids<F>(
    storage_dir: &Path,
    query: &str,
    visitor: F,
) -> Result<usize, String>
where
    F: Fn(&[i64]) -> Result<(), String> + Send + Sync + 'static,
{
    if query.trim().is_empty() {
        return Ok(0);
    }
    let runtime = runtime(storage_dir)?;
    let searcher = runtime.reader.searcher();
    let mut parser = QueryParser::for_index(&runtime.index, search_fields(&runtime.fields));
    configure_field_boosts(&mut parser, &runtime.fields);
    let parsed_query = parse_search_query(query);
    let Some(query_expr) = tantivy_query(&parsed_query) else {
        return Ok(0);
    };
    let expanded_query = expand_query_for_tantivy(query);
    let parsed = parser
        .parse_query(&query_expr)
        .or_else(|_| parser.parse_query(&escape_query(&expanded_query)))
        .map_err(|e| format!("Tantivy query parse failed: {e}"))?;

    searcher
        .search(
            &parsed,
            &StreamingDownloadIdCollector {
                visitor: Arc::new(visitor),
                cancelled: Arc::new(AtomicBool::new(false)),
            },
        )
        .map_err(|e| format!("Tantivy id collection failed: {e}"))?
}

const MATCH_SCORE_BATCH_SIZE: usize = 256;

/// Streams every lexical match with its BM25 score. Segment collectors emit
/// bounded, unsorted batches; callers persist them and add the final
/// `(score DESC, stable download_id ASC)` ordering on disk.
pub fn visit_matching_download_scores<F>(
    storage_dir: &Path,
    query: &str,
    visitor: F,
) -> Result<usize, String>
where
    F: Fn(&[(i64, f32)]) -> Result<(), String> + Send + Sync + 'static,
{
    if query.trim().is_empty() {
        return Ok(0);
    }
    let runtime = runtime(storage_dir)?;
    let searcher = runtime.reader.searcher();
    let mut parser = QueryParser::for_index(&runtime.index, search_fields(&runtime.fields));
    configure_field_boosts(&mut parser, &runtime.fields);
    let parsed_query = parse_search_query(query);
    let Some(query_expr) = tantivy_query(&parsed_query) else {
        return Ok(0);
    };
    let expanded_query = expand_query_for_tantivy(query);
    let parsed = parser
        .parse_query(&query_expr)
        .or_else(|_| parser.parse_query(&escape_query(&expanded_query)))
        .map_err(|e| format!("Tantivy query parse failed: {e}"))?;

    searcher
        .search(
            &parsed,
            &StreamingDownloadScoreCollector {
                visitor: Arc::new(visitor),
                cancelled: Arc::new(AtomicBool::new(false)),
            },
        )
        .map_err(|e| format!("Tantivy score collection failed: {e}"))?
}

struct StreamingDownloadScoreCollector<F> {
    visitor: Arc<F>,
    cancelled: Arc<AtomicBool>,
}

impl<F> tantivy::collector::Collector for StreamingDownloadScoreCollector<F>
where
    F: Fn(&[(i64, f32)]) -> Result<(), String> + Send + Sync + 'static,
{
    type Fruit = Result<usize, String>;
    type Child = StreamingDownloadScoreSegmentCollector<F>;

    fn for_segment(
        &self,
        _segment_local_id: tantivy::SegmentOrdinal,
        segment: &tantivy::SegmentReader,
    ) -> tantivy::Result<Self::Child> {
        Ok(StreamingDownloadScoreSegmentCollector {
            column: segment.fast_fields().u64("download_id")?,
            visitor: self.visitor.clone(),
            cancelled: self.cancelled.clone(),
            scores: Vec::with_capacity(MATCH_SCORE_BATCH_SIZE),
            visited: 0,
            error: None,
        })
    }

    fn requires_scoring(&self) -> bool {
        true
    }

    fn merge_fruits(
        &self,
        segment_fruits: Vec<Result<usize, String>>,
    ) -> tantivy::Result<Self::Fruit> {
        let mut total = 0usize;
        for fruit in segment_fruits {
            match fruit {
                Ok(count) => total = total.saturating_add(count),
                Err(error) => return Ok(Err(error)),
            }
        }
        Ok(Ok(total))
    }
}

struct StreamingDownloadScoreSegmentCollector<F> {
    column: tantivy::fastfield::Column<u64>,
    visitor: Arc<F>,
    cancelled: Arc<AtomicBool>,
    scores: Vec<(i64, f32)>,
    visited: usize,
    error: Option<String>,
}

impl<F> StreamingDownloadScoreSegmentCollector<F>
where
    F: Fn(&[(i64, f32)]) -> Result<(), String> + Send + Sync + 'static,
{
    fn flush(&mut self) {
        if self.scores.is_empty()
            || self.error.is_some()
            || self.cancelled.load(AtomicOrdering::Acquire)
        {
            return;
        }
        if let Err(error) = (self.visitor)(&self.scores) {
            self.cancelled.store(true, AtomicOrdering::Release);
            self.error = Some(error);
            return;
        }
        self.visited = self.visited.saturating_add(self.scores.len());
        self.scores.clear();
    }
}

impl<F> tantivy::collector::SegmentCollector for StreamingDownloadScoreSegmentCollector<F>
where
    F: Fn(&[(i64, f32)]) -> Result<(), String> + Send + Sync + 'static,
{
    type Fruit = Result<usize, String>;

    fn collect(&mut self, doc: tantivy::DocId, score: tantivy::Score) {
        if self.error.is_some() || self.cancelled.load(AtomicOrdering::Acquire) {
            return;
        }
        if !score.is_finite() {
            self.cancelled.store(true, AtomicOrdering::Release);
            self.error = Some("Tantivy returned a non-finite search score".to_string());
            return;
        }
        let Some(value) = self.column.first(doc) else {
            self.cancelled.store(true, AtomicOrdering::Release);
            self.error = Some("Tantivy match was missing its download id".to_string());
            return;
        };
        let Ok(download_id) = i64::try_from(value) else {
            self.cancelled.store(true, AtomicOrdering::Release);
            self.error = Some(format!(
                "Tantivy match id {value} exceeds SQLite's integer range"
            ));
            return;
        };
        self.scores.push((download_id, score));
        if self.scores.len() == MATCH_SCORE_BATCH_SIZE {
            self.flush();
        }
    }

    fn harvest(mut self) -> Result<usize, String> {
        self.flush();
        self.error.map_or(Ok(self.visited), Err)
    }
}

/// Returns the download id of every document matching `query`.
///
/// Relevance paging cannot answer "the same matches, ordered by title", so a
/// sorted search needs the whole match set handed to SQL, which owns the
/// ordering. Ids come from the fast field, so this never touches stored data.
#[cfg(test)]
pub fn matching_download_ids(storage_dir: &Path, query: &str) -> Result<Vec<i64>, String> {
    let ids = Arc::new(Mutex::new(Vec::new()));
    let sink = ids.clone();
    visit_matching_download_ids(storage_dir, query, move |batch| {
        sink.lock().extend_from_slice(batch);
        Ok(())
    })?;
    let result = std::mem::take(&mut *ids.lock());
    Ok(result)
}

struct StreamingDownloadIdCollector<F> {
    visitor: Arc<F>,
    cancelled: Arc<AtomicBool>,
}

impl<F> tantivy::collector::Collector for StreamingDownloadIdCollector<F>
where
    F: Fn(&[i64]) -> Result<(), String> + Send + Sync + 'static,
{
    type Fruit = Result<usize, String>;
    type Child = StreamingDownloadIdSegmentCollector<F>;

    fn for_segment(
        &self,
        _segment_local_id: tantivy::SegmentOrdinal,
        segment: &tantivy::SegmentReader,
    ) -> tantivy::Result<Self::Child> {
        Ok(StreamingDownloadIdSegmentCollector {
            column: segment.fast_fields().u64("download_id")?,
            visitor: self.visitor.clone(),
            cancelled: self.cancelled.clone(),
            ids: Vec::with_capacity(MATCH_ID_BATCH_SIZE),
            visited: 0,
            error: None,
        })
    }

    fn requires_scoring(&self) -> bool {
        false
    }

    fn merge_fruits(
        &self,
        segment_fruits: Vec<Result<usize, String>>,
    ) -> tantivy::Result<Self::Fruit> {
        let mut total = 0usize;
        for fruit in segment_fruits {
            match fruit {
                Ok(count) => total = total.saturating_add(count),
                Err(error) => return Ok(Err(error)),
            }
        }
        Ok(Ok(total))
    }
}

struct StreamingDownloadIdSegmentCollector<F> {
    column: tantivy::fastfield::Column<u64>,
    visitor: Arc<F>,
    cancelled: Arc<AtomicBool>,
    ids: Vec<i64>,
    visited: usize,
    error: Option<String>,
}

impl<F> StreamingDownloadIdSegmentCollector<F>
where
    F: Fn(&[i64]) -> Result<(), String> + Send + Sync + 'static,
{
    fn flush(&mut self) {
        if self.ids.is_empty()
            || self.error.is_some()
            || self.cancelled.load(AtomicOrdering::Acquire)
        {
            return;
        }
        if let Err(error) = (self.visitor)(&self.ids) {
            self.cancelled.store(true, AtomicOrdering::Release);
            self.error = Some(error);
            return;
        }
        self.visited = self.visited.saturating_add(self.ids.len());
        self.ids.clear();
    }
}

impl<F> tantivy::collector::SegmentCollector for StreamingDownloadIdSegmentCollector<F>
where
    F: Fn(&[i64]) -> Result<(), String> + Send + Sync + 'static,
{
    type Fruit = Result<usize, String>;

    fn collect(&mut self, doc: tantivy::DocId, _score: tantivy::Score) {
        if self.error.is_some() || self.cancelled.load(AtomicOrdering::Acquire) {
            return;
        }
        let Some(value) = self.column.first(doc) else {
            self.cancelled.store(true, AtomicOrdering::Release);
            self.error = Some("Tantivy match was missing its download id".to_string());
            return;
        };
        let Ok(download_id) = i64::try_from(value) else {
            self.cancelled.store(true, AtomicOrdering::Release);
            self.error = Some(format!(
                "Tantivy match id {value} exceeds SQLite's integer range"
            ));
            return;
        };
        self.ids.push(download_id);
        if self.ids.len() == MATCH_ID_BATCH_SIZE {
            self.flush();
        }
    }

    fn harvest(mut self) -> Result<usize, String> {
        self.flush();
        self.error.map_or(Ok(self.visited), Err)
    }
}

fn search_fields(fields: &TantivyFields) -> Vec<Field> {
    vec![
        fields.title_exact,
        fields.title_lower,
        fields.title_ngram,
        fields.title_reading_kana,
        fields.title_reading_romaji,
        fields.author_name_exact,
        fields.author_name_lower,
        fields.author_name_ngram,
        fields.author_name_reading_kana,
        fields.author_name_reading_romaji,
        fields.author_id_exact,
        fields.author_id_lower,
        fields.author_id_ngram,
        fields.author_id_reading_kana,
        fields.author_id_reading_romaji,
        fields.source_id_exact,
        fields.source_id_lower,
        fields.source_id_ngram,
        fields.source_id_reading_kana,
        fields.source_id_reading_romaji,
        fields.source_url_exact,
        fields.source_url_lower,
        fields.source_url_ngram,
        fields.source_url_reading_kana,
        fields.source_url_reading_romaji,
        fields.tags_exact,
        fields.tags_lower,
        fields.tags_ngram,
        fields.tags_reading_kana,
        fields.tags_reading_romaji,
        fields.series_title_exact,
        fields.series_title_lower,
        fields.series_title_ngram,
        fields.series_title_reading_kana,
        fields.series_title_reading_romaji,
        fields.excerpt_exact,
        fields.excerpt_lower,
        fields.excerpt_ngram,
        fields.excerpt_reading_kana,
        fields.excerpt_reading_romaji,
        fields.body_ngram,
        fields.body_reading_kana,
        fields.body_reading_romaji,
        fields.asset_kinds,
    ]
}

fn configure_field_boosts(parser: &mut QueryParser, fields: &TantivyFields) {
    for (field, boost) in [
        (fields.source_id_exact, 10.0),
        (fields.source_id_lower, 9.0),
        (fields.source_id_ngram, 8.0),
        (fields.source_url_exact, 9.0),
        (fields.source_url_lower, 8.0),
        (fields.source_url_ngram, 7.0),
        (fields.title_exact, 12.0),
        (fields.title_lower, 11.0),
        (fields.title_ngram, 10.0),
        (fields.title_reading_kana, 9.0),
        (fields.title_reading_romaji, 8.5),
        (fields.author_name_exact, 10.0),
        (fields.author_name_lower, 9.5),
        (fields.author_name_ngram, 8.5),
        (fields.author_name_reading_kana, 8.0),
        (fields.author_name_reading_romaji, 7.5),
        (fields.author_id_exact, 9.0),
        (fields.author_id_lower, 8.0),
        (fields.author_id_ngram, 7.0),
        (fields.tags_exact, 9.0),
        (fields.tags_lower, 8.5),
        (fields.tags_ngram, 8.0),
        (fields.tags_reading_kana, 7.5),
        (fields.tags_reading_romaji, 7.0),
        (fields.series_title_exact, 8.0),
        (fields.series_title_lower, 7.5),
        (fields.series_title_ngram, 7.0),
        (fields.series_title_reading_kana, 6.5),
        (fields.series_title_reading_romaji, 6.0),
        (fields.excerpt_exact, 4.0),
        (fields.excerpt_lower, 3.8),
        (fields.excerpt_ngram, 3.5),
        (fields.excerpt_reading_kana, 3.2),
        (fields.excerpt_reading_romaji, 3.0),
        (fields.body_ngram, 1.0),
        (fields.body_reading_kana, 0.9),
        (fields.body_reading_romaji, 0.8),
        (fields.asset_kinds, 1.0),
    ] {
        parser.set_field_boost(field, boost);
    }
}

fn tantivy_query(parsed: &ParsedSearchQuery) -> Option<String> {
    if parsed.include.is_empty() {
        return None;
    }

    let mut parts = parsed
        .include
        .iter()
        .filter_map(term_query_group)
        .collect::<Vec<_>>();
    if parts.is_empty() {
        return None;
    }

    for term in &parsed.exclude {
        if let Some(group) = term_query_group(term) {
            parts.push(format!("-{}", group));
        }
    }

    Some(parts.join(" AND "))
}

fn term_query_group(term: &SearchTerm) -> Option<String> {
    let mut variants = Vec::new();
    for variant in &term.variants {
        push_query_variant(&mut variants, variant);
    }
    for synonym in &term.synonyms {
        for variant in query_variants(synonym) {
            push_query_variant(&mut variants, &variant);
        }
    }

    if variants.is_empty() {
        None
    } else if variants.len() == 1 {
        Some(variants.remove(0))
    } else {
        Some(format!("({})", variants.join(" OR ")))
    }
}

fn push_query_variant(values: &mut Vec<String>, value: &str) {
    let value = query_literal(value);
    if value.is_empty() || values.iter().any(|existing| existing == &value) {
        return;
    }
    values.push(value);
}

fn query_literal(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return String::new();
    }
    if value.chars().any(char::is_whitespace) {
        return format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""));
    }

    let mut out = String::new();
    for c in value.chars() {
        if matches!(
            c,
            '+' | '^'
                | '`'
                | '{'
                | '}'
                | '['
                | ']'
                | '"'
                | '~'
                | '*'
                | '?'
                | ':'
                | '\\'
                | '/'
                | '('
                | ')'
                | '-'
                | '!'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

pub fn load_document(
    storage_dir: &Path,
    download_id: i64,
) -> Result<Option<SearchDocument>, String> {
    let runtime = runtime(storage_dir)?;
    let fields = runtime.fields;
    let searcher = runtime.reader.searcher();
    let term = Term::from_field_u64(fields.download_id, download_id as u64);
    let query = tantivy::query::TermQuery::new(term, tantivy::schema::IndexRecordOption::Basic);
    let top_docs = searcher
        .search(
            &query as &dyn tantivy::query::Query,
            &TopDocs::with_limit(1).order_by_score(),
        )
        .map_err(|e| format!("Tantivy document lookup failed: {}", e))?;
    let Some((_, address)) = top_docs.into_iter().next() else {
        return Ok(None);
    };
    let retrieved = searcher
        .doc::<TantivyDocument>(address)
        .map_err(|e| format!("Tantivy document read failed: {}", e))?;

    Ok(Some(search_document_from_tantivy(&retrieved, &fields)))
}

fn search_document_from_tantivy(
    retrieved: &TantivyDocument,
    fields: &TantivyFields,
) -> SearchDocument {
    SearchDocument {
        title: text_value(retrieved, fields.title_exact),
        author_name: text_value(retrieved, fields.author_name_exact),
        tags: text_value(retrieved, fields.tags_exact),
        series_title: text_value(retrieved, fields.series_title_exact),
        excerpt: text_value(retrieved, fields.excerpt_exact),
        body: text_value(retrieved, fields.body_exact),
    }
}

fn tantivy_document(fields: &TantivyFields, doc: &TantivyIndexDocument) -> TantivyDocument {
    let mut tantivy_doc = TantivyDocument::default();
    tantivy_doc.add_u64(fields.download_id, doc.download_id as u64);
    tantivy_doc.add_text(fields.source_exact, &doc.source);
    add_search_text(
        &mut tantivy_doc,
        fields.source_id_exact,
        fields.source_id_lower,
        fields.source_id_ngram,
        fields.source_id_reading_kana,
        fields.source_id_reading_romaji,
        &doc.source_id,
    );
    add_url_text(&mut tantivy_doc, fields, &doc.source_url);
    add_search_text(
        &mut tantivy_doc,
        fields.title_exact,
        fields.title_lower,
        fields.title_ngram,
        fields.title_reading_kana,
        fields.title_reading_romaji,
        &doc.title,
    );
    add_search_text(
        &mut tantivy_doc,
        fields.author_name_exact,
        fields.author_name_lower,
        fields.author_name_ngram,
        fields.author_name_reading_kana,
        fields.author_name_reading_romaji,
        &doc.author_name,
    );
    add_search_text(
        &mut tantivy_doc,
        fields.author_id_exact,
        fields.author_id_lower,
        fields.author_id_ngram,
        fields.author_id_reading_kana,
        fields.author_id_reading_romaji,
        &doc.author_id,
    );
    add_search_text(
        &mut tantivy_doc,
        fields.tags_exact,
        fields.tags_lower,
        fields.tags_ngram,
        fields.tags_reading_kana,
        fields.tags_reading_romaji,
        &doc.tags,
    );
    add_search_text(
        &mut tantivy_doc,
        fields.series_title_exact,
        fields.series_title_lower,
        fields.series_title_ngram,
        fields.series_title_reading_kana,
        fields.series_title_reading_romaji,
        &doc.series_title,
    );
    add_search_text(
        &mut tantivy_doc,
        fields.excerpt_exact,
        fields.excerpt_lower,
        fields.excerpt_ngram,
        fields.excerpt_reading_kana,
        fields.excerpt_reading_romaji,
        &doc.excerpt,
    );
    // The body is stored for snippets but never matched whole, so it gets no
    // exact/lower term of its own - one term the length of a novel only bloats
    // the dictionary.
    tantivy_doc.add_text(fields.body_exact, &doc.body);
    let body = index_text(&doc.body);
    tantivy_doc.add_text(fields.body_ngram, body.surface);
    tantivy_doc.add_text(fields.body_reading_kana, body.reading_kana);
    tantivy_doc.add_text(fields.body_reading_romaji, body.reading_romaji);
    tantivy_doc.add_text(fields.published_at, &doc.published_at);
    tantivy_doc.add_text(fields.downloaded_at, &doc.downloaded_at);
    tantivy_doc.add_bool(fields.favorite, doc.favorite);
    tantivy_doc.add_bool(fields.watch_updates, doc.watch_updates);
    tantivy_doc.add_text(fields.asset_kinds, &doc.asset_kinds);
    tantivy_doc.add_i64(fields.text_length, doc.text_length);
    tantivy_doc
}

fn build_schema() -> Schema {
    let mut builder = Schema::builder();
    // FAST as well as stored: sorting a text search by a library column needs
    // the id of every match, and reading that from a fast field costs a column
    // lookup instead of loading each stored document (body included).
    builder.add_u64_field("download_id", INDEXED | STORED | FAST);
    builder.add_text_field("source_exact", STRING);
    add_text_family(&mut builder, "source_id");
    add_text_family(&mut builder, "source_url");
    add_text_family(&mut builder, "title");
    add_text_family(&mut builder, "author_name");
    add_text_family(&mut builder, "author_id");
    add_text_family(&mut builder, "tags");
    add_text_family(&mut builder, "series_title");
    add_text_family(&mut builder, "excerpt");
    // The body is read back for snippets and matched through its derived
    // forms, never as one term of its own.
    builder.add_text_field("body_exact", STORED);
    builder.add_text_field("body_ngram", TEXT);
    builder.add_text_field("body_reading_kana", TEXT);
    builder.add_text_field("body_reading_romaji", TEXT);
    builder.add_text_field("published_at", STRING);
    builder.add_text_field("downloaded_at", STRING);
    builder.add_bool_field("favorite", FAST);
    builder.add_bool_field("watch_updates", FAST);
    builder.add_text_field("asset_kinds", TEXT);
    builder.add_i64_field("text_length", FAST);
    builder.build()
}

/// Only the verbatim value is stored: the derived forms exist to be matched
/// against and are never read back, so storing them doubled the index for
/// nothing.
fn add_text_family(builder: &mut tantivy::schema::SchemaBuilder, name: &str) {
    builder.add_text_field(&format!("{}_exact", name), STRING | STORED);
    builder.add_text_field(&format!("{}_lower", name), STRING);
    builder.add_text_field(&format!("{}_ngram", name), TEXT);
    builder.add_text_field(&format!("{}_reading_kana", name), TEXT);
    builder.add_text_field(&format!("{}_reading_romaji", name), TEXT);
}

fn schema_has_required_fields(schema: &Schema) -> bool {
    [
        "download_id",
        "source_exact",
        "source_id_exact",
        "source_id_lower",
        "source_id_ngram",
        "source_id_reading_kana",
        "source_id_reading_romaji",
        "source_url_exact",
        "source_url_lower",
        "source_url_ngram",
        "source_url_reading_kana",
        "source_url_reading_romaji",
        "title_exact",
        "title_lower",
        "title_ngram",
        "title_reading_kana",
        "title_reading_romaji",
        "author_name_exact",
        "author_name_lower",
        "author_name_ngram",
        "author_name_reading_kana",
        "author_name_reading_romaji",
        "author_id_exact",
        "author_id_lower",
        "author_id_ngram",
        "author_id_reading_kana",
        "author_id_reading_romaji",
        "tags_exact",
        "tags_lower",
        "tags_ngram",
        "tags_reading_kana",
        "tags_reading_romaji",
        "series_title_exact",
        "series_title_lower",
        "series_title_ngram",
        "series_title_reading_kana",
        "series_title_reading_romaji",
        "excerpt_exact",
        "excerpt_lower",
        "excerpt_ngram",
        "excerpt_reading_kana",
        "excerpt_reading_romaji",
        "body_exact",
        "body_ngram",
        "body_reading_kana",
        "body_reading_romaji",
        "published_at",
        "downloaded_at",
        "favorite",
        "watch_updates",
        "asset_kinds",
        "text_length",
    ]
    .iter()
    .all(|field| schema.get_field(field).is_ok())
}

fn register_tokenizer(index: &Index) -> Result<(), String> {
    let tokenizer = TextAnalyzer::builder(
        NgramTokenizer::all_ngrams(2, 3)
            .map_err(|e| format!("Tantivy tokenizer creation failed: {}", e))?,
    )
    .filter(LowerCaser)
    .filter(RemoveLongFilter::limit(64))
    .build();
    index.tokenizers().register(TOKENIZER_NAME, tokenizer);
    Ok(())
}

fn fields(schema: &Schema) -> Result<TantivyFields, String> {
    let get = |name: &str| {
        schema
            .get_field(name)
            .map_err(|_| format!("Tantivy schema missing {}", name))
    };
    Ok(TantivyFields {
        download_id: get("download_id")?,
        source_exact: get("source_exact")?,
        source_id_exact: get("source_id_exact")?,
        source_id_lower: get("source_id_lower")?,
        source_id_ngram: get("source_id_ngram")?,
        source_id_reading_kana: get("source_id_reading_kana")?,
        source_id_reading_romaji: get("source_id_reading_romaji")?,
        source_url_exact: get("source_url_exact")?,
        source_url_lower: get("source_url_lower")?,
        source_url_ngram: get("source_url_ngram")?,
        source_url_reading_kana: get("source_url_reading_kana")?,
        source_url_reading_romaji: get("source_url_reading_romaji")?,
        title_exact: get("title_exact")?,
        title_lower: get("title_lower")?,
        title_ngram: get("title_ngram")?,
        title_reading_kana: get("title_reading_kana")?,
        title_reading_romaji: get("title_reading_romaji")?,
        author_name_exact: get("author_name_exact")?,
        author_name_lower: get("author_name_lower")?,
        author_name_ngram: get("author_name_ngram")?,
        author_name_reading_kana: get("author_name_reading_kana")?,
        author_name_reading_romaji: get("author_name_reading_romaji")?,
        author_id_exact: get("author_id_exact")?,
        author_id_lower: get("author_id_lower")?,
        author_id_ngram: get("author_id_ngram")?,
        author_id_reading_kana: get("author_id_reading_kana")?,
        author_id_reading_romaji: get("author_id_reading_romaji")?,
        tags_exact: get("tags_exact")?,
        tags_lower: get("tags_lower")?,
        tags_ngram: get("tags_ngram")?,
        tags_reading_kana: get("tags_reading_kana")?,
        tags_reading_romaji: get("tags_reading_romaji")?,
        series_title_exact: get("series_title_exact")?,
        series_title_lower: get("series_title_lower")?,
        series_title_ngram: get("series_title_ngram")?,
        series_title_reading_kana: get("series_title_reading_kana")?,
        series_title_reading_romaji: get("series_title_reading_romaji")?,
        excerpt_exact: get("excerpt_exact")?,
        excerpt_lower: get("excerpt_lower")?,
        excerpt_ngram: get("excerpt_ngram")?,
        excerpt_reading_kana: get("excerpt_reading_kana")?,
        excerpt_reading_romaji: get("excerpt_reading_romaji")?,
        body_exact: get("body_exact")?,
        body_ngram: get("body_ngram")?,
        body_reading_kana: get("body_reading_kana")?,
        body_reading_romaji: get("body_reading_romaji")?,
        published_at: get("published_at")?,
        downloaded_at: get("downloaded_at")?,
        favorite: get("favorite")?,
        watch_updates: get("watch_updates")?,
        asset_kinds: get("asset_kinds")?,
        text_length: get("text_length")?,
    })
}

/// Indexes a work's source URL by what identifies it, not by its boilerplate.
///
/// Every pixiv novel URL contains the path segment "novel", and the synonym
/// table expands 物語 to "novel" - so searching 物語 matched the entire pixiv
/// library through their URLs. Only the host and the id-bearing segments carry
/// identifying information; the fixed path words carry none and are the only
/// part that ever produced a false match. The verbatim URL is still indexed
/// exactly, so pasting a full URL still finds its work.
fn add_url_text(doc: &mut TantivyDocument, fields: &TantivyFields, url: &str) {
    let identity = url_identity_text(url);
    let indexed = index_text(&identity);
    doc.add_text(fields.source_url_exact, url);
    doc.add_text(fields.source_url_lower, normalize_for_search(url));
    doc.add_text(fields.source_url_ngram, indexed.surface);
    doc.add_text(fields.source_url_reading_kana, indexed.reading_kana);
    doc.add_text(fields.source_url_reading_romaji, indexed.reading_romaji);
}

/// The host plus every path or query value carrying a digit - which is where
/// work, post and creator ids live. Fixed words like "novel", "show.php" and
/// "posts" are the same on every URL and identify nothing.
fn url_identity_text(url: &str) -> String {
    let Ok(parsed) = Url::parse(url) else {
        return url.to_string();
    };
    let mut parts = Vec::new();
    if let Some(host) = parsed.host_str() {
        parts.push(host.to_string());
    }
    let carries_id = |value: &str| value.chars().any(|c| c.is_ascii_digit());
    if let Some(segments) = parsed.path_segments() {
        for segment in segments.filter(|segment| carries_id(segment)) {
            parts.push(segment.to_string());
        }
    }
    for (_, value) in parsed.query_pairs() {
        if carries_id(&value) {
            parts.push(value.to_string());
        }
    }
    parts.join(" ")
}

/// One morphological pass feeds all four derived fields. Deriving them with
/// separate helper calls re-analysed the same text once per field.
fn add_search_text(
    doc: &mut TantivyDocument,
    exact: Field,
    lower: Field,
    ngram: Field,
    reading_kana: Field,
    reading_romaji: Field,
    value: &str,
) {
    let indexed = index_text(value);
    doc.add_text(exact, value);
    doc.add_text(lower, indexed.normalized);
    doc.add_text(ngram, indexed.surface);
    doc.add_text(reading_kana, indexed.reading_kana);
    doc.add_text(reading_romaji, indexed.reading_romaji);
}

fn text_value(doc: &TantivyDocument, field: Field) -> String {
    doc.get_first(field)
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string()
}

fn escape_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|part| format!("\"{}\"", part.replace('"', "\\\"")))
        .collect::<Vec<String>>()
        .join(" ")
}
