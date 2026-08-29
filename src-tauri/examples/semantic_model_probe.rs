//! いまのモデルと乗り換え候補を、**同じ棚・同じ問い**で比べる。
//!
//!     cargo run --release --example semantic_model_probe -- \
//!         <piep.db の写し> <storage_dir> <モデル置き場>...
//!
//! 置き場は何本でも渡せる。名前は**ディレクトリの名前**をそのまま使う。
//! 書き込みはしないが、**写しを渡すこと**。
//!
//! # 何をしているか
//!
//! - いまのモデル（`intfloat/multilingual-e5-small` / 384次元）の文書ベクトルは、
//!   **索引にもう入っているものをそのまま読む**。アプリが実際に作ったものなので、
//!   ここで作り直すと「アプリの索引」ではなく「私が作った索引」を測ることになる。
//! - 候補（`cl-nagoya/ruri-v3-30m` / 256次元）は、**同じ断片の文字列**を読んで
//!   その場で埋め込む。断片の切り方を揃えないと、モデルの差と切り方の差が
//!   混ざる。DirectML で 163,393 断片が約90秒（`ruri_directml_probe` で実測）。
//! - 探すのは **ANN を通さない総当たり**。HNSW の近似の差を混ぜないため。
//!
//! 正解は `semantic_query_probe` と同じく**作品自身**に取らせる。題名を問いに
//! したとき、本文の一文を問いにしたときの、その作品の順位を見る。
//!
//! # 前置きが違う
//!
//! e5 は `query:` / `passage:`、ruri-v3 は `検索クエリ: ` / `検索文書: `。
//! **どちらもそのモデルが学習したときの形で渡す。** 揃えると片方が不利になる。
//!
//! # 測った結果（163,393 断片・150 作品・2026-08-29）
//!
//! ```text
//! ■ 題名を問いにしたとき            1位     5位内   10位内   MRR
//!   multilingual-e5-small (384)    86.0%   98.7%   98.7%   0.919
//!   ruri-v3-30m (256)              76.7%   91.3%   92.7%   0.834
//!
//! ■ 本文の一文を問いにしたとき
//!   multilingual-e5-small (384)    76.7%   86.7%   90.0%   0.814
//!   ruri-v3-30m (256)              56.7%   80.0%   81.3%   0.657
//! ```
//!
//! **公表値と逆に出た。** JMTEB の検索課題では ruri-v3-30m が +10.8 とされて
//! いて、乗り換える前提で調べ始めた。この棚では現行モデルが両方で勝っている。
//!
//! 一般の検索課題と「自分の棚から、覚えている一文で作品を引く」は別物だという
//! ことで、**順位表では決められない**という結論そのものが、この道具の成果である。
//! 次の候補（ruri-v3-130m など）を見るときも、ここへ足して測ってから決めること。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use fastembed::{
    EmbeddingModel, InitOptions, InitOptionsUserDefined, Pooling, TextEmbedding, TokenizerFiles,
    UserDefinedEmbeddingModel,
};
use rusqlite::Connection;

use piep_lib::database::Database;

/// 一度に埋め込む本数。進み具合を出したいので、全部を1回で渡さない。
const EMBED_BATCH: usize = 512;
/// 何作品を問いにするか。
const SAMPLE_WORKS: usize = 150;
/// 何位まで見るか。
const RANK_LIMIT: usize = 20;

struct Candidate {
    label: String,
    query_prefix: &'static str,
    embedder: TextEmbedding,
    /// 断片ごとの文書ベクトル。`chunks` と同じ並び。
    vectors: Vec<Vec<f32>>,
}

#[derive(Default)]
struct Tally {
    asked: usize,
    hit_at_1: usize,
    hit_at_5: usize,
    hit_at_10: usize,
    reciprocal: f64,
}

impl Tally {
    fn record(&mut self, rank: Option<usize>) {
        self.asked += 1;
        let Some(rank) = rank else { return };
        if rank == 1 {
            self.hit_at_1 += 1;
        }
        if rank <= 5 {
            self.hit_at_5 += 1;
        }
        if rank <= 10 {
            self.hit_at_10 += 1;
        }
        self.reciprocal += 1.0 / rank as f64;
    }

    fn report(&self, label: &str) {
        if self.asked == 0 {
            println!("  {label}: 測れなかった");
            return;
        }
        let pct = |n: usize| n as f64 * 100.0 / self.asked as f64;
        println!(
            "  {label:<24}  1位 {:5.1}%   5位内 {:5.1}%   10位内 {:5.1}%   MRR {:.3}",
            pct(self.hit_at_1),
            pct(self.hit_at_5),
            pct(self.hit_at_10),
            self.reciprocal / self.asked as f64,
        );
    }
}

fn read_file(dir: &Path, name: &str) -> Result<Vec<u8>, String> {
    std::fs::read(dir.join(name)).map_err(|e| format!("{name} を読めません: {e}"))
}

