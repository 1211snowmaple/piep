use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
#[cfg(not(test))]
use std::time::{Duration, Instant};

#[cfg(not(test))]
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use sha2::{Digest, Sha256};
#[cfg(not(test))]
use std::sync::Mutex;

#[cfg(test)]
use super::normalization::{query_variants, search_index_text};

const INDEX_DIR_NAME: &str = "search";
const INDEX_VERSION_DIR: &str = "semantic-v2";
const INDEX_FILE_NAME: &str = "index.sqlite";
const INDEX_SCHEMA_VERSION: i64 = 1;
const MODEL_ID: &str = "intfloat/multilingual-e5-small";
const VECTOR_DIMENSION: usize = 384;
// 本文の分割規則を変えるときに上げる。既存行に付ける初期値とは独立させる。
const CHUNKING_VERSION: i64 = 1;
const CHUNK_TARGET_CHARS: usize = 620;
const CHUNK_OVERLAP_CHARS: usize = 80;

#[cfg(not(test))]
// Only successful initialization is cached. A transient download/runtime
// failure must not poison semantic search until the whole app is restarted.
static MODEL: OnceLock<Mutex<EmbeddingRuntimeSlot>> = OnceLock::new();
#[cfg(not(test))]
static MODEL_REAPER_STARTED: OnceLock<()> = OnceLock::new();
static MODEL_CACHE_DIR: OnceLock<PathBuf> = OnceLock::new();
#[cfg(not(test))]
const MODEL_IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
#[cfg(not(test))]
// Fastembed's default is 256 passages. A small DirectML batch leaves room for
// the local LLM and desktop while preserving the input order and embeddings.
const GPU_EMBEDDING_BATCH_SIZE: usize = 16;
/// fastembed が指定なしのときに使う置き場。**現在の作業ディレクトリ相対**。
#[cfg(not(test))]
const FASTEMBED_DEFAULT_CACHE_DIR: &str = ".fastembed_cache";
const MODEL_DIR_NAME: &str = "models";

/// 埋め込みモデルの置き場を、アプリのデータの下（`models`）に固定する。
///
/// fastembed の既定は `./.fastembed_cache`、つまり**起動した場所**なので、指定しないと
/// 起動した場所ごとに 465MB のモデルを落とし直す。棚の外に置くのは、モデルが棚のデータでも
/// 索引でもないから（バックアップに入れない）。起動時に一度だけ呼ぶ。呼ばれなかったときは
/// fastembed の既定のままにする。
pub fn set_model_cache_dir(app_data: &Path) {
    let _ = MODEL_CACHE_DIR.set(model_cache_dir_in(app_data));
}

/// hf-hub がモデルを置くときのディレクトリ名（`a/b` → `models--a--b`）。
#[cfg(test)]
fn hf_hub_dir_name(model_id: &str) -> String {
    format!("models--{}", model_id.replace('/', "--"))
}

fn model_cache_dir_in(app_data: &Path) -> PathBuf {
    app_data.join(MODEL_DIR_NAME)
}

#[cfg(not(test))]
fn model_cache_dir() -> PathBuf {
    MODEL_CACHE_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| PathBuf::from(FASTEMBED_DEFAULT_CACHE_DIR))
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
    pub field: String,
    pub text: String,
    pub score: f64,
}

#[derive(Debug, Clone, Default)]
pub struct SemanticIndexStatus {
    /// Number of searchable works.  One row is always one library work.
    pub indexed_works: i64,
    pub model_ready: bool,
    pub provider: String,
    pub gpu_enabled: bool,
}

#[derive(Debug, Clone)]
struct ChunkInput {
    field: String,
    text: String,
}

#[cfg(not(test))]
struct EmbeddingRuntime {
    model: TextEmbedding,
    provider: String,
    gpu_enabled: bool,
}

#[cfg(not(test))]
#[derive(Default)]
struct EmbeddingRuntimeSlot {
    runtime: Option<EmbeddingRuntime>,
    last_used: Option<Instant>,
}

/// 前の索引にある、同じ文書から作ったベクトル。
pub struct ReusableVectors<'a> {
    /// 前の索引の置き場（`upsert_documents` に渡したのと同じ `storage_dir`）。
    pub storage_dir: &'a Path,
    /// 新しい文書の ID → 前の索引の文書の ID。
    pub previous_ids: &'a HashMap<i64, i64>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct UpsertReport {
    pub indexed: Vec<i64>,
    /// 前の索引から写した説明（題・作者・タグ・シリーズ・説明）のベクトルの数。
    pub reused_metadata: usize,
    /// 前の索引から写した本文のベクトルの数。
    pub reused_content: usize,
    /// 埋め込んだ断片の数。
    pub embedded_passages: usize,
}

/// 写せる前のベクトル。同じモデル・同じ文字列から作ったものだけ。
#[derive(Clone)]
struct Previous {
    metadata: Option<Vec<f32>>,
    /// 本文の重心と断片の数。本文がない作品は `Some((None, 0))`。
    content: Option<(Option<Vec<f32>>, i64)>,
}

