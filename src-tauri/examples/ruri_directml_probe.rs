//! ruri-v3-30m が、この機械の DirectML で本当に動くのかを1点だけ確かめる。
//!
//!     cargo run --release --example ruri_directml_probe -- <モデルを置いた場所>
//!
//! 置き場には次の5つを入れておく（どれも公開されているもの）:
//!
//! - `model.onnx`              `onnx-community/ruri-v3-30m-ONNX` の `onnx/model.onnx`
//! - `tokenizer.json`          `cl-nagoya/ruri-v3-30m`
//! - `tokenizer_config.json`   同上
//! - `special_tokens_map.json` 同上
//! - `config.json`             同上
//!
//! # なぜ確かめるのか
//!
//! いまの意味検索は `multilingual-e5-small`（384次元・465MB）で、乗り換え候補の
//! ruri-v3-30m は 256次元・147MB と**小さくて、公表値では強い**。ただし中身は
//! ModernBERT で、いまの `ort 2.0.0-rc.12` の DirectML で動く保証がない。
//!
//! ここが折れると、163,393 断片の作り直しが CPU で走ることになって現実的で
//! なくなる。**乗り換えの可否は、速さや点数より先にここで決まる。**
//!
//! 測るのは三つだけ:
//!
//! 1. DirectML で初期化できるか（できなければ CPU に落ちる）
//! 2. 返るベクトルの次元が 256 か
//! 3. 「問い」と「合っている文書」が、「合っていない文書」より近く出るか
//!
//! 3 は当たり前のことしか見ていない。**当たり前が壊れていないことの確認**で、
//! 良し悪しの比較ではない。比較は `semantic_query_probe` の仕事。

use std::path::Path;
use std::time::Instant;

use fastembed::{
    InitOptionsUserDefined, Pooling, TextEmbedding, TokenizerFiles, UserDefinedEmbeddingModel,
};

/// ruri-v3 の前置き。e5 の `query:` / `passage:` とは別の語を使う。
const QUERY_PREFIX: &str = "検索クエリ: ";
const DOCUMENT_PREFIX: &str = "検索文書: ";
/// 次元はモデルが決める。**こちらが決め打ちにしない** - 30m は 256、130m は 512。
/// 索引の寸法が変わるという事実だけを、はっきり出す。
const CURRENT_DIMENSION: usize = 384;

fn read(dir: &Path, name: &str) -> Result<Vec<u8>, String> {
    std::fs::read(dir.join(name)).map_err(|e| format!("{name} を読めません: {e}"))
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

/// 与えた実行提供者で組み立てを試す。落ちた理由は握りつぶさずに返す。
fn build(dir: &Path, use_directml: bool) -> Result<(TextEmbedding, f64), String> {
    let model = UserDefinedEmbeddingModel::new(
        read(dir, "model.onnx")?,
        TokenizerFiles {
            tokenizer_file: read(dir, "tokenizer.json")?,
            config_file: read(dir, "config.json")?,
            special_tokens_map_file: read(dir, "special_tokens_map.json")?,
            tokenizer_config_file: read(dir, "tokenizer_config.json")?,
        },
    )
    // 1_Pooling/config.json は `pooling_mode_mean_tokens: true`。
    // ここを間違えると、動いてはいるのに当たらない索引ができる。
    .with_pooling(Pooling::Mean);

    let mut options = InitOptionsUserDefined::new().with_max_length(512);
    if use_directml {
        options = options.with_execution_providers(vec![ort::ep::DirectML::default().build()]);
    } else {
        options = options.with_intra_threads(4);
    }

    let started = Instant::now();
    let embedding = TextEmbedding::try_new_from_user_defined(model, options)
        .map_err(|error| error.to_string())?;
    Ok((embedding, started.elapsed().as_secs_f64()))
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        return Err("usage: ruri_directml_probe <モデルを置いた場所>".into());
    }
    let dir = Path::new(&args[1]);
    println!("置き場: {}", dir.display());

    // まず DirectML。落ちたら理由を出して CPU で続ける - 「動かない」で
    // 終わらせず、**どこまでは動くのか**まで持って帰る。
    let (mut model, provider, init_secs) = match build(dir, true) {
        Ok((model, secs)) => {
            println!("DirectML: 組み上がった（{secs:.1}秒）");
            (model, "directml", secs)
        }
        Err(error) => {
            println!("DirectML: 組み上がらない -> {error}");
            let (model, secs) = build(dir, false)?;
            println!("CPU: 組み上がった（{secs:.1}秒）");
            (model, "cpu", secs)
        }
    };

    let passages = [
        "雨上がりの図書室で、彼女は借りたままの本を返しそびれていた。",
        "港の倉庫は錆びた鉄の匂いがして、機械の唸りが夜通し止まらない。",
        "祖母の作る煮物はいつも少し甘くて、台所には湯気が立ちこめていた。",
    ];
    let query = "図書館で本を返せずにいる女の子の話";

    let mut inputs: Vec<String> = passages
        .iter()
        .map(|text| format!("{DOCUMENT_PREFIX}{text}"))
        .collect();
    inputs.push(format!("{QUERY_PREFIX}{query}"));

    let started = Instant::now();
    let vectors = model
        .embed(inputs, None)
        .map_err(|error| format!("埋め込みに失敗: {error}"))?;
    let embed_secs = started.elapsed().as_secs_f64();

    let dimension = vectors.first().map(Vec::len).unwrap_or(0);
    println!("次元: {dimension}（いまの索引は {CURRENT_DIMENSION}）");
    println!("4本の埋め込みに {embed_secs:.2}秒");
    if dimension == 0 {
        return Err("ベクトルが空である".into());
    }
    if dimension != CURRENT_DIMENSION {
        println!(
            "  索引の寸法が {CURRENT_DIMENSION} から {dimension} へ変わる。乗り換えるなら全作り直し。"
        );
    }

    let query_vector = vectors.last().ok_or("問いの埋め込みが無い")?;
    println!("\n問い: {query}");
    let mut scored: Vec<(f32, &str)> = passages
        .iter()
        .enumerate()
        .map(|(index, text)| (cosine(query_vector, &vectors[index]), *text))
        .collect();
    for (score, text) in &scored {
        println!("  {score:.4}  {text}");
    }
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    let top = scored.first().ok_or("比べる相手が無い")?;
    if top.1 != passages[0] {
        return Err(format!(
            "合っているはずの文書が1位に来ない（1位: {}）。前置きかプーリングが違う",
            top.1
        ));
    }

    println!("\n判定: {provider} で動く。初期化 {init_secs:.1}秒、次元 {dimension}。");
    println!("順位も期待どおりなので、前置きとプーリングの読みは合っている。");

    // **組み上がることと、実用の速さで回ることは別。** DirectML の提供者を
    // 載せても、対応していない演算はセッションの中で黙って CPU へ落ちる。
    // 乗り換えの可否は結局「棚を作り直すのに何時間かかるか」で決まるので、
    // 実際の断片に近い長さで通して測る。
    println!("\n■ 作り直しにかかる時間の目安");
    let sample = build_batch(BATCH_SIZE);
    for (label, use_directml) in [("DirectML", true), ("CPU", false)] {
        let Ok((mut model, _)) = build(dir, use_directml) else {
            println!("  {label:<9} 組み上がらないので測れない");
            continue;
        };
        // 最初の1回は暖機。ここを混ぜると倍以上ずれる。
        let _ = model.embed(build_batch(8), None);
        let started = Instant::now();
        match model.embed(sample.clone(), None) {
            Ok(vectors) => {
                let secs = started.elapsed().as_secs_f64();
                let per_sec = vectors.len() as f64 / secs;
                let minutes = SHELF_CHUNKS as f64 / per_sec / 60.0;
                println!(
                    "  {label:<9} {per_sec:>7.1} 断片/秒  ->  {SHELF_CHUNKS} 断片で約 {minutes:.1} 分"
                );
            }
            Err(error) => println!("  {label:<9} 途中で落ちた: {error}"),
        }
    }
    println!("\nいまの棚の断片数を前提にした目安。実際は保存と同時に少しずつ入る。");
    Ok(())
}

