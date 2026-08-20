use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[cfg(not(test))]
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use hnsw_rs::prelude::{AnnT, DistCosine, Hnsw};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(not(test))]
use std::sync::Mutex;

#[cfg(test)]
use super::search_normalization::search_index_text;
use super::search_normalization::{normalize_for_search, query_variants, synonym_variants};

const INDEX_DIR_NAME: &str = "search";
const INDEX_VERSION_DIR: &str = "semantic-v1";
const INDEX_FILE_NAME: &str = "index.sqlite";
const MODEL_ID: &str = "intfloat/multilingual-e5-small";
const VECTOR_DIMENSION: usize = 384;
const CHUNK_TARGET_CHARS: usize = 620;
const CHUNK_OVERLAP_CHARS: usize = 80;
const ANN_MANIFEST_FILE: &str = "ann-manifest.json";
const ANN_FORMAT_VERSION: u32 = 1;

#[cfg(not(test))]
static MODEL: OnceLock<Result<Mutex<EmbeddingRuntime>, String>> = OnceLock::new();
static ANN_BUILD_LOCK: OnceLock<parking_lot::Mutex<()>> = OnceLock::new();

pub fn model_id() -> &'static str {
    MODEL_ID
}

#[derive(Debug, Clone)]
pub struct SemanticIndexDocument {
    pub download_id: i64,
    pub title: String,
    pub author_name: String,
    pub tags: String,
    pub series_title: String,
    pub excerpt: String,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct SemanticSearchHit {
    pub download_id: i64,
    pub chunk_id: String,
    pub field: String,
    pub text: String,
    pub score: f64,
}

#[derive(Debug, Clone, Default)]
pub struct SemanticIndexStatus {
    pub indexed_chunks: i64,
    pub model_ready: bool,
    pub provider: String,
    pub gpu_enabled: bool,
}

#[derive(Debug, Clone)]
struct ChunkInput {
    chunk_id: String,
    field: String,
    text: String,
}

#[cfg(not(test))]
struct EmbeddingRuntime {
    model: TextEmbedding,
    provider: String,
    gpu_enabled: bool,
}

#[derive(Debug, Clone)]
struct ChunkRecord {
    download_id: i64,
    chunk: ChunkInput,
}

#[derive(Debug, Clone)]
struct ChunkMeta {
    download_id: i64,
    chunk_id: String,
    field: String,
    text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AnnManifest {
    format_version: u32,
    model_id: String,
    dimension: usize,
    shard_span: i64,
    shards: Vec<AnnShard>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AnnShard {
    bucket: i64,
    fingerprint: String,
    basename: String,
    count: usize,
}

pub fn upsert_documents(
    storage_dir: &Path,
    docs: &[SemanticIndexDocument],
) -> Result<usize, String> {
    if docs.is_empty() {
        return Ok(0);
    }

    let mut records = Vec::new();
    for doc in docs {
        records.extend(build_chunks(doc).into_iter().map(|chunk| ChunkRecord {
            download_id: doc.download_id,
            chunk,
        }));
    }

    let passages = records
        .iter()
        .map(|record| format!("passage: {}", record.chunk.text))
        .collect::<Vec<_>>();
    let vectors = if passages.is_empty() {
        Vec::new()
    } else {
        embed_texts(passages)?
    };

    let conn = open_index(storage_dir)?;
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("Semantic transaction failed: {}", e))?;
    for doc in docs {
        tx.execute(
            "DELETE FROM semantic_chunks WHERE download_id = ?1",
            params![doc.download_id],
        )
        .map_err(|e| format!("Semantic clear failed: {}", e))?;
    }
    for (record, vector) in records.iter().zip(vectors.iter()) {
        if vector.len() != VECTOR_DIMENSION {
            return Err(format!(
                "Semantic model dimension mismatch: expected {}, got {}",
                VECTOR_DIMENSION,
                vector.len()
            ));
        }
        tx.execute(
            "INSERT OR REPLACE INTO semantic_chunks (
                download_id, chunk_id, field, text, text_hash, model_id, dimension, vector, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                record.download_id,
                record.chunk.chunk_id,
                record.chunk.field,
                record.chunk.text,
                hash_text(&record.chunk.text),
                MODEL_ID,
                VECTOR_DIMENSION as i64,
                vector_to_blob(vector),
                chrono::Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|e| format!("Semantic chunk insert failed: {}", e))?;
    }
    tx.commit()
        .map_err(|e| format!("Semantic transaction commit failed: {}", e))?;
    invalidate_ann_cache(storage_dir);
    Ok(records.len())
}

pub fn clear_document(storage_dir: &Path, download_id: i64) -> Result<(), String> {
    clear_documents(storage_dir, &[download_id])
}

/// Removes several works in one SQLite transaction and invalidates the ANN
/// cache once. Bulk library deletion used to rebuild this cache boundary once
/// per work, which made a large delete needlessly expensive.
pub fn clear_documents(storage_dir: &Path, download_ids: &[i64]) -> Result<(), String> {
    if download_ids.is_empty() {
        return Ok(());
    }

    let mut conn = open_index(storage_dir)?;
    let tx = conn
        .transaction()
        .map_err(|e| format!("Semantic clear transaction failed: {e}"))?;
    {
        let mut statement = tx
            .prepare("DELETE FROM semantic_chunks WHERE download_id = ?1")
            .map_err(|e| format!("Semantic clear prepare failed: {e}"))?;
        for download_id in download_ids {
            statement
                .execute(params![download_id])
                .map_err(|e| format!("Semantic clear failed for {download_id}: {e}"))?;
        }
    }
    tx.commit()
        .map_err(|e| format!("Semantic clear commit failed: {e}"))?;
    invalidate_ann_cache(storage_dir);
    Ok(())
}

pub fn search(
    storage_dir: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<SemanticSearchHit>, String> {
    if query.trim().is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let query_text = semantic_query_text(query);
    let query_vector = embed_texts(vec![format!("query: {}", query_text)])?
        .into_iter()
        .next()
        .ok_or_else(|| "Semantic query embedding returned no vector".to_string())?;

    let manifest = ensure_ann_shards(storage_dir)?;
    let conn = open_index(storage_dir)?;
    let mut hits = Vec::with_capacity(limit.saturating_mul(manifest.shards.len().min(8)));
    for shard in &manifest.shards {
        let mut loader = hnsw_rs::prelude::HnswIo::new(&ann_dir(storage_dir), &shard.basename);
        let ann: Hnsw<'_, f32, DistCosine> = loader
            .load_hnsw::<f32, DistCosine>()
            .map_err(|error| format!("Semantic ANN shard load failed: {error}"))?;
        let neighbours = ann.search(
            &query_vector,
            limit.saturating_mul(4).max(32),
            limit.saturating_mul(8).max(64),
        );
        let ids = neighbours
            .iter()
            .map(|neighbour| neighbour.d_id as i64)
            .collect::<Vec<_>>();
        let metadata = chunk_metadata_by_rowid(&conn, &ids)?;
        for neighbour in neighbours {
            let Some(meta) = metadata.get(&(neighbour.d_id as i64)) else {
                continue;
            };
            let score = (1.0 - neighbour.distance as f64).clamp(0.0, 1.0);
            if score >= 0.18 {
                hits.push(SemanticSearchHit {
                    download_id: meta.download_id,
                    chunk_id: meta.chunk_id.clone(),
                    field: meta.field.clone(),
                    text: meta.text.clone(),
                    score,
                });
            }
        }
    }

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.download_id.cmp(&b.download_id))
    });
    hits.truncate(limit);
    Ok(hits)
}