/// 文書のベクトルを作って入れる。`reuse` があれば、同じ文字列から作った前のベクトルは
/// 埋め込み直さずに写す（ベクトルは文字列とモデルだけで決まる）。説明と本文は別々に判定する
/// ので、タグの並びだけが違う作品は、説明の一断片だけを埋め込めば済む。
pub fn upsert_documents_reusing(
    storage_dir: &Path,
    docs: &[SemanticIndexDocument],
    reuse: Option<ReusableVectors<'_>>,
) -> Result<UpsertReport, String> {
    let mut report = UpsertReport::default();
    if docs.is_empty() {
        return Ok(report);
    }

    // Keep the chunks grouped by work. Rebuilding every work's chunks again
    // after inference doubled the text scanning and allocations for long
    // documents, precisely while a large-library rebuild is already busy.
    let chunks_by_doc = docs.iter().map(build_chunks).collect::<Vec<_>>();
    let previous = match &reuse {
        Some(reuse) => previous_vectors(reuse, docs, &chunks_by_doc)?,
        None => vec![None; docs.len()],
    };

    // 埋め込むのは、写せなかった断片だけ。写せた断片の位置には何も置かない。
    let mut passages = Vec::new();
    for (chunks, previous) in chunks_by_doc.iter().zip(&previous) {
        let metadata_known = previous.as_ref().is_some_and(|p| p.metadata.is_some());
        let content_known = previous.as_ref().is_some_and(|p| p.content.is_some());
        for chunk in chunks {
            let known = match chunk.field.as_str() {
                "metadata" => metadata_known,
                _ => content_known,
            };
            if !known {
                passages.push(format!("passage: {}", chunk.text));
            }
        }
    }
    let passage_count = passages.len();
    report.embedded_passages = passage_count;
    let embedded = if passages.is_empty() {
        Vec::new()
    } else {
        embed_texts(passages)?
    };
    if passage_count != embedded.len() {
        return Err(format!(
            "Semantic embedding count mismatch: sent {}, received {}",
            passage_count,
            embedded.len()
        ));
    }
    for vector in &embedded {
        validate_embedding(vector)?;
    }
    // 断片ごとのベクトルに戻す。再利用した断片は計算していないので None にする。
    let mut embedded = embedded.into_iter();
    let mut vectors = Vec::new();
    for (chunks, previous) in chunks_by_doc.iter().zip(&previous) {
        let metadata_known = previous.as_ref().is_some_and(|p| p.metadata.is_some());
        let content_known = previous.as_ref().is_some_and(|p| p.content.is_some());
        for chunk in chunks {
            let known = match chunk.field.as_str() {
                "metadata" => metadata_known,
                _ => content_known,
            };
            vectors.push(if known {
                None
            } else {
                Some(
                    embedded
                        .next()
                        .ok_or("Semantic work vector range is incomplete")?,
                )
            });
        }
    }

    let conn = open_index(storage_dir)?;
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| format!("Semantic transaction failed: {}", e))?;
    for doc in docs {
        tx.execute(
            "DELETE FROM semantic_works WHERE download_id = ?1",
            params![doc.download_id],
        )
        .map_err(|e| format!("Semantic clear failed: {}", e))?;
    }
    let mut offset = 0usize;
    let mut indexed = Vec::with_capacity(docs.len());
    for ((doc, chunks), previous) in docs.iter().zip(chunks_by_doc.iter()).zip(&previous) {
        let end = offset.saturating_add(chunks.len());
        let doc_vectors = vectors
            .get_mut(offset..end)
            .ok_or_else(|| "Semantic work vector range is incomplete".to_string())?;
        offset = end;

        let (content_vector, content_chunk_count) =
            match previous.as_ref().and_then(|p| p.content.clone()) {
                Some(content) => {
                    report.reused_content += 1;
                    content
                }
                None => {
                    let body_vectors = chunks
                        .iter()
                        .zip(doc_vectors.iter())
                        .filter(|(chunk, _)| chunk.field == "body")
                        .map(|(_, vector)| {
                            vector
                                .as_deref()
                                .ok_or("Semantic body embedding is missing")
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    (
                        mean_normalized_vector(&body_vectors),
                        body_vectors.len() as i64,
                    )
                }
            };
        let metadata_index = chunks.iter().position(|chunk| chunk.field == "metadata");
        let reused_metadata = previous.as_ref().and_then(|p| p.metadata.clone());
        let (metadata_text, metadata_vector) = match (metadata_index, reused_metadata) {
            (Some(index), Some(vector)) => {
                report.reused_metadata += 1;
                (chunks[index].text.as_str(), vector)
            }
            (Some(index), None) => {
                let vector = doc_vectors[index]
                    .take()
                    .ok_or("Semantic metadata embedding is missing")?;
                let Some(vector) = unit_vector(vector) else {
                    log::warn!(
                        "Semantic metadata vector for {} had no direction; skipping",
                        doc.download_id
                    );
                    continue;
                };
                (chunks[index].text.as_str(), vector)
            }
            (None, _) => {
                // 説明がない作品では本文の重心と先頭の断片を説明の代わりに使う。
                let Some(vector) = content_vector.clone() else {
                    continue;
                };
                (
                    chunks.first().map_or("", |chunk| chunk.text.as_str()),
                    vector,
                )
            }
        };

        tx.execute(
            "INSERT OR REPLACE INTO semantic_works (
                download_id, metadata_text, metadata_hash, content_hash,
                model_id, dimension, metadata_vector, content_vector, content_chunk_count,
                chunking_version, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                doc.download_id,
                metadata_text,
                hash_text(metadata_text),
                hash_text(&doc.body),
                MODEL_ID,
                VECTOR_DIMENSION as i64,
                vector_to_blob(&metadata_vector),
                content_vector.as_deref().map(vector_to_blob),
                content_chunk_count,
                CHUNKING_VERSION,
                chrono::Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|e| format!("Semantic work insert failed: {e}"))?;
        indexed.push(doc.download_id);
    }
    tx.commit()
        .map_err(|e| format!("Semantic transaction commit failed: {}", e))?;
    report.indexed = indexed;
    Ok(report)
}

/// 前の索引から、文字列が同じ部分のベクトルを読む。説明は説明の断片の文字列、本文は本文の
/// 全文のハッシュ・断片数・分割規則の版で比べる。
fn previous_vectors(
    reuse: &ReusableVectors<'_>,
    docs: &[SemanticIndexDocument],
    chunks_by_doc: &[Vec<ChunkInput>],
) -> Result<Vec<Option<Previous>>, String> {
    if !index_path(reuse.storage_dir).exists() {
        return Ok(vec![None; docs.len()]);
    }
    let conn = open_index(reuse.storage_dir)?;
    let mut statement = conn
        .prepare_cached(
            "SELECT metadata_hash,content_hash,metadata_vector,content_vector,content_chunk_count,
                    chunking_version
               FROM semantic_works WHERE download_id=?1 AND model_id=?2 AND dimension=?3",
        )
        .map_err(|e| format!("Semantic reuse prepare failed: {e}"))?;
    let mut previous = Vec::with_capacity(docs.len());
    for (doc, chunks) in docs.iter().zip(chunks_by_doc) {
        let Some(old_id) = reuse.previous_ids.get(&doc.download_id) else {
            previous.push(None);
            continue;
        };
        let row = statement
            .query_row(params![old_id, MODEL_ID, VECTOR_DIMENSION as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Option<Vec<u8>>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .optional()
            .map_err(|e| format!("Semantic reuse read failed: {e}"))?;
        let Some((
            metadata_hash,
            content_hash,
            metadata_blob,
            content_blob,
            chunk_count,
            chunking_version,
        )) = row
        else {
            previous.push(None);
            continue;
        };
        let body_chunks = chunks.iter().filter(|chunk| chunk.field == "body").count() as i64;
        let content = if content_hash == hash_text(&doc.body)
            && chunk_count == body_chunks
            && chunking_version == CHUNKING_VERSION
        {
            match content_blob.as_deref() {
                Some(blob) => {
                    decode_vector(blob).map(|vector| (Some(vector.to_vec()), chunk_count))
                }
                None if body_chunks == 0 => Some((None, 0)),
                None => None,
            }
        } else {
            None
        };
        let metadata = chunks
            .iter()
            .find(|chunk| chunk.field == "metadata")
            .filter(|chunk| hash_text(&chunk.text) == metadata_hash)
            .and_then(|_| decode_vector(&metadata_blob))
            .map(|vector| vector.to_vec());
        previous.push(Some(Previous { metadata, content }));
    }
    Ok(previous)
}

#[derive(Debug)]
pub(crate) struct RankedSemanticCandidate {
    pub download_id: i64,
    pub score: f64,
    content: bool,
}

impl PartialEq for RankedSemanticCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.score.total_cmp(&other.score) == Ordering::Equal
            && self.download_id == other.download_id
    }
}

impl Eq for RankedSemanticCandidate {}

impl PartialOrd for RankedSemanticCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RankedSemanticCandidate {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .total_cmp(&other.score)
            // At an equal score the smaller id is the stable better result.
            .then_with(|| other.download_id.cmp(&self.download_id))
    }
}

