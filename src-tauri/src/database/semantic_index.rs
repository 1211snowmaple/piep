use std::cmp::{Ordering, Reverse};
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
#[cfg(not(test))]
use std::time::{Duration, Instant};

#[cfg(not(test))]
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
#[cfg(not(test))]
use std::sync::Mutex;

#[cfg(test)]
use super::search_normalization::{query_variants, search_index_text};

const INDEX_DIR_NAME: &str = "search";
const INDEX_VERSION_DIR: &str = "semantic-v2";
const INDEX_FILE_NAME: &str = "index.sqlite";
const MODEL_ID: &str = "intfloat/multilingual-e5-small";
const VECTOR_DIMENSION: usize = 384;
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
/// fastembed が指定なしのときに使う置き場。**現在の作業ディレクトリ相対**。
const FASTEMBED_DEFAULT_CACHE_DIR: &str = ".fastembed_cache";
const MODEL_DIR_NAME: &str = "models";

/// 埋め込みモデルの置き場を、棚と同じ器の中に固定する。
///
/// fastembed の既定は `./.fastembed_cache`、つまり**起動した場所**である。
/// piep はこれを指定していなかったので、別の場所から起動するたびに 465MB の
/// モデルを落とし直し、その場に置き去りにしていた（この作業機にも二つあった）。
///
/// 置き場は `downloads` とも `search` とも**並べる**。診断が歩くのはこの二つ
/// なので、中に入れると 465MB が「身元不明のファイル」か「索引の大きさ」に
/// 化ける。モデルはそのどちらでもない。
///
/// 起動時に一度だけ呼ぶ。呼ばれなかったときは fastembed の既定のままにする。
pub fn set_model_cache_dir(storage_dir: &Path) {
    let target = model_cache_dir_for(storage_dir);
    if MODEL_CACHE_DIR.set(target.clone()).is_err() {
        return;
    }
    if target.exists() {
        return;
    }
    // すでに落としてあるものを捨てさせない。同じ器の中なら付け替えるだけで済む。
    //
    // ただし、**そこにあるのが piep のものだとは限らない。** fastembed を使う
    // 別のものが同じ名前で置いている可能性があるので、いま使うモデルが入って
    // いることを確かめてからでなければ動かさない。
    let legacy = Path::new(FASTEMBED_DEFAULT_CACHE_DIR);
    if !legacy.join(hf_hub_dir_name(MODEL_ID)).is_dir() {
        return;
    }
    if let Some(parent) = target.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    match std::fs::rename(legacy, &target) {
        Ok(()) => log::info!("埋め込みモデルの置き場を棚の隣へ移しました: {target:?}"),
        Err(error) => log::info!(
            "以前の置き場を移せないので、モデルは取り直しになります（{legacy:?}）: {error}"
        ),
    }
}

/// hf-hub がモデルを置くときのディレクトリ名（`a/b` → `models--a--b`）。
fn hf_hub_dir_name(model_id: &str) -> String {
    format!("models--{}", model_id.replace('/', "--"))
}

fn model_cache_dir_for(storage_dir: &Path) -> PathBuf {
    storage_dir
        .parent()
        .unwrap_or(storage_dir)
        .join(MODEL_DIR_NAME)
}

#[cfg(not(test))]
fn model_cache_dir() -> PathBuf {
    MODEL_CACHE_DIR
        .get()
        .cloned()
        .unwrap_or_else(|| PathBuf::from(FASTEMBED_DEFAULT_CACHE_DIR))
}

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
    /// Number of searchable works.  One row is always one library work.
    pub indexed_works: i64,
    /// Kept as a wire-compatible alias while the UI is renamed from chunks to
    /// works.  This is deliberately the work count, not a passage count.
    pub indexed_chunks: i64,
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