/// 保存されているベクトルは 4バイトずつのリトルエンディアン（`vector_to_blob`）。
fn blob_to_vector(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

fn normalized(vector: &[f32]) -> Vec<f32> {
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm == 0.0 {
        return vector.to_vec();
    }
    vector.iter().map(|v| v / norm).collect()
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// 意味索引の置き場。`semantic_index::index_path` と同じ導き方をしている。
/// **片方だけ変えると、測る相手を間違える。**
fn index_path(storage_dir: &Path) -> PathBuf {
    storage_dir
        .parent()
        .unwrap_or(storage_dir)
        .join("search")
        .join("semantic-v1")
        .join("index.sqlite")
}

/// いまアプリが使っているのと同じ組み立て（`init_embedding_runtime` を写したもの）。
fn build_current_model() -> Result<TextEmbedding, String> {
    TextEmbedding::try_new(
        InitOptions::new(EmbeddingModel::MultilingualE5Small)
            .with_show_download_progress(true)
            .with_execution_providers(vec![ort::ep::DirectML::default().build()]),
    )
    .map_err(|error| format!("いまのモデルを組めません: {error}"))
}

fn build_ruri(dir: &Path) -> Result<TextEmbedding, String> {
    let model = UserDefinedEmbeddingModel::new(
        read_file(dir, "model.onnx")?,
        TokenizerFiles {
            tokenizer_file: read_file(dir, "tokenizer.json")?,
            config_file: read_file(dir, "config.json")?,
            special_tokens_map_file: read_file(dir, "special_tokens_map.json")?,
            tokenizer_config_file: read_file(dir, "tokenizer_config.json")?,
        },
    )
    // `1_Pooling/config.json` が `pooling_mode_mean_tokens: true`。
    .with_pooling(Pooling::Mean);
    TextEmbedding::try_new_from_user_defined(
        model,
        InitOptionsUserDefined::new()
            .with_max_length(512)
            .with_execution_providers(vec![ort::ep::DirectML::default().build()]),
    )
    .map_err(|error| format!("ruri を組めません: {error}"))
}

/// 断片を順に埋め込む。進み具合を出しながら、正規化して返す。
fn embed_all(
    model: &mut TextEmbedding,
    prefix: &str,
    texts: &[String],
    label: &str,
) -> Result<Vec<Vec<f32>>, String> {
    let started = Instant::now();
    let mut out = Vec::with_capacity(texts.len());
    for (index, batch) in texts.chunks(EMBED_BATCH).enumerate() {
        let inputs = batch
            .iter()
            .map(|text| format!("{prefix}{text}"))
            .collect::<Vec<_>>();
        let vectors = model
            .embed(inputs, None)
            .map_err(|error| format!("{label} の埋め込みに失敗: {error}"))?;
        if vectors.len() != batch.len() {
            return Err(format!(
                "{label}: {} 本渡して {} 本しか返らない",
                batch.len(),
                vectors.len()
            ));
        }
        out.extend(vectors.iter().map(|v| normalized(v)));
        if index % 40 == 0 {
            println!(
                "  {label}: {}/{}（{:.0}秒）",
                out.len(),
                texts.len(),
                started.elapsed().as_secs_f64()
            );
        }
    }
    println!(
        "  {label}: {} 本を {:.0}秒で",
        out.len(),
        started.elapsed().as_secs_f64()
    );
    Ok(out)
}

/// 総当たりで探して、その作品が何位に来たかを返す。断片ではなく**作品**で数える。
fn rank_of(
    candidate: &mut Candidate,
    owners: &[i64],
    query: &str,
    want: i64,
) -> Result<Option<usize>, String> {
    let query_vector = candidate
        .embedder
        .embed(vec![format!("{}{}", candidate.query_prefix, query)], None)
        .map_err(|error| format!("問いの埋め込みに失敗: {error}"))?
        .into_iter()
        .next()
        .ok_or("問いの埋め込みが返らない")?;
    let query_vector = normalized(&query_vector);

    let mut scored: Vec<(f32, i64)> = candidate
        .vectors
        .iter()
        .zip(owners)
        .map(|(vector, owner)| (dot(&query_vector, vector), *owner))
        .collect();
    // 上位だけ要る。全体を並べ替える必要はない。
    let take = scored.len().min(RANK_LIMIT * 64);
    scored.select_nth_unstable_by(take - 1, |a, b| b.0.total_cmp(&a.0));
    scored.truncate(take);
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));

    let mut seen = HashSet::new();
    let mut rank = 0usize;
    for (_, owner) in scored {
        if !seen.insert(owner) {
            continue;
        }
        rank += 1;
        if owner == want {
            return Ok(Some(rank));
        }
        if rank >= RANK_LIMIT {
            break;
        }
    }
    Ok(None)
}