/// 索引のファイルがあるか（なければ、記録の上で作った作品も作り直す）。
pub fn index_exists(storage_dir: &Path) -> bool {
    index_path(storage_dir).exists()
}

/// 複数の作品を一つのトランザクションで削除する。
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
            .prepare("DELETE FROM semantic_works WHERE download_id = ?1")
            .map_err(|e| format!("Semantic clear prepare failed: {e}"))?;
        for download_id in download_ids {
            statement
                .execute(params![download_id])
                .map_err(|e| format!("Semantic clear failed for {download_id}: {e}"))?;
        }
    }
    tx.commit()
        .map_err(|e| format!("Semantic clear commit failed: {e}"))?;
    Ok(())
}

pub fn clear_all_works(storage_dir: &Path) -> Result<(), String> {
    let conn = open_index(storage_dir)?;
    conn.execute("DELETE FROM semantic_works", [])
        .map_err(|e| format!("Semantic work index clear failed: {e}"))?;
    Ok(())
}

/// 候補を絞り込み、同じ索引のスナップショットから表示対象の説明を読む。
pub(crate) fn search_selected<T, E>(
    storage_dir: &Path,
    query: &str,
    select: impl FnOnce(&mut Vec<RankedSemanticCandidate>) -> Result<T, E>,
    index_error: impl Fn(String) -> E,
) -> Result<(T, Vec<SemanticSearchHit>), E> {
    let query_text = semantic_query_text(query);
    if query_text.is_empty() {
        return Ok((select(&mut Vec::new())?, Vec::new()));
    }

    let vectors = embed_texts(vec![format!("query: {query_text}")]).map_err(&index_error)?;
    let query_vector = normalize_query_embedding(vectors).map_err(&index_error)?;
    let mut conn = open_index(storage_dir).map_err(&index_error)?;
    search_snapshot(&mut conn, &query_vector, usize::MAX, select, index_error)
}

fn normalize_query_embedding(vectors: Vec<Vec<f32>>) -> Result<Vec<f32>, String> {
    let vector = vectors
        .into_iter()
        .next()
        .ok_or("Semantic query embedding returned no vector")?;
    validate_embedding(&vector)?;
    unit_vector(vector).ok_or_else(|| "Semantic query embedding has no direction".to_string())
}

fn search_snapshot<T, E>(
    conn: &mut Connection,
    query_vector: &[f32],
    limit: usize,
    select: impl FnOnce(&mut Vec<RankedSemanticCandidate>) -> Result<T, E>,
    index_error: impl Fn(String) -> E,
) -> Result<(T, Vec<SemanticSearchHit>), E> {
    let tx = conn
        .transaction()
        .map_err(|e| index_error(format!("Semantic search transaction failed: {e}")))?;
    let mut candidates = score_search_candidates(&tx, query_vector, limit).map_err(&index_error)?;
    let selected = select(&mut candidates)?;
    let hits = read_search_hits(&tx, candidates).map_err(index_error)?;
    Ok((selected, hits))
}

#[cfg(test)]
fn search_by_vector(
    conn: &mut Connection,
    query_vector: &[f32],
    limit: usize,
) -> Result<Vec<SemanticSearchHit>, String> {
    let ((), mut hits) = search_snapshot(conn, query_vector, limit, |_| Ok(()), |error| error)?;
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.download_id.cmp(&b.download_id))
    });
    Ok(hits)
}

fn score_search_candidates(
    tx: &Transaction<'_>,
    query_vector: &[f32],
    limit: usize,
) -> Result<Vec<RankedSemanticCandidate>, String> {
    let mut statement = tx
        .prepare(
            "SELECT download_id, metadata_vector, content_vector
               FROM semantic_works
              WHERE model_id = ?1 AND dimension = ?2",
        )
        .map_err(|e| format!("Semantic work search prepare failed: {e}"))?;
    let rows = statement
        .query_map(params![MODEL_ID, VECTOR_DIMENSION as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
            ))
        })
        .map_err(|e| format!("Semantic work search failed: {e}"))?;
    // 点数を付けながら、返す件数分の上位候補だけを保持する。
    let mut candidates = BinaryHeap::<Reverse<RankedSemanticCandidate>>::new();
    for row in rows {
        let (download_id, metadata_blob, content_blob) =
            row.map_err(|e| format!("Semantic work search read failed: {e}"))?;
        let Some(metadata) = decode_vector(&metadata_blob) else {
            continue;
        };
        let metadata_score = cosine_similarity(query_vector, &metadata);
        let content_score = content_blob
            .as_deref()
            .and_then(decode_vector)
            .map(|vector| cosine_similarity(query_vector, &vector));
        let (score, content) = match content_score {
            Some(content_score) => {
                let combined = metadata_score * 0.35 + content_score * 0.65;
                if content_score >= metadata_score {
                    (combined.max(content_score * 0.9), true)
                } else {
                    (combined.max(metadata_score * 0.9), false)
                }
            }
            None => (metadata_score, false),
        };
        if score >= 0.18 {
            let candidate = RankedSemanticCandidate {
                download_id,
                score,
                content,
            };
            if candidates.len() < limit {
                candidates.push(Reverse(candidate));
            } else if candidates.peek().is_some_and(|worst| candidate > worst.0) {
                candidates.pop();
                candidates.push(Reverse(candidate));
            }
        }
    }

    Ok(candidates.into_iter().map(|entry| entry.0).collect())
}