pub fn upsert_documents(
    storage_dir: &Path,
    docs: &[SemanticIndexDocument],
) -> Result<Vec<i64>, String> {
    if docs.is_empty() {
        return Ok(Vec::new());
    }

    // Keep the chunks grouped by work. Rebuilding every work's chunks again
    // after inference doubled the text scanning and allocations for long
    // documents, precisely while a large-library rebuild is already busy.
    let chunks_by_doc = docs.iter().map(build_chunks).collect::<Vec<_>>();

    let passages = chunks_by_doc
        .iter()
        .flatten()
        .map(|record| format!("passage: {}", record.text))
        .collect::<Vec<_>>();
    let passage_count = passages.len();
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
            "DELETE FROM semantic_works WHERE download_id = ?1",
            params![doc.download_id],
        )
        .map_err(|e| format!("Semantic clear failed: {}", e))?;
    }
    // **`zip` は短いほうで黙って打ち切る。** 直前にその作品の断片を全部
    // 消しているので、返ったベクトルが足りなければ、消したぶんの一部が二度と
    // 入らない。エラーにもならず、意味検索からその作品が静かに欠ける。
    if passage_count != vectors.len() {
        return Err(format!(
            "Semantic embedding count mismatch: sent {}, received {}",
            passage_count,
            vectors.len()
        ));
    }
    for vector in &vectors {
        if vector.len() != VECTOR_DIMENSION {
            return Err(format!(
                "Semantic model dimension mismatch: expected {}, got {}",
                VECTOR_DIMENSION,
                vector.len()
            ));
        }
    }

    let mut offset = 0usize;
    let mut indexed = Vec::with_capacity(docs.len());
    for (doc, chunks) in docs.iter().zip(chunks_by_doc.iter()) {
        let end = offset.saturating_add(chunks.len());
        let doc_vectors = vectors
            .get(offset..end)
            .ok_or_else(|| "Semantic work vector range is incomplete".to_string())?;
        offset = end;

        let body_vectors = chunks
            .iter()
            .zip(doc_vectors.iter())
            .filter(|(chunk, _)| chunk.field == "body")
            .map(|(_, vector)| vector.as_slice())
            .collect::<Vec<_>>();
        let content_vector = mean_normalized_vector(&body_vectors);
        let content_preview = chunks
            .iter()
            .find(|chunk| chunk.field == "body")
            .map(|chunk| chunk.text.as_str())
            .unwrap_or("");
        let metadata_index = chunks.iter().position(|chunk| chunk.field == "metadata");
        // A body-only work is still searchable. Previously it was skipped here
        // but its SQLite coverage row was recorded as successful, making it
        // permanently absent from semantic results. Use the normalized body
        // centroid as its metadata fallback and report only actually inserted
        // ids to the caller.
        let (metadata_text, metadata_vector) = match metadata_index {
            Some(index) => {
                let Some(vector) = unit_vector(doc_vectors[index].clone()) else {
                    log::warn!(
                        "Semantic metadata vector for {} had no direction; skipping",
                        doc.download_id
                    );
                    continue;
                };
                (chunks[index].text.as_str(), vector)
            }
            None => {
                let Some(vector) = content_vector.clone() else {
                    continue;
                };
                (content_preview, vector)
            }
        };

        tx.execute(
            "INSERT OR REPLACE INTO semantic_works (
                download_id, metadata_text, content_preview, metadata_hash, content_hash,
                model_id, dimension, metadata_vector, content_vector, content_chunk_count,
                updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                doc.download_id,
                metadata_text,
                content_preview,
                hash_text(metadata_text),
                hash_text(&doc.body),
                MODEL_ID,
                VECTOR_DIMENSION as i64,
                vector_to_blob(&metadata_vector),
                content_vector.as_deref().map(vector_to_blob),
                body_vectors.len() as i64,
                chrono::Utc::now().to_rfc3339(),
            ],
        )
        .map_err(|e| format!("Semantic work insert failed: {e}"))?;
        indexed.push(doc.download_id);
    }
    tx.commit()
        .map_err(|e| format!("Semantic transaction commit failed: {}", e))?;
    Ok(indexed)
}