/// 本文の真ん中あたりから一文を取る。`semantic_query_probe` と同じ取り方。
fn middle_sentence(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 200 {
        return None;
    }
    let start = chars.len() / 3;
    let slice: String = chars[start..(start + 90).min(chars.len())].iter().collect();
    let trimmed = slice.trim();
    (trimmed.chars().count() >= 30).then(|| trimmed.to_string())
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        return Err(
            "usage: semantic_model_probe <piep.db の写し> <storage_dir> <モデル置き場>...".into(),
        );
    }
    let storage = Path::new(&args[2]);
    let model_dirs: Vec<&Path> = args[3..].iter().map(Path::new).collect();
    let db = Database::open(Path::new(&args[1]), storage)?;

    // 索引に入っている断片を、そのまま読む。**作り直さない。**
    let index = index_path(storage);
    println!("索引: {}", index.display());
    let conn = Connection::open_with_flags(
        &index,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("索引を開けません: {error}"))?;
    let mut statement = conn
        .prepare("SELECT download_id, field, text, vector FROM semantic_chunks ORDER BY rowid")
        .map_err(|error| format!("断片を読めません: {error}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
            ))
        })
        .map_err(|error| format!("断片を読めません: {error}"))?;

    let mut owners: Vec<i64> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    let mut current_vectors: Vec<Vec<f32>> = Vec::new();
    let mut body_samples: Vec<(i64, String)> = Vec::new();
    let mut sampled_works: HashSet<i64> = HashSet::new();
    for row in rows {
        let (download_id, field, text, blob) = row.map_err(|error| error.to_string())?;
        // **一作品につき一本。** 断片は作品ごとにまとまって並ぶので、先頭から
        // 順に取ると同じ作品ばかりになる（最初はそれで1件しか測れていなかった）。
        if field == "body" && text.chars().count() >= 200 && sampled_works.insert(download_id) {
            body_samples.push((download_id, text.clone()));
        }
        owners.push(download_id);
        texts.push(text);
        current_vectors.push(normalized(&blob_to_vector(&blob)));
    }
    println!("断片: {} 本 / 作品: {} 件", texts.len(), {
        owners.iter().collect::<HashSet<_>>().len()
    });
    if texts.is_empty() {
        return Err("索引が空である".into());
    }

    println!("\n■ 文書側を用意する");
    println!("  いまのモデル: 索引に入っているものをそのまま使う");
    let mut candidates = vec![Candidate {
        label: "multilingual-e5-small (384)".to_string(),
        query_prefix: "query: ",
        embedder: build_current_model()?,
        vectors: current_vectors,
    }];

    for dir in &model_dirs {
        let name = dir
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("model")
            .to_string();
        let mut model = build_ruri(dir)?;
        let vectors = embed_all(&mut model, "検索文書: ", &texts, &name)?;
        let dimension = vectors.first().map(Vec::len).unwrap_or(0);
        candidates.push(Candidate {
            // 何本渡されるか分からないので、名前は借り物ではなく持ち物にする。
            label: format!("{name} ({dimension})"),
            query_prefix: "検索クエリ: ",
            embedder: model,
            vectors,
        });
    }

    // 問いにする作品を選ぶ。一作品につき一本。
    let mut chosen = HashSet::new();
    let mut samples: Vec<(i64, String, String)> = Vec::new();
    for (download_id, text) in body_samples {
        if samples.len() >= SAMPLE_WORKS {
            break;
        }
        if !chosen.insert(download_id) {
            continue;
        }
        let Ok(entry) = db.get_download(download_id) else {
            continue;
        };
        if entry.title.trim().is_empty() {
            continue;
        }
        samples.push((download_id, entry.title, text));
    }
    println!("\n測る作品: {} 件", samples.len());

    let mut by_title: Vec<Tally> = candidates.iter().map(|_| Tally::default()).collect();
    let mut by_passage: Vec<Tally> = candidates.iter().map(|_| Tally::default()).collect();
    for (index, (download_id, title, text)) in samples.iter().enumerate() {
        if index % 25 == 0 {
            println!("  ... {index}/{}", samples.len());
        }
        let sentence = middle_sentence(text);
        for (slot, candidate) in candidates.iter_mut().enumerate() {
            by_title[slot].record(rank_of(candidate, &owners, title, *download_id)?);
            if let Some(sentence) = &sentence {
                by_passage[slot].record(rank_of(candidate, &owners, sentence, *download_id)?);
            }
        }
    }

    println!("\n■ 題名を問いにしたとき");
    for (slot, candidate) in candidates.iter().enumerate() {
        by_title[slot].report(&candidate.label);
    }
    println!("\n■ 本文の一文を問いにしたとき");
    for (slot, candidate) in candidates.iter().enumerate() {
        by_passage[slot].report(&candidate.label);
    }
    println!("\n総当たりで比べている。ANN の近似は入っていない。");
    Ok(())
}
