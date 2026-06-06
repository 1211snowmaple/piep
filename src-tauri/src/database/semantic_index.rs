use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

#[cfg(not(test))]
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use hnsw_rs::prelude::{DistCosine, Hnsw};
use parking_lot::Mutex as ParkingMutex;
use rusqlite::{params, Connection};
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

#[cfg(not(test))]
static MODEL: OnceLock<Result<Mutex<EmbeddingRuntime>, String>> = OnceLock::new();
static ANN_CACHE: OnceLock<ParkingMutex<HashMap<PathBuf, Arc<SemanticAnnIndex>>>> = OnceLock::new();

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

struct SemanticAnnIndex {
    fingerprint: String,
    hnsw: Hnsw<'static, f32, DistCosine>,
    chunks: Vec<ChunkMeta>,
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
    let conn = open_index(storage_dir)?;
    conn.execute(
        "DELETE FROM semantic_chunks WHERE download_id = ?1",
        params![download_id],
    )
    .map_err(|e| format!("Semantic clear failed: {}", e))?;
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

    let ann = ann_index(storage_dir)?;
    let neighbours = ann.hnsw.search(
        &query_vector,
        limit.saturating_mul(4).max(32),
        limit.saturating_mul(8).max(64),
    );
    let mut hits = Vec::with_capacity(neighbours.len());
    for neighbour in neighbours {
        let idx = neighbour.d_id;
        let Some(meta) = ann.chunks.get(idx) else {
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

fn ann_index(storage_dir: &Path) -> Result<Arc<SemanticAnnIndex>, String> {
    let key = index_path(storage_dir);
    let fingerprint = index_fingerprint(storage_dir)?;
    let cache = ANN_CACHE.get_or_init(|| ParkingMutex::new(HashMap::new()));
    if let Some(index) = cache.lock().get(&key).cloned() {
        if index.fingerprint == fingerprint {
            return Ok(index);
        }
    }

    let conn = open_index(storage_dir)?;
    let mut stmt = conn
        .prepare(
            "SELECT download_id, chunk_id, field, text, vector
             FROM semantic_chunks
             WHERE model_id = ?1 AND dimension = ?2
             ORDER BY download_id, chunk_id",
        )
        .map_err(|e| format!("Semantic ANN load prepare failed: {}", e))?;
    let rows = stmt
        .query_map(params![MODEL_ID, VECTOR_DIMENSION as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })
        .map_err(|e| format!("Semantic ANN load failed: {}", e))?;

    let mut chunks = Vec::new();
    let mut vectors = Vec::new();
    for row in rows {
        let (download_id, chunk_id, field, text, blob) =
            row.map_err(|e| format!("Semantic ANN row read failed: {}", e))?;
        let vector = blob_to_vector(&blob);
        if vector.len() != VECTOR_DIMENSION {
            continue;
        }
        chunks.push(ChunkMeta {
            download_id,
            chunk_id,
            field,
            text,
        });
        vectors.push(vector);
    }

    let max_elements = vectors.len().max(1);
    let mut hnsw = Hnsw::<f32, DistCosine>::new(16, max_elements, 16, 200, DistCosine {});
    if vectors.len() >= 256 {
        let refs = vectors
            .iter()
            .enumerate()
            .map(|(idx, vector)| (vector, idx))
            .collect::<Vec<_>>();
        hnsw.parallel_insert(&refs);
    } else {
        for (idx, vector) in vectors.iter().enumerate() {
            hnsw.insert((vector, idx));
        }
    }
    hnsw.set_searching_mode(true);

    let index = Arc::new(SemanticAnnIndex {
        fingerprint,
        hnsw,
        chunks,
    });
    cache.lock().insert(key, index.clone());
    Ok(index)
}

fn index_fingerprint(storage_dir: &Path) -> Result<String, String> {
    let conn = open_index(storage_dir)?;
    conn.query_row(
        "SELECT
            COUNT(*),
            COALESCE(MAX(updated_at), ''),
            COALESCE(SUM(LENGTH(text_hash)), 0),
            COALESCE(SUM(LENGTH(vector)), 0)
         FROM semantic_chunks
         WHERE model_id = ?1 AND dimension = ?2",
        params![MODEL_ID, VECTOR_DIMENSION as i64],
        |row| {
            Ok(format!(
                "{}:{}:{}:{}",
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?
            ))
        },
    )
    .map_err(|e| format!("Semantic fingerprint failed: {}", e))
}

fn invalidate_ann_cache(storage_dir: &Path) {
    let Some(cache) = ANN_CACHE.get() else {
        return;
    };
    cache.lock().remove(&index_path(storage_dir));
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
    blob.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
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
        let _ = fs::remove_dir_all(root);
    }
}