fn read_search_hits(
    tx: &Transaction<'_>,
    candidates: Vec<RankedSemanticCandidate>,
) -> Result<Vec<SemanticSearchHit>, String> {
    // 本文の重心で選ばれた候補には定型文を使う。説明を表示する候補だけを一度に読む。
    let ids: Vec<_> = candidates
        .iter()
        .filter(|candidate| !candidate.content)
        .map(|candidate| candidate.download_id)
        .collect();
    let mut texts = HashMap::<i64, String>::new();
    if !ids.is_empty() {
        let keys =
            serde_json::to_string(&ids).map_err(|e| format!("Semantic result keys failed: {e}"))?;
        let mut statement = tx
            .prepare(
                "SELECT w.download_id, w.metadata_text
                   FROM json_each(?1) j JOIN semantic_works w ON w.download_id = j.value",
            )
            .map_err(|e| format!("Semantic result text prepare failed: {e}"))?;
        texts = statement
            .query_map([keys], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| format!("Semantic result text query failed: {e}"))?
            .collect::<rusqlite::Result<_>>()
            .map_err(|e| format!("Semantic result text read failed: {e}"))?;
    }
    candidates
        .into_iter()
        .map(|candidate| {
            let (field, text) = if candidate.content {
                (
                    "content",
                    "本文全体の傾向が近い候補です。個別の一致箇所は特定していません。".to_string(),
                )
            } else {
                let text = texts.remove(&candidate.download_id).ok_or_else(|| {
                    format!("Semantic result text missing for {}", candidate.download_id)
                })?;
                ("metadata", text)
            };
            Ok(SemanticSearchHit {
                download_id: candidate.download_id,
                field: field.to_string(),
                text,
                score: candidate.score,
            })
        })
        .collect()
}

/// 作品ごとの本文ベクトル。全チャンクの平均を、長さ1にそろえて返す。
///
/// 索引には削除済み作品の行が残っていることがあるので、呼び出し側が
/// 実在する id を渡す。ここでは渡された id 以外を返さない。
pub fn work_centroids(
    storage_dir: &Path,
    wanted: &HashSet<i64>,
) -> Result<HashMap<i64, Vec<f32>>, String> {
    if wanted.is_empty() {
        return Ok(HashMap::new());
    }
    let conn = open_index(storage_dir)?;
    let mut stmt = conn
        .prepare(
            "SELECT download_id, content_vector FROM semantic_works
              WHERE content_vector IS NOT NULL AND model_id = ?1 AND dimension = ?2",
        )
        .map_err(|e| format!("Semantic centroid prepare failed: {e}"))?;
    let rows = stmt
        .query_map(params![MODEL_ID, VECTOR_DIMENSION as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(|e| format!("Semantic centroid query failed: {e}"))?;

    let mut out = HashMap::new();
    for row in rows {
        let (download_id, blob) = row.map_err(|e| format!("Semantic centroid read failed: {e}"))?;
        if !wanted.contains(&download_id) {
            continue;
        }
        if let Some(vector) = decode_vector(&blob) {
            out.insert(download_id, vector.to_vec());
        }
    }
    Ok(out)
}

/// 索引済みの作品ベクトルから類似作品を探す。埋め込みモデルは読み込まない。
pub fn similar_works(
    storage_dir: &Path,
    seed_ids: &[i64],
    limit: usize,
) -> Result<Vec<(i64, f64)>, String> {
    if seed_ids.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let mut conn = open_index(storage_dir)?;
    let tx = conn
        .transaction()
        .map_err(|e| format!("Semantic related-work transaction failed: {e}"))?;
    let seed_set = seed_ids.iter().copied().collect::<HashSet<_>>();
    let mut seed_statement = tx
        .prepare(
            "SELECT metadata_vector, content_vector
               FROM semantic_works
              WHERE download_id = ?1 AND model_id = ?2 AND dimension = ?3",
        )
        .map_err(|e| format!("Semantic seed prepare failed: {e}"))?;
    let mut seeds = Vec::with_capacity(seed_set.len());
    for id in &seed_set {
        let row = seed_statement
            .query_row(params![id, MODEL_ID, VECTOR_DIMENSION as i64], |row| {
                Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Option<Vec<u8>>>(1)?))
            })
            .optional()
            .map_err(|e| format!("Semantic seed read failed: {e}"))?;
        let Some((metadata, content)) = row else {
            continue;
        };
        let Some(metadata) = decode_vector(&metadata) else {
            continue;
        };
        let content = match content.as_deref() {
            Some(blob) => match decode_vector(blob) {
                Some(vector) => Some(vector),
                None => continue,
            },
            None => None,
        };
        seeds.push((metadata, content));
    }
    drop(seed_statement);
    if seeds.is_empty() {
        return Ok(Vec::new());
    }

    let mut statement = tx
        .prepare(
            "SELECT download_id, metadata_vector, content_vector
               FROM semantic_works
              WHERE model_id = ?1 AND dimension = ?2",
        )
        .map_err(|e| format!("Semantic related-work prepare failed: {e}"))?;
    let rows = statement
        .query_map(params![MODEL_ID, VECTOR_DIMENSION as i64], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
            ))
        })
        .map_err(|e| format!("Semantic related-work query failed: {e}"))?;

    let mut candidates = BinaryHeap::<Reverse<RankedSemanticCandidate>>::new();
    for row in rows {
        let (id, metadata, content) =
            row.map_err(|e| format!("Semantic related-work read failed: {e}"))?;
        if seed_set.contains(&id) {
            continue;
        }
        let Some(metadata) = decode_vector(&metadata) else {
            continue;
        };
        let content = content.as_deref().and_then(decode_vector);
        let score = seeds
            .iter()
            .map(|(seed_metadata, seed_content)| {
                let metadata_score = cosine_similarity(seed_metadata, &metadata);
                match (seed_content.as_ref(), content.as_ref()) {
                    (Some(seed), Some(candidate)) => {
                        metadata_score * 0.30 + cosine_similarity(seed, candidate) * 0.70
                    }
                    _ => metadata_score,
                }
            })
            .fold(0.0_f64, f64::max);
        if score < 0.18 {
            continue;
        }
        let candidate = RankedSemanticCandidate {
            download_id: id,
            score,
            content: false,
        };
        if candidates.len() < limit {
            candidates.push(Reverse(candidate));
        } else if candidates.peek().is_some_and(|worst| candidate > worst.0) {
            candidates.pop();
            candidates.push(Reverse(candidate));
        }
    }
    drop(statement);
    drop(tx);
    let mut scored = candidates
        .into_iter()
        .map(|entry| (entry.0.download_id, entry.0.score))
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    Ok(scored)
}