pub fn status(storage_dir: &Path) -> SemanticIndexStatus {
    let Ok(conn) = open_index(storage_dir) else {
        return SemanticIndexStatus {
            indexed_chunks: 0,
            model_ready: semantic_model_ready(),
            provider: embedding_provider(),
            gpu_enabled: embedding_gpu_enabled(),
        };
    };
    let indexed_chunks = conn
        .query_row(
            "SELECT COUNT(*) FROM semantic_chunks WHERE model_id = ?1 AND dimension = ?2",
            params![MODEL_ID, VECTOR_DIMENSION as i64],
            |row| row.get(0),
        )
        .unwrap_or(0);
    SemanticIndexStatus {
        indexed_chunks,
        model_ready: semantic_model_ready(),
        provider: embedding_provider(),
        gpu_enabled: embedding_gpu_enabled(),
    }
}

fn ensure_ann_shards(storage_dir: &Path) -> Result<AnnManifest, String> {
    let _guard = ANN_BUILD_LOCK
        .get_or_init(|| parking_lot::Mutex::new(()))
        .lock();
    let conn = open_index(storage_dir)?;
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM semantic_chunks WHERE model_id = ?1 AND dimension = ?2",
            params![MODEL_ID, VECTOR_DIMENSION as i64],
            |row| row.get(0),
        )
        .map_err(|error| format!("Semantic ANN count failed: {error}"))?;
    if total == 0 {
        return Ok(AnnManifest {
            format_version: ANN_FORMAT_VERSION,
            model_id: MODEL_ID.to_string(),
            dimension: VECTOR_DIMENSION,
            shard_span: 1,
            shards: Vec::new(),
        });
    }
    let max_vectors = (super::resource_budget::semantic_ann_bytes()
        / ((VECTOR_DIMENSION * std::mem::size_of::<f32>()) as u64 * 3))
        .clamp(2_000, 50_000) as i64;
    let shard_span = max_vectors;
    let manifest_path = ann_dir(storage_dir).join(ANN_MANIFEST_FILE);
    let previous = std::fs::read(&manifest_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<AnnManifest>(&bytes).ok())
        .filter(|manifest| {
            manifest.format_version == ANN_FORMAT_VERSION
                && manifest.model_id == MODEL_ID
                && manifest.dimension == VECTOR_DIMENSION
                && manifest.shard_span == shard_span
        });

    std::fs::create_dir_all(ann_dir(storage_dir))
        .map_err(|error| format!("Semantic ANN directory create failed: {error}"))?;
    let mut stmt = conn
        .prepare(
            "SELECT rowid, vector
             FROM semantic_chunks
             WHERE model_id = ?1 AND dimension = ?2
               AND rowid >= ?3 AND rowid < ?4
             ORDER BY rowid",
        )
        .map_err(|e| format!("Semantic ANN load prepare failed: {}", e))?;
    let max_rowid: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(rowid), 0) FROM semantic_chunks WHERE model_id = ?1 AND dimension = ?2",
            params![MODEL_ID, VECTOR_DIMENSION as i64],
            |row| row.get(0),
        )
        .map_err(|error| format!("Semantic ANN max row failed: {error}"))?;
    let previous_by_bucket = previous
        .map(|manifest| {
            manifest
                .shards
                .into_iter()
                .map(|shard| (shard.bucket, shard))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let mut shards = Vec::new();
    for bucket in 0..=(max_rowid / shard_span) {
        let start = bucket * shard_span;
        let end = start.saturating_add(shard_span);
        let fingerprint: String = conn
            .query_row(
                "SELECT printf('%d:%s:%d', COUNT(*), COALESCE(MAX(updated_at), ''), COALESCE(SUM(LENGTH(vector)), 0))
                 FROM semantic_chunks
                 WHERE model_id = ?1 AND dimension = ?2 AND rowid >= ?3 AND rowid < ?4",
                params![MODEL_ID, VECTOR_DIMENSION as i64, start, end],
                |row| row.get(0),
            )
            .map_err(|error| format!("Semantic ANN fingerprint failed: {error}"))?;
        if let Some(previous) = previous_by_bucket.get(&bucket).filter(|shard| {
            shard.fingerprint == fingerprint && ann_files_exist(storage_dir, &shard.basename)
        }) {
            shards.push(previous.clone());
            continue;
        }
        let rows = stmt
            .query_map(
                params![MODEL_ID, VECTOR_DIMENSION as i64, start, end],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .map_err(|error| format!("Semantic ANN shard query failed: {error}"))?;
        let mut ids = Vec::new();
        let mut vectors = Vec::new();
        for row in rows {
            let (rowid, blob) = row.map_err(|error| format!("Semantic ANN row failed: {error}"))?;
            let vector = blob_to_vector(&blob);
            if vector.len() == VECTOR_DIMENSION {
                ids.push(rowid as usize);
                vectors.push(vector);
            }
        }
        if vectors.is_empty() {
            continue;
        }
        let mut hnsw = Hnsw::<f32, DistCosine>::new(16, vectors.len(), 16, 200, DistCosine {});
        for (vector, rowid) in vectors.iter().zip(ids.iter()) {
            hnsw.insert((vector, *rowid));
        }
        hnsw.set_searching_mode(true);
        let requested = format!("ann-{bucket}-{}", rand::random::<u64>());
        let basename = hnsw
            .file_dump(&ann_dir(storage_dir), &requested)
            .map_err(|error| format!("Semantic ANN persist failed: {error}"))?;
        shards.push(AnnShard {
            bucket,
            fingerprint,
            basename,
            count: vectors.len(),
        });
    }
    let manifest = AnnManifest {
        format_version: ANN_FORMAT_VERSION,
        model_id: MODEL_ID.to_string(),
        dimension: VECTOR_DIMENSION,
        shard_span,
        shards,
    };
    write_ann_manifest(&manifest_path, &manifest)?;
    cleanup_old_ann_files(storage_dir, &manifest);
    Ok(manifest)
}

fn invalidate_ann_cache(_storage_dir: &Path) {
    // Shards are content-fingerprinted against SQLite and rebuilt lazily. No
    // process-global graph or payload cache is retained.
}

fn ann_dir(storage_dir: &Path) -> PathBuf {
    index_path(storage_dir)
        .parent()
        .unwrap_or(storage_dir)
        .join("ann")
}

fn ann_files_exist(storage_dir: &Path, basename: &str) -> bool {
    let directory = ann_dir(storage_dir);
    directory.join(format!("{basename}.hnsw.graph")).is_file()
        && directory.join(format!("{basename}.hnsw.data")).is_file()
}

fn write_ann_manifest(path: &Path, manifest: &AnnManifest) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Semantic ANN manifest has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Semantic ANN directory create failed: {error}"))?;
    let temporary = parent.join(format!(".ann-manifest-{:016x}.tmp", rand::random::<u64>()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| format!("Semantic ANN manifest create failed: {error}"))?;
    let write_result = (|| {
        serde_json::to_writer(&mut file, manifest)
            .map_err(|error| format!("Semantic ANN manifest serialize failed: {error}"))?;
        file.flush()
            .map_err(|error| format!("Semantic ANN manifest flush failed: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("Semantic ANN manifest sync failed: {error}"))?;
        replace_manifest(&temporary, path)
    })();
    if write_result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    write_result
}

#[cfg(windows)]
fn replace_manifest(source: &Path, destination: &Path) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let mut source_wide = source.as_os_str().encode_wide().collect::<Vec<_>>();
    source_wide.push(0);
    let mut destination_wide = destination.as_os_str().encode_wide().collect::<Vec<_>>();
    destination_wide.push(0);
    // SAFETY: both paths are immutable, NUL-terminated UTF-16 buffers for the
    // duration of this synchronous Win32 call.
    let result = unsafe {
        MoveFileExW(
            PCWSTR(source_wide.as_ptr()),
            PCWSTR(destination_wide.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if let Err(error) = result {
        return Err(format!("Semantic ANN manifest publish failed: {error}"));
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_manifest(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::rename(source, destination)
        .map_err(|error| format!("Semantic ANN manifest publish failed: {error}"))
}

fn cleanup_old_ann_files(storage_dir: &Path, manifest: &AnnManifest) {
    let keep = manifest
        .shards
        .iter()
        .flat_map(|shard| {
            [
                format!("{}.hnsw.graph", shard.basename),
                format!("{}.hnsw.data", shard.basename),
            ]
        })
        .collect::<std::collections::HashSet<_>>();
    let Ok(entries) = std::fs::read_dir(ann_dir(storage_dir)) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("ann-")
            && name != ANN_MANIFEST_FILE
            && !name.ends_with(".tmp")
            && !keep.contains(&name)
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn chunk_metadata_by_rowid(
    conn: &Connection,
    rowids: &[i64],
) -> Result<HashMap<i64, ChunkMeta>, String> {
    if rowids.is_empty() {
        return Ok(HashMap::new());
    }
    let json = serde_json::to_string(rowids)
        .map_err(|error| format!("Semantic ANN row ID encode failed: {error}"))?;
    let mut statement = conn
        .prepare(
            "SELECT rowid, download_id, chunk_id, field, text
             FROM semantic_chunks
             WHERE rowid IN (SELECT CAST(value AS INTEGER) FROM json_each(?1))",
        )
        .map_err(|error| format!("Semantic ANN metadata prepare failed: {error}"))?;
    let rows = statement
        .query_map(params![json], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                ChunkMeta {
                    download_id: row.get(1)?,
                    chunk_id: row.get(2)?,
                    field: row.get(3)?,
                    text: row.get(4)?,
                },
            ))
        })
        .map_err(|error| format!("Semantic ANN metadata query failed: {error}"))?;
    rows.collect::<Result<HashMap<_, _>, _>>()
        .map_err(|error| format!("Semantic ANN metadata row failed: {error}"))
}

fn build_chunks(doc: &SemanticIndexDocument) -> Vec<ChunkInput> {
    let mut chunks = Vec::new();
    let meta = [
        doc.title.as_str(),
        doc.author_name.as_str(),
        doc.tags.as_str(),
        doc.series_title.as_str(),
        doc.excerpt.as_str(),
    ]
    .iter()
    .map(|value| value.trim())
    .filter(|value| !value.is_empty())
    .collect::<Vec<_>>()
    .join("\n");
    if !meta.trim().is_empty() {
        chunks.push(ChunkInput {
            chunk_id: "meta-0".to_string(),
            field: "metadata".to_string(),
            text: meta,
        });
    }
    for (idx, text) in split_text_chunks(&doc.body).into_iter().enumerate() {
        chunks.push(ChunkInput {
            chunk_id: format!("body-{}", idx),
            field: "body".to_string(),
            text,
        });
    }
    chunks
}

fn split_text_chunks(text: &str) -> Vec<String> {
    let paragraphs = text
        .split("\n\n")
        .flat_map(|part| part.split('\n'))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let mut chunks = Vec::new();
    let mut current = String::new();

    for paragraph in paragraphs {
        let next_len = current.chars().count() + paragraph.chars().count() + 1;
        if !current.is_empty() && next_len > CHUNK_TARGET_CHARS {
            chunks.push(current.trim().to_string());
            let overlap = tail_chars(
                chunks.last().map(String::as_str).unwrap_or(""),
                CHUNK_OVERLAP_CHARS,
            );
            current = overlap;
            if !current.is_empty() {
                current.push('\n');
            }
        }
        current.push_str(paragraph);
        current.push('\n');
    }
    if !current.trim().is_empty() {
        chunks.push(current.trim().to_string());
    }
    chunks
}

fn semantic_query_text(query: &str) -> String {
    let mut values = query_variants(query);
    for synonym in synonym_variants(query) {
        values.push(synonym);
    }
    if values.is_empty() {
        normalize_for_search(query)
    } else {
        values.join(" ")
    }
}

fn tail_chars(text: &str, count: usize) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let start = chars.len().saturating_sub(count);
    chars[start..].iter().collect()
}

fn open_index(storage_dir: &Path) -> Result<Connection, String> {
    let path = index_path(storage_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Semantic index dir creation failed: {}", e))?;
    }
    let conn = Connection::open(&path).map_err(|e| format!("Semantic index open failed: {}", e))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE IF NOT EXISTS semantic_chunks (
            download_id INTEGER NOT NULL,
            chunk_id TEXT NOT NULL,
            field TEXT NOT NULL,
            text TEXT NOT NULL,
            text_hash TEXT NOT NULL,
            model_id TEXT NOT NULL,
            dimension INTEGER NOT NULL,
            vector BLOB NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY(download_id, chunk_id)
         );
         CREATE INDEX IF NOT EXISTS idx_semantic_chunks_model
            ON semantic_chunks(model_id, dimension);
         CREATE INDEX IF NOT EXISTS idx_semantic_chunks_download
            ON semantic_chunks(download_id);",
    )
    .map_err(|e| format!("Semantic schema init failed: {}", e))?;
    Ok(conn)
}

fn index_path(storage_dir: &Path) -> PathBuf {
    storage_dir
        .parent()
        .unwrap_or(storage_dir)
        .join(INDEX_DIR_NAME)
        .join(INDEX_VERSION_DIR)
        .join(INDEX_FILE_NAME)
}

#[cfg(not(test))]
fn embed_texts(texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
    let model = MODEL.get_or_init(|| {
        init_embedding_runtime()
            .map(Mutex::new)
            .map_err(|e| format!("Semantic model init failed: {}", e))
    });
    let mutex = model.as_ref().map_err(Clone::clone)?;
    let mut runtime = mutex
        .lock()
        .map_err(|e| format!("Semantic model lock failed: {}", e))?;
    runtime
        .model
        .embed(texts, None)
        .map_err(|e| format!("Semantic embedding failed: {}", e))
}

#[cfg(not(test))]
fn init_embedding_runtime() -> Result<EmbeddingRuntime, String> {
    #[cfg(windows)]
    {
        let gpu_options = InitOptions::new(EmbeddingModel::MultilingualE5Small)
            .with_show_download_progress(true)
            .with_intra_threads(2)
            .with_execution_providers(vec![ort::ep::DirectML::default().build()]);
        match TextEmbedding::try_new(gpu_options) {
            Ok(model) => {
                return Ok(EmbeddingRuntime {
                    model,
                    provider: "fastembed/directml".to_string(),
                    gpu_enabled: true,
                });
            }
            Err(error) => {
                log::warn!(
                    "DirectML semantic model init failed, falling back to CPU: {}",
                    error
                );
            }
        }
    }

    let threads = std::thread::available_parallelism()
        .map(|value| value.get().clamp(2, 8))
        .unwrap_or(4);
    TextEmbedding::try_new(
        InitOptions::new(EmbeddingModel::MultilingualE5Small)
            .with_show_download_progress(true)
            .with_intra_threads(threads),
    )
    .map(|model| EmbeddingRuntime {
        model,
        provider: "fastembed/cpu".to_string(),
        gpu_enabled: false,
    })
    .map_err(|e| e.to_string())
}

#[cfg(test)]
fn embed_texts(texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
    Ok(texts
        .iter()
        .map(|text| pseudo_embedding(text, VECTOR_DIMENSION))
        .collect())
}

#[cfg(not(test))]
fn semantic_model_ready() -> bool {
    MODEL.get().map(|state| state.is_ok()).unwrap_or(false)
}

#[cfg(test)]
fn semantic_model_ready() -> bool {
    true
}

#[cfg(not(test))]
fn embedding_provider() -> String {
    MODEL
        .get()
        .and_then(|state| state.as_ref().ok())
        .and_then(|runtime| runtime.lock().ok().map(|runtime| runtime.provider.clone()))
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "fastembed/directml-pending".to_string()
            } else {
                "fastembed/cpu-pending".to_string()
            }
        })
}

#[cfg(test)]
fn embedding_provider() -> String {
    "pseudo/test".to_string()
}

#[cfg(not(test))]
fn embedding_gpu_enabled() -> bool {
    MODEL
        .get()
        .and_then(|state| state.as_ref().ok())
        .and_then(|runtime| runtime.lock().ok().map(|runtime| runtime.gpu_enabled))
        .unwrap_or(false)
}

#[cfg(test)]
fn embedding_gpu_enabled() -> bool {
    false
}

#[cfg(test)]
fn pseudo_embedding(text: &str, dimension: usize) -> Vec<f32> {
    let mut vector = vec![0.0; dimension];
    let mut terms = query_variants(text);
    for part in search_index_text(text).split_whitespace() {
        terms.extend(query_variants(part));
    }
    for term in terms {
        let mut hasher = Sha256::new();
        hasher.update(term.as_bytes());
        let digest = hasher.finalize();
        let idx =
            u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]) as usize % dimension;
        vector[idx] += 1.0;
    }
    normalize_vector(vector)
}