#[derive(Debug)]
struct RankedSemanticCandidate {
    download_id: i64,
    score: f64,
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

pub fn search(
    storage_dir: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<SemanticSearchHit>, String> {
    search_with_query_text(storage_dir, &semantic_query_text(query), limit)
}

/// 問いの作り方だけを差し替えて測るための口。
///
/// 索引側は本文をそのまま `passage:` として渡すのに、検索側が何を `query:` と
/// して渡すかで結果が変わる。**どちらが良いかは実測でしか言えない**ので、
/// 前処理を外から与えられるようにしてある（`examples/semantic_query_probe.rs`）。
pub fn search_with_query_text(
    storage_dir: &Path,
    query_text: &str,
    limit: usize,
) -> Result<Vec<SemanticSearchHit>, String> {
    if query_text.trim().is_empty() || limit == 0 {
        return Ok(Vec::new());
    }

    let query_vector = embed_texts(vec![format!("query: {}", query_text)])?
        .into_iter()
        .next()
        .and_then(unit_vector)
        .ok_or_else(|| "Semantic query embedding returned no vector".to_string())?;

    let conn = open_index(storage_dir)?;
    let mut statement = conn
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
    // Keep only the requested top-k while scanning. The former Vec retained
    // and sorted every qualifying work, and also loaded both preview strings
    // for the entire library. Text is fetched only for the final results.
    let mut candidates = BinaryHeap::<Reverse<RankedSemanticCandidate>>::new();
    for row in rows {
        let (download_id, metadata_blob, content_blob) =
            row.map_err(|e| format!("Semantic work search read failed: {e}"))?;
        let metadata_score = cosine_similarity_blob(&query_vector, &metadata_blob);
        let content_score = content_blob
            .as_deref()
            .filter(|blob| blob.len() == VECTOR_DIMENSION * std::mem::size_of::<f32>())
            .map(|blob| cosine_similarity_blob(&query_vector, blob));
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

    let mut candidates = candidates
        .into_iter()
        .map(|entry| entry.0)
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.download_id.cmp(&b.download_id))
    });
    let mut text_statement = conn
        .prepare("SELECT metadata_text, content_preview FROM semantic_works WHERE download_id = ?1")
        .map_err(|e| format!("Semantic result text prepare failed: {e}"))?;
    candidates
        .into_iter()
        .map(|candidate| {
            let (metadata, content): (String, String) = text_statement
                .query_row(params![candidate.download_id], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .map_err(|e| format!("Semantic result text read failed: {e}"))?;
            Ok(SemanticSearchHit {
                download_id: candidate.download_id,
                chunk_id: "work".to_string(),
                field: if candidate.content {
                    "content"
                } else {
                    "metadata"
                }
                .to_string(),
                text: if candidate.content { content } else { metadata },
                score: candidate.score,
            })
        })
        .collect()
}

/// 索引に残った、もう存在しない作品の行を落とす。
///
/// 削除のたびに `clear_documents` を呼んではいるが、取りこぼしはたまる。
/// 実測で索引 3,946 件に対して実在 3,938 件 — 8 件が幽霊として残っていた。
/// 検索の結果からは弾かれるので害は小さいが、走査のたびに読む無駄が残り、
/// 「索引の件数」が棚の件数と合わない気持ち悪さも残る。
///
/// `alive` は実在する `download_id` の全体。**部分集合を渡してはいけない** —
/// 渡した分以外が全部消える。
pub fn prune_missing_documents(storage_dir: &Path, alive: &HashSet<i64>) -> Result<usize, String> {
    let conn = open_index(storage_dir)?;
    let indexed = conn
        .prepare("SELECT download_id FROM semantic_works")
        .and_then(|mut stmt| {
            stmt.query_map([], |row| row.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|e| format!("Semantic prune scan failed: {e}"))?;
    let orphans = indexed
        .into_iter()
        .filter(|id| !alive.contains(id))
        .collect::<Vec<_>>();
    if orphans.is_empty() {
        return Ok(0);
    }
    drop(conn);
    clear_documents(storage_dir, &orphans)?;
    Ok(orphans.len())
}

/// 作品ごとの本文ベクトル。全チャンクの平均を、長さ1にそろえて返す。
///
/// 検索は「問いに近いチャンク」を探すが、束ねるときに要るのは
/// **作品そのものがどのあたりにあるか**である。章の平均はその代わりになる。
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
        let vector = blob_to_vector(&blob);
        if vector.len() != VECTOR_DIMENSION {
            continue;
        }
        out.insert(download_id, vector);
    }
    Ok(out)
}

/// Finds related works from already-indexed work vectors without loading the
/// embedding model.  Collection suggestions use this path: their input is a
/// work, not a free-form query, so embedding its title again only loses
/// information and can unexpectedly start a model download.
pub fn similar_works(
    storage_dir: &Path,
    seed_ids: &[i64],
    limit: usize,
) -> Result<Vec<(i64, f64)>, String> {
    if seed_ids.is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let conn = open_index(storage_dir)?;
    let seed_set = seed_ids.iter().copied().collect::<HashSet<_>>();
    let mut seed_statement = conn
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
        if let Some((metadata, content)) = row {
            let metadata = blob_to_vector(&metadata);
            let content = content.as_deref().map(blob_to_vector);
            if metadata.len() == VECTOR_DIMENSION
                && content
                    .as_ref()
                    .is_none_or(|vector| vector.len() == VECTOR_DIMENSION)
            {
                seeds.push((metadata, content));
            }
        }
    }
    if seeds.is_empty() {
        return Ok(Vec::new());
    }