pub fn status(storage_dir: &Path) -> SemanticIndexStatus {
    release_model_if_idle();
    let Ok(conn) = open_index(storage_dir) else {
        return SemanticIndexStatus {
            indexed_works: 0,
            model_ready: semantic_model_ready(),
            provider: embedding_provider(),
            gpu_enabled: embedding_gpu_enabled(),
        };
    };
    let indexed_works = conn
        .query_row(
            "SELECT COUNT(*) FROM semantic_works WHERE model_id = ?1 AND dimension = ?2",
            params![MODEL_ID, VECTOR_DIMENSION as i64],
            |row| row.get(0),
        )
        .unwrap_or(0);
    SemanticIndexStatus {
        indexed_works,
        model_ready: semantic_model_ready(),
        provider: embedding_provider(),
        gpu_enabled: embedding_gpu_enabled(),
    }
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
            field: "metadata".to_string(),
            text: meta,
        });
    }
    for text in split_text_chunks(&doc.body) {
        chunks.push(ChunkInput {
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

/// いま使っている問いの作り方。measure 用に公開している。
/// 意味検索に渡す問い。**索引側と同じ姿で渡す。**
///
/// 索引側は題名も本文もそのまま `passage: {原文}` として埋め込んでいる
/// （`build_chunks` を見ること）。だから問いも `query: {原文}` でよい。
///
/// 0.11.0 まではここで、かな・ローマ字・同義語を空白で連ねた袋を作っていた。
/// multilingual-e5 は**両側に自然文を期待する**モデルなので、片側だけ語彙の
/// 袋にすると埋め込みが本来の位置から離れる。棚（163,393 断片・188 作品）で
/// 測ったのが次の表で、本文の一文を問いにして、その作品が何位に出るか:
///
/// | 問いの作り方     |  1位  |  MRR  |
/// |------------------|-------|-------|
/// | 変形形＋同義語   |  6.9% | 0.098 |
/// | 正規化した自然文 | 50.5% | 0.546 |
/// | **そのまま**     | 69.7% | 0.739 |
///
/// 題名を問いにしたときも 26.6% → 69.7%（MRR 0.340 → 0.756）で同じ向き。
/// 測り直すときは、棚の実物で問いと当たりを数えて比べる。
///
/// かなやローマ字で引き当てるのは**字面の索引（tantivy）の仕事**で、
/// 検索はその二つを束ねている。ここで面倒を見る必要はない。
pub fn semantic_query_text(query: &str) -> String {
    query.trim().to_string()
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
    let mut conn =
        Connection::open(&path).map_err(|e| format!("Semantic index open failed: {}", e))?;
    init_index_schema(&mut conn).map_err(|e| format!("Semantic schema init failed: {e}"))?;
    Ok(conn)
}

fn init_index_schema(conn: &mut Connection) -> Result<(), Box<dyn std::error::Error>> {
    let version =
        |conn: &Connection| conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0));
    match version(conn)? {
        INDEX_SCHEMA_VERSION => return Ok(()),
        0 => {}
        other => return Err(format!("Unsupported semantic index schema version: {other}").into()),
    }

    conn.execute_batch("PRAGMA journal_mode = WAL;")?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    match version(&tx)? {
        INDEX_SCHEMA_VERSION => return Ok(()),
        0 => {}
        other => return Err(format!("Unsupported semantic index schema version: {other}").into()),
    }
    let has_works = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='semantic_works')",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if has_works {
        // 既存索引の分割規則は版1。行やベクトルを作り直さずに列だけ移行する。
        tx.execute_batch(
            "ALTER TABLE semantic_works ADD COLUMN chunking_version INTEGER NOT NULL DEFAULT 1;
             ALTER TABLE semantic_works DROP COLUMN content_preview;",
        )?;
    } else {
        tx.execute_batch(
            "CREATE TABLE semantic_works (
            download_id INTEGER PRIMARY KEY,
            metadata_text TEXT NOT NULL,
            metadata_hash TEXT NOT NULL,
            content_hash TEXT NOT NULL,
            model_id TEXT NOT NULL,
            dimension INTEGER NOT NULL,
            metadata_vector BLOB NOT NULL,
            content_vector BLOB,
            content_chunk_count INTEGER NOT NULL,
            chunking_version INTEGER NOT NULL DEFAULT 1,
            updated_at TEXT NOT NULL
             );
             CREATE INDEX idx_semantic_works_model ON semantic_works(model_id, dimension);",
        )?;
    }
    tx.pragma_update(None, "user_version", INDEX_SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
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
    start_model_reaper();
    let model = MODEL.get_or_init(|| Mutex::new(EmbeddingRuntimeSlot::default()));
    let mut slot = model
        .lock()
        .map_err(|e| format!("Semantic model lock failed: {}", e))?;
    if slot.runtime.is_none() {
        slot.runtime =
            Some(init_embedding_runtime().map_err(|e| format!("Semantic model init failed: {e}"))?);
    }
    let runtime = slot
        .runtime
        .as_mut()
        .expect("semantic runtime was initialized above");
    let batch_size = runtime.gpu_enabled.then_some(GPU_EMBEDDING_BATCH_SIZE);
    let result = runtime
        .model
        .embed(texts, batch_size)
        .map_err(|e| format!("Semantic embedding failed: {}", e));
    slot.last_used = Some(Instant::now());
    result
}

#[cfg(not(test))]
fn start_model_reaper() {
    MODEL_REAPER_STARTED.get_or_init(|| {
        if let Err(error) = std::thread::Builder::new()
            .name("semantic-model-reaper".to_string())
            .spawn(|| loop {
                std::thread::sleep(Duration::from_secs(60));
                release_model_if_idle();
            })
        {
            log::warn!("意味検索モデルの待機時解放スレッドを開始できません: {error}");
        }
    });
}