/// いま索引に入っている断片の数（`semantic_query_probe` が数えたもの）。
const SHELF_CHUNKS: usize = 163_393;
/// 一度に通す本数。多くしすぎると暖機と区別が付かなくなる。
const BATCH_SIZE: usize = 128;

/// 本文の断片に近い長さの入力を作る。
///
/// **短い文で測ると、実際より何倍も速い数字が出る。** 最初はここが約160字
/// しかなく、それで出した「163,393 断片で 4.7 分」は本番の断片（中央値 594字、
/// 99% が 619字）にはまるで足りなかった。注意は長さの二乗で効くので、4倍近い
/// 差はそのまま何倍もの時間になる。棚の実測に合わせてある。
fn build_batch(count: usize) -> Vec<String> {
    let paragraph = concat!(
        "雨上がりの図書室は、窓の外の光をまだ含んだままの空気で満ちていた。",
        "彼女は借りたままの本を鞄から出して、返却台の前でしばらく立っていた。",
        "背表紙の折れ目は自分が付けたものではないと分かっていたが、",
        "それでも指でなぞると、誰かがここで同じ場所を読んだのだと思えた。",
        "司書は何も言わずに日付の印を押し、次の人へ向き直った。",
        "外へ出ると、濡れた地面が夕方の色を映していて、",
        "帰り道の途中で一度だけ振り返った。",
        "傘は結局ひらかないまま鞄の横に差してあって、濡れた布の匂いだけが残った。",
        "誰かに読まれることを前提にしていない字が、余白に小さく並んでいる。",
        "日付も名前も無いので、いつ書かれたものなのかは分からない。",
        "貸出票の裏には、返却の期限だけがはっきりと印字されていた。",
        "階段を降りるとき、手すりの冷たさで指の先が少しだけ痛んだ。",
        "駅までの道は覚えているのに、いつもどこかで一度立ち止まってしまう。",
        "その日のことを、あとになって何度も思い出すことになるとは思わなかった。",
        "本のあいだに挟まっていた栞は、もう色が褪せて文字が読めなくなっていた。",
        "受付の時計は二分ほど進んでいて、誰もそれを直そうとしないまま何年も経つ。",
        "窓際の席には日が差していて、そこだけ紙の色がわずかに褪せて見えた。",
        "帰ってから、鞄の底に入れたままだったことに気づいて、少しだけ笑った。",
    );
    (0..count)
        .map(|index| format!("{DOCUMENT_PREFIX}{paragraph}（{index}）"))
        .collect()
}