fn vector_to_blob(vector: &[f32]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        blob.extend_from_slice(&value.to_le_bytes());
    }
    blob
}

fn blob_to_vector(blob: &[u8]) -> Vec<f32> {
    // 4バイトずつ。長さが型で決まるので、添字で取り出す必要がなくなる。
    blob.as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect()
}

#[cfg(test)]
fn normalize_vector(mut vector: Vec<f32>) -> Vec<f32> {
    let norm = vector
        .iter()
        .map(|value| (*value as f64) * (*value as f64))
        .sum::<f64>()
        .sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value = (*value as f64 / norm) as f32;
        }
    }
    vector
}

fn hash_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn semantic_sidecar_returns_chunk_hits() {
        let root =
            std::env::temp_dir().join(format!("piep_semantic_test_{}", rand::random::<u32>()));
        let storage = root.join("downloads");
        fs::create_dir_all(&storage).unwrap();
        upsert_documents(
            &storage,
            &[SemanticIndexDocument {
                download_id: 1,
                title: "静かな物語".to_string(),
                author_name: "作者".to_string(),
                tags: "小説".to_string(),
                series_title: String::new(),
                excerpt: String::new(),
                body: "これは長い小説本文です。".to_string(),
            }],
        )
        .unwrap();
        let hits = search(&storage, "shousetsu", 10).unwrap();
        assert!(hits.iter().any(|hit| hit.download_id == 1));
        let manifest: AnnManifest =
            serde_json::from_slice(&fs::read(ann_dir(&storage).join(ANN_MANIFEST_FILE)).unwrap())
                .unwrap();
        assert_eq!(manifest.shards.len(), 1);
        assert!(ann_files_exist(&storage, &manifest.shards[0].basename));
        let _ = fs::remove_dir_all(root);
    }
}