#[cfg(not(test))]
fn release_model_if_idle() {
    let Some(model) = MODEL.get() else {
        return;
    };
    let removed = {
        // **待たない。** 埋め込みの最中なら鍵は取れないが、そのときはそもそも
        // idle ではないので解放する用も無い。ここで待つと、状態を知りたいだけの
        // 呼び出しが1バッチぶん（実測で40秒台）止まる。
        let Ok(mut slot) = model.try_lock() else {
            return;
        };
        let idle = slot
            .last_used
            .is_some_and(|last_used| last_used.elapsed() >= MODEL_IDLE_TIMEOUT);
        if !idle {
            return;
        }
        slot.last_used = None;
        slot.runtime.take()
    };
    drop(removed);
    log::info!("5分間使われていない意味検索モデルをメモリから解放しました");
}

#[cfg(test)]
fn release_model_if_idle() {}

#[cfg(not(test))]
fn init_embedding_runtime() -> Result<EmbeddingRuntime, String> {
    #[cfg(windows)]
    {
        let gpu_options = InitOptions::new(EmbeddingModel::MultilingualE5Small)
            .with_cache_dir(model_cache_dir())
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
            .with_cache_dir(model_cache_dir())
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
    // 鍵が取れないのは、誰かが埋め込みに使っている最中である。つまり読み込み
    // 済みなので、待たずに「使える」と答えてよい。
    let Some(model) = MODEL.get() else {
        return false;
    };
    match model.try_lock() {
        Ok(slot) => slot.runtime.is_some(),
        Err(std::sync::TryLockError::WouldBlock) => true,
        Err(std::sync::TryLockError::Poisoned(_)) => false,
    }
}

#[cfg(test)]
fn semantic_model_ready() -> bool {
    true
}

#[cfg(not(test))]
fn embedding_provider() -> String {
    // ここも待たない。取れなければ「読み込み済みだが今は使用中」なので、
    // 名前が一時的に出ないだけで済ませる。
    MODEL
        .get()
        .and_then(|slot| slot.try_lock().ok())
        .and_then(|slot| {
            slot.runtime
                .as_ref()
                .map(|runtime| runtime.provider.clone())
        })
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
    // 状態を読むだけの経路は、埋め込みの終わりを待たない。
    MODEL
        .get()
        .and_then(|slot| slot.try_lock().ok())
        .and_then(|slot| slot.runtime.as_ref().map(|runtime| runtime.gpu_enabled))
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

fn decode_vector(blob: &[u8]) -> Option<[f32; VECTOR_DIMENSION]> {
    let (bytes, remainder) = blob.as_chunks::<4>();
    if !remainder.is_empty() {
        return None;
    }
    let bytes: &[[u8; 4]; VECTOR_DIMENSION] = bytes.try_into().ok()?;
    let vector = bytes.map(f32::from_le_bytes);
    vector
        .iter()
        .all(|value| value.is_finite())
        .then_some(vector)
}

fn validate_embedding(vector: &[f32]) -> Result<(), String> {
    if vector.len() != VECTOR_DIMENSION {
        return Err(format!(
            "Semantic model dimension mismatch: expected {VECTOR_DIMENSION}, got {}",
            vector.len()
        ));
    }
    if vector.iter().any(|value| !value.is_finite()) {
        return Err("Semantic embedding contains a non-finite value".to_string());
    }
    Ok(())
}

fn mean_normalized_vector(vectors: &[&[f32]]) -> Option<Vec<f32>> {
    if vectors.is_empty() {
        return None;
    }
    let mut sum = vec![0.0_f64; VECTOR_DIMENSION];
    for vector in vectors {
        if vector.len() != VECTOR_DIMENSION {
            return None;
        }
        for (slot, value) in sum.iter_mut().zip(vector.iter()) {
            *slot += *value as f64;
        }
    }
    let norm = sum.iter().map(|value| value * value).sum::<f64>().sqrt();
    (norm > f64::EPSILON).then(|| sum.into_iter().map(|value| (value / norm) as f32).collect())
}

/// 向きだけを残して、長さを1にそろえる。
///
/// `cosine_similarity` は内積をそのまま余弦として使う。それが成り立つのは
/// **両方が長さ1のとき**だけである。本文の重心は `mean_normalized_vector` が
/// そろえているが、メタデータの向きは埋め込みが返したものをそのまま保存して
/// いた。つまり「fastembed は正規化して返す」という、どこにも書いていない
/// 前提の上に余弦が乗っていた。
///
/// しかも破れても見えない。`cosine_similarity` の `.clamp(0.0, 1.0)` が、
/// 1を超えた内積を 1.0 に丸めて隠してしまう。モデルを差し替えた日に、
/// 「なんとなく結果が変わった」としてしか現れない種類の壊れ方である。
///
/// すでに長さ1なら、これは何もしない。ただならぬのは、そうでなかったときに
/// **何も起きないこと**のほうである。
fn unit_vector(vector: Vec<f32>) -> Option<Vec<f32>> {
    let norm = vector
        .iter()
        .map(|value| *value as f64 * *value as f64)
        .sum::<f64>()
        .sqrt();
    (norm > f64::EPSILON).then(|| {
        vector
            .into_iter()
            .map(|value| (value as f64 / norm) as f32)
            .collect()
    })
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> f64 {
    debug_assert_eq!(left.len(), right.len());
    left.iter()
        .zip(right.iter())
        .map(|(left, right)| *left as f64 * *right as f64)
        .sum::<f64>()
        .clamp(0.0, 1.0)
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

    struct TempIndex(PathBuf);

    impl TempIndex {
        fn new() -> Self {
            Self(
                std::env::temp_dir()
                    .join(format!("piep-semantic-{}", uuid::Uuid::now_v7()))
                    .join("semantic"),
            )
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempIndex {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    fn first_axis() -> [f32; VECTOR_DIMENSION] {
        let mut vector = [0.0; VECTOR_DIMENSION];
        vector[0] = 1.0;
        vector
    }

    #[test]
    fn malformed_centroids_are_never_returned() {
        let temp = TempIndex::new();
        let conn = open_index(temp.path()).unwrap();
        let mut direction = first_axis();
        assert!(validate_embedding(&direction).is_ok());
        let valid = vector_to_blob(&direction);
        let mut trailing_byte = valid.clone();
        trailing_byte.push(0);
        direction[0] = f32::NAN;
        let nan = vector_to_blob(&direction);
        assert!(validate_embedding(&[]).is_err());
        assert!(validate_embedding(&direction[..VECTOR_DIMENSION - 1]).is_err());
        assert!(validate_embedding(&direction).is_err());
        let cases = [
            (1, None),
            (2, Some(Vec::new())),
            (3, Some(trailing_byte)),
            (4, Some(nan)),
            (5, Some(valid.clone())),
            (6, Some(valid[..valid.len() - 4].to_vec())),
        ];
        for (id, content) in cases {
            conn.execute(
                "INSERT INTO semantic_works (
                    download_id, metadata_text, metadata_hash, content_hash,
                    model_id, dimension, metadata_vector, content_vector, content_chunk_count,
                    updated_at
                 ) VALUES (?1, '', '', '', ?2, ?3, ?4, ?5, 1, '')",
                params![id, MODEL_ID, VECTOR_DIMENSION as i64, valid, content],
            )
            .unwrap();
        }
        let centroids = work_centroids(temp.path(), &HashSet::from([1, 2, 3, 4, 5, 6])).unwrap();
        assert_eq!(
            centroids.keys().copied().collect::<HashSet<_>>(),
            HashSet::from([5])
        );
        // NULL は本文が未計算の種として使えるが、不正な BLOB がある種は使わない。
        assert!(!similar_works(temp.path(), &[1], 10).unwrap().is_empty());
        for id in [2, 3, 4, 6] {
            assert!(similar_works(temp.path(), &[id], 10).unwrap().is_empty());
        }
    }

    #[test]
    fn centroid_reuse_requires_the_same_chunking_rule() {
        let temp = TempIndex::new();
        let doc = SemanticIndexDocument {
            download_id: 1,
            title: "題".into(),
            author_name: String::new(),
            tags: String::new(),
            series_title: String::new(),
            excerpt: String::new(),
            body: "本文".into(),
        };
        let path = index_path(temp.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let metadata = vector_to_blob(&first_axis());
        let mut direction = first_axis();
        direction.swap(0, 1);
        let body = vector_to_blob(&direction);
        {
            let old = Connection::open(path).unwrap();
            old.execute_batch(
                "PRAGMA journal_mode = WAL;
                 CREATE TABLE semantic_works (
                    download_id INTEGER PRIMARY KEY,
                    metadata_text TEXT NOT NULL,
                    content_preview TEXT NOT NULL,
                    metadata_hash TEXT NOT NULL,
                    content_hash TEXT NOT NULL,
                    model_id TEXT NOT NULL,
                    dimension INTEGER NOT NULL,
                    metadata_vector BLOB NOT NULL,
                    content_vector BLOB,
                    content_chunk_count INTEGER NOT NULL,
                    updated_at TEXT NOT NULL
                 );
                 CREATE INDEX idx_semantic_works_model ON semantic_works(model_id, dimension);",
            )
            .unwrap();
            old.execute(
                "INSERT INTO semantic_works VALUES (1, ?1, '本文', ?2, ?3, ?4, ?5, ?6, ?7, 1, '')",
                params![
                    doc.title,
                    hash_text(&doc.title),
                    hash_text(&doc.body),
                    MODEL_ID,
                    VECTOR_DIMENSION as i64,
                    metadata,
                    body,
                ],
            )
            .unwrap();
        }
        let docs = [doc];
        let reuse = ReusableVectors {
            storage_dir: temp.path(),
            previous_ids: &HashMap::from([(1, 1)]),
        };
        let conn = open_index(temp.path()).unwrap();
        let migrated: (i64, Vec<u8>, Vec<u8>) = conn.query_row(
            "SELECT chunking_version, metadata_vector, content_vector FROM semantic_works WHERE download_id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        assert_eq!(migrated, (1, metadata, body));
        let preview_count: i64 = conn.query_row(
            "SELECT count(*) FROM pragma_table_info('semantic_works') WHERE name='content_preview'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(preview_count, 0);
        let chunks = [build_chunks(&docs[0])];
        let read = || {
            previous_vectors(&reuse, &docs, &chunks)
                .unwrap()
                .pop()
                .unwrap()
                .unwrap()
        };
        assert_eq!(read().content, Some((Some(direction.to_vec()), 1)));
        conn.execute(
            "UPDATE semantic_works SET chunking_version=?1",
            [CHUNKING_VERSION + 1],
        )
        .unwrap();
        let previous = read();
        assert!(previous.metadata.is_some());
        assert!(previous.content.is_none());
    }

    #[test]
    fn scored_work_remains_visible_after_concurrent_delete() {
        let temp = TempIndex::new();
        let mut reader = open_index(temp.path()).unwrap();
        let writer = open_index(temp.path()).unwrap();
        let query = first_axis();
        writer
            .execute(
                "INSERT INTO semantic_works (
                download_id, metadata_text, metadata_hash, content_hash,
                model_id, dimension, metadata_vector, content_chunk_count, updated_at
             ) VALUES (1, '採点時の説明', '', '', ?1, ?2, ?3, 0, ''),
                      (2, X'FF', '', '', ?1, ?2, ?3, 0, '')",
                params![MODEL_ID, VECTOR_DIMENSION as i64, vector_to_blob(&query)],
            )
            .unwrap();

        let ((), hits) = search_snapshot(
            &mut reader,
            &query,
            2,
            |candidates| {
                assert_eq!(candidates.len(), 2);
                // 表示しない作品の説明を読めば、型の違う値で検索が失敗する。
                candidates.retain(|candidate| candidate.download_id == 1);
                writer
                    .execute("DELETE FROM semantic_works WHERE download_id=1", [])
                    .unwrap();
                Ok(())
            },
            |error| error,
        )
        .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].download_id, 1);
        assert_eq!(hits[0].text, "採点時の説明");
        assert_eq!(hits[0].score, 1.0);
        writer
            .execute("DELETE FROM semantic_works WHERE download_id=2", [])
            .unwrap();
        assert!(search_by_vector(&mut reader, &query, 1).unwrap().is_empty());
    }

    /// `PIEP_SEMANTIC_PROBE` に意味索引 DB の**写し**、`PIEP_SEMANTIC_PROBE_MODEL` に
    /// 使用したモデルのスナップショットを指定する。写しには本番と同じ索引の移行を当ててから、
    /// モデルをダウンロードせずに固定の3問の上位10件を出す（実物の索引を指さないこと）。
    // 通常の単体試験は疑似埋め込みを使う。実モデルのリンクは比較を行うときだけ有効にする。
    #[cfg(feature = "semantic-probe")]
    #[test]
    #[ignore]
    fn probe_semantic_ranking() {
        use fastembed::{
            InitOptionsUserDefined, Pooling, TextEmbedding, TokenizerFiles,
            UserDefinedEmbeddingModel,
        };

        let path =
            std::env::var_os("PIEP_SEMANTIC_PROBE").expect("意味索引DBの写しを指定してください");
        let model_dir = PathBuf::from(
            std::env::var_os("PIEP_SEMANTIC_PROBE_MODEL")
                .expect("埋め込みモデルのスナップショットの写しを指定してください"),
        );
        let mut conn = Connection::open(path).expect("意味索引DBの写しを開けません");
        init_index_schema(&mut conn).expect("意味索引DBの写しを移行できません");
        let read = |name| std::fs::read(model_dir.join(name)).unwrap();
        let files = TokenizerFiles {
            tokenizer_file: read("tokenizer.json"),
            config_file: read("config.json"),
            special_tokens_map_file: read("special_tokens_map.json"),
            tokenizer_config_file: read("tokenizer_config.json"),
        };
        let model = UserDefinedEmbeddingModel::new(read("onnx/model.onnx"), files)
            .with_pooling(Pooling::Mean);
        let mut model =
            TextEmbedding::try_new_from_user_defined(model, InitOptionsUserDefined::new())
                .expect("埋め込みモデルを読み込めません");
        for query in [
            "幼なじみとの再会",
            "失われた記憶を取り戻す旅",
            "静かな日常に起きた小さな変化",
        ] {
            let vectors = model
                .embed(vec![format!("query: {}", semantic_query_text(query))], None)
                .unwrap();
            let vector = normalize_query_embedding(vectors).unwrap();
            println!("検索語 {query}");
            for hit in search_by_vector(&mut conn, &vector, 10).unwrap() {
                println!("{}\t{:.17}", hit.download_id, hit.score);
            }
        }
    }

    /// 内積を余弦として使う以上、しまう向きは長さ1でなければならない。
    ///
    /// 埋め込みの実装が正規化して返すことに頼っていた。頼れなくなった日に
    /// 気づけない — `cosine_similarity` の丸めが、1を超えた内積を隠すからである。
    #[test]
    fn a_direction_is_stored_with_unit_length() {
        let stretched = vec![3.0_f32, 4.0, 0.0];
        let unit = unit_vector(stretched).expect("向きがある");
        let norm = unit
            .iter()
            .map(|value| *value as f64 * *value as f64)
            .sum::<f64>()
            .sqrt();
        assert!((norm - 1.0).abs() < 1e-6, "{norm}");

        // すでに長さ1のものは、そのまま。正規化が結果を動かさない。
        let already = vec![1.0_f32, 0.0, 0.0];
        assert_eq!(unit_vector(already.clone()).unwrap(), already);
    }

    /// 長さの無い向きは、比べようがない。1.0 を返す代わりに何も返さない。
    #[test]
    fn a_vector_without_direction_is_not_normalised_into_one() {
        assert!(unit_vector(vec![0.0_f32; 8]).is_none());
    }

    /// 正規化しないままだと、余弦は 1.0 に丸められて**同じに見える**。
    ///
    /// この試験は壊れ方そのものを写している。長さ2の向きと自分自身の内積は
    /// 4.0 になるが、`cosine_similarity` はそれを 1.0 に丸める。長さをそろえて
    /// おけば、内積は本当に 1.0 になる。
    #[test]
    fn clamping_would_have_hidden_an_unnormalised_vector() {
        let stretched = vec![2.0_f32, 0.0, 0.0];
        // 丸めのせいで、間違った値と正しい値が見分けられない。
        assert_eq!(cosine_similarity(&stretched, &stretched), 1.0);
        let unit = unit_vector(stretched).unwrap();
        assert!((cosine_similarity(&unit, &unit) - 1.0).abs() < 1e-6);
        // 直交する向きは、そろえたあとでも直交したまま。
        let other = unit_vector(vec![0.0_f32, 5.0, 0.0]).unwrap();
        assert!(cosine_similarity(&unit, &other) < 1e-6);
    }

    /// 移してよいのは piep のモデルが入っているときだけ。名前の作り方を間違える
    /// と、他所の置き場を丸ごと持っていくか、自分のものを見落として取り直す。
    #[test]
    fn the_old_cache_is_recognised_by_the_model_it_holds() {
        assert_eq!(
            hf_hub_dir_name(MODEL_ID),
            "models--intfloat--multilingual-e5-small"
        );
    }

    /// モデルは棚の**隣**に置く。中へ入れてはならない。
    ///
    /// 診断が歩くのは `downloads` と `search` の二つで、前者の下にあるものは
    /// 身元不明のファイル、後者の下にあるものは索引の大きさとして数えられる。
    /// 465MB のモデルはそのどちらでもないので、どちらの下にも置かない。
    #[test]
    fn the_model_sits_beside_the_shelf_not_inside_it() {
        let storage = Path::new("C:/data/piep/downloads");
        let model = model_cache_dir_in(storage.parent().unwrap());
        assert_eq!(model, Path::new("C:/data/piep/models"));
        assert!(!model.starts_with(storage), "棚の中にモデルを置いている");
        let index_root = index_path(storage)
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .to_path_buf();
        assert!(
            !model.starts_with(&index_root),
            "索引の中にモデルを置いている: {index_root:?}"
        );
        assert_eq!(model.parent(), index_root.parent(), "同じ器に入っていない");
    }

    /// 問いは索引側と同じ姿でなければならない。**前置きを剥がさないこと。**
    ///
    /// ここに正規化や語の展開を足し戻すと、意味検索の当たりが目に見えて落ちる
    /// （[`semantic_query_text`] の表を見ること）。足すなら
    /// 棚の実物で測り直してからにすること。
    #[test]
    fn the_query_reaches_the_model_the_way_it_was_typed() {
        for query in [
            "雨上がりの図書室",
            "アイリ", // 片仮名は片仮名のまま。索引側も畳んでいない。
            "Cafe au lait",
            "彼女は窓の外を見て、何も言わなかった。",
        ] {
            assert_eq!(semantic_query_text(query), query);
        }
        assert_eq!(semantic_query_text("  余白  "), "余白");
    }
}