    let mut statement = conn
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
        let score = seeds
            .iter()
            .map(|(seed_metadata, seed_content)| {
                let metadata_score = cosine_similarity_blob(seed_metadata, &metadata);
                match (seed_content.as_deref(), content.as_deref()) {
                    (Some(seed), Some(candidate)) => {
                        metadata_score * 0.30 + cosine_similarity_blob(seed, candidate) * 0.70
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

/// 測るために、索引から作品と本文の断片を拾う。
///
/// `examples/semantic_query_probe.rs` が使う。正解を人手で用意せずに問いの
/// 作り方を比べるため、**作品自身を答えにする**ので、その材料がここから要る。
pub struct ProbeSample {
    pub download_id: i64,
    pub text: String,
}

pub fn probe_samples(storage_dir: &Path, limit: usize) -> Result<Vec<ProbeSample>, String> {
    let conn = open_index(storage_dir)?;
    let mut statement = conn
        .prepare(
            "SELECT download_id, content_preview
               FROM semantic_works
              WHERE model_id = ?1 AND dimension = ?2 AND content_vector IS NOT NULL
              ORDER BY download_id
              LIMIT ?3",
        )
        .map_err(|e| format!("Semantic probe prepare failed: {e}"))?;
    let rows = statement
        .query_map(
            params![MODEL_ID, VECTOR_DIMENSION as i64, limit as i64],
            |row| {
                Ok(ProbeSample {
                    download_id: row.get(0)?,
                    text: row.get(1)?,
                })
            },
        )
        .map_err(|e| format!("Semantic probe query failed: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Semantic probe read failed: {e}"))
}

pub fn status(storage_dir: &Path) -> SemanticIndexStatus {
    release_model_if_idle();
    let Ok(conn) = open_index(storage_dir) else {
        return SemanticIndexStatus {
            indexed_works: 0,
            indexed_chunks: 0,
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
        indexed_chunks: indexed_works,
        model_ready: semantic_model_ready(),
        provider: embedding_provider(),
        gpu_enabled: embedding_gpu_enabled(),
    }
}

/// IDs physically present in the current sidecar format/model.
///
/// Coverage metadata lives in the main database, so startup recovery compares
/// it with this set. A count alone cannot detect a partially rebuilt sidecar
/// whose surviving rows happen to be fewer but non-zero.
pub fn indexed_download_ids(storage_dir: &Path) -> Result<HashSet<i64>, String> {
    let conn = open_index(storage_dir)?;
    let mut statement = conn
        .prepare(
            "SELECT download_id FROM semantic_works
              WHERE model_id = ?1 AND dimension = ?2",
        )
        .map_err(|e| format!("Semantic id scan prepare failed: {e}"))?;
    let rows = statement
        .query_map(params![MODEL_ID, VECTOR_DIMENSION as i64], |row| row.get(0))
        .map_err(|e| format!("Semantic id scan failed: {e}"))?;
    rows.collect::<Result<HashSet<_>, _>>()
        .map_err(|e| format!("Semantic id scan read failed: {e}"))
}

/// Cheap common-case coverage check used during startup. The full ID scan is
/// reserved for a count mismatch, which avoids allocating every work ID on
/// every launch of a healthy large library.
pub fn indexed_download_count(storage_dir: &Path) -> Result<i64, String> {
    let conn = open_index(storage_dir)?;
    conn.query_row(
        "SELECT COUNT(*) FROM semantic_works WHERE model_id = ?1 AND dimension = ?2",
        params![MODEL_ID, VECTOR_DIMENSION as i64],
        |row| row.get(0),
    )
    .map_err(|e| format!("Semantic work count failed: {e}"))
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
/// （[`build_chunks`] を見ること）。だから問いも `query: {原文}` でよい。
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
/// 測り直すときは `cargo run --release --example semantic_query_probe`。
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
    let conn = Connection::open(&path).map_err(|e| format!("Semantic index open failed: {}", e))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         CREATE TABLE IF NOT EXISTS semantic_works (
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
         CREATE INDEX IF NOT EXISTS idx_semantic_works_model
            ON semantic_works(model_id, dimension);",
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
    start_model_reaper();
    let model = MODEL.get_or_init(|| Mutex::new(EmbeddingRuntimeSlot::default()));
    let mut slot = model
        .lock()
        .map_err(|e| format!("Semantic model lock failed: {}", e))?;
    if slot.runtime.is_none() {
        slot.runtime =
            Some(init_embedding_runtime().map_err(|e| format!("Semantic model init failed: {e}"))?);
    }
    let result = slot
        .runtime
        .as_mut()
        .expect("semantic runtime was initialized above")
        .model
        .embed(texts, None)
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

/// Releases the ONNX Runtime session and its DirectML allocations.
///
/// The mutex is held throughout embedding, so `take` cannot race an active
/// inference. Drop outside the lock because ORT may synchronously tear down
/// provider resources and that work must not block status readers.
#[cfg(not(test))]
pub fn release_model() -> bool {
    let Some(model) = MODEL.get() else {
        return false;
    };
    let removed = {
        let Ok(mut slot) = model.lock() else {
            return false;
        };
        slot.last_used = None;
        slot.runtime.take()
    };
    let released = removed.is_some();
    drop(removed);
    if released {
        log::info!("意味検索モデルをメモリから解放しました");
    }
    released
}

#[cfg(test)]
pub fn release_model() -> bool {
    false
}

#[cfg(not(test))]
fn release_model_if_idle() {
    let Some(model) = MODEL.get() else {
        return;
    };
    let removed = {
        let Ok(mut slot) = model.lock() else {
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
    MODEL
        .get()
        .and_then(|slot| slot.lock().ok().map(|slot| slot.runtime.is_some()))
        .unwrap_or(false)
}

#[cfg(test)]
fn semantic_model_ready() -> bool {
    true
}

#[cfg(not(test))]
fn embedding_provider() -> String {
    MODEL
        .get()
        .and_then(|slot| slot.lock().ok())
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
    MODEL
        .get()
        .and_then(|slot| slot.lock().ok())
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

fn blob_to_vector(blob: &[u8]) -> Vec<f32> {
    // 4バイトずつ。長さが型で決まるので、添字で取り出す必要がなくなる。
    blob.as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect()
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

#[cfg(test)]
fn cosine_similarity(left: &[f32], right: &[f32]) -> f64 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    left.iter()
        .zip(right.iter())
        .map(|(left, right)| *left as f64 * *right as f64)
        .sum::<f64>()
        .clamp(0.0, 1.0)
}

fn cosine_similarity_blob(left: &[f32], right: &[u8]) -> f64 {
    if right.len() != std::mem::size_of_val(left) || left.is_empty() {
        return 0.0;
    }
    left.iter()
        .zip(right.as_chunks::<4>().0.iter())
        .map(|(left, bytes)| *left as f64 * f32::from_le_bytes(*bytes) as f64)
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
    use std::fs;

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
        let model = model_cache_dir_for(storage);
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
    /// `examples/semantic_query_probe` で測り直してからにすること。
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

    #[test]
    fn semantic_sidecar_returns_one_hit_per_work() {
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
        assert_eq!(hits.iter().filter(|hit| hit.download_id == 1).count(), 1);
        assert_eq!(status(&storage).indexed_works, 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_body_only_work_is_indexed_and_reported_as_inserted() {
        let root =
            std::env::temp_dir().join(format!("piep_semantic_body_{}", rand::random::<u32>()));
        let storage = root.join("downloads");
        fs::create_dir_all(&storage).unwrap();
        let inserted = upsert_documents(
            &storage,
            &[SemanticIndexDocument {
                download_id: 2,
                title: String::new(),
                author_name: String::new(),
                tags: String::new(),
                series_title: String::new(),
                excerpt: String::new(),
                body: "題名が無くても、この本文は意味検索の対象になる。".to_string(),
            }],
        )
        .unwrap();
        assert_eq!(inserted, vec![2]);
        assert!(search(&storage, "意味検索", 10)
            .unwrap()
            .iter()
            .any(|hit| hit.download_id == 2));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn related_work_scan_keeps_only_the_requested_best_results() {
        let root =
            std::env::temp_dir().join(format!("piep_semantic_related_{}", rand::random::<u32>()));
        let storage = root.join("downloads");
        fs::create_dir_all(&storage).unwrap();
        let document = |download_id, title: &str, body: &str| SemanticIndexDocument {
            download_id,
            title: title.to_string(),
            author_name: "作者".to_string(),
            tags: "小説".to_string(),
            series_title: String::new(),
            excerpt: String::new(),
            body: body.to_string(),
        };
        upsert_documents(
            &storage,
            &[
                document(1, "雨の図書室", "雨音を聞きながら本を読む。"),
                document(2, "雨の図書室", "雨音を聞きながら本を読む。"),
                document(3, "宇宙船", "遠い惑星へ向かう。"),
            ],
        )
        .unwrap();

        let related = similar_works(&storage, &[1], 1).unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].0, 2);
        assert!(related[0].1 > 0.9);
        let _ = fs::remove_dir_all(root);
    }
}
