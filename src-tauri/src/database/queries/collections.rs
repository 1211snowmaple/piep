//! 作品コレクションとその候補生成。
//!
//! 取得や検索とは独立した、利用者が自分で組み立てる集合を扱う。`queries` の
//! 子モジュールに置いてあるのは、`Database` の接続やプールといった非公開の
//! 内部へ触れる必要があるためで、親の実装詳細をここだけに開いている。

use super::*;

use crate::database::collection_rules;

/// 公式シリーズの話数がこれだけ離れていたら、隣り合っているとは言わない。
///
/// 抜けが1つある連載を切らない程度に広く、151作のシリーズの端どうしを
/// つながないくらいには狭く。
const SERIES_ADJACENT_GAP: i64 = 2;

/// 投稿がこれだけ近ければ、同じ時期の作品として文脈に数える。
const PUBLISH_NEARBY_DAYS: i64 = 60;

/// 束らしい入力になる作品を拾う（命名エンジンの試し書き用）。
pub(crate) fn sample_bundle_like_ids_in(conn: &Connection) -> Result<Vec<i64>, String> {
    // 締まった束になるタグを起点にする。「♡喘ぎ」のように広いタグで拾うと
    // 材料がばらけ、返ってきた名前が良いのか悪いのか判断できない。
    // 3〜8作のタグなら、人物名や作品名であることが多い。
    let mut stmt = conn
        .prepare(
            "SELECT t.name FROM download_tags dt
             JOIN tags t ON t.id = dt.tag_id
             GROUP BY t.id
             HAVING COUNT(*) BETWEEN 3 AND 8
             ORDER BY COUNT(*) DESC, t.name COLLATE NOCASE ASC
             LIMIT 40",
        )
        .map_err(|e| format!("Failed to prepare sample tags: {e}"))?;
    let anchor = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("Failed to query sample tags: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read sample tags: {e}"))?
        .into_iter()
        .find(|tag| collection_rules::is_informative_tag(tag));
    drop(stmt);

    // タグが一つも無い棚でも試せるように、最近保存した作品で代用する。
    let (sql, tag) = match anchor {
        Some(tag) => (
            "SELECT dt.download_id FROM download_tags dt
             JOIN tags t ON t.id = dt.tag_id
             WHERE t.name = ?1
             ORDER BY dt.download_id
             LIMIT 6",
            tag,
        ),
        // 束ねる手がかりが無いので、束ではなく最近の作品で試す。
        // 引数の数をそろえるために、当たらない値を1つ渡す。
        None => (
            "SELECT id FROM downloads
             WHERE ?1 IS NOT NULL
             ORDER BY downloaded_at DESC
             LIMIT 4",
            String::new(),
        ),
    };
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("Failed to prepare sample works: {e}"))?;
    let ids = stmt
        .query_map(params![tag], |row| row.get::<_, i64>(0))
        .map_err(|e| format!("Failed to query sample works: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read sample works: {e}"))?;
    Ok(ids)
}

/// 束の名前と説明の案（今の名前も案として残す）。保存はしない。
pub(crate) fn name_proposals_in(
    conn: &Connection,
    ids: &[i64],
    current_name: &str,
    current_source: &str,
) -> Result<Vec<CollectionNameCandidate>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let works = load_suggestion_works(conn, ids)?;
    let ranked = works
        .into_iter()
        .map(|work| RankedSuggestionMember {
            episode_order: collection_rules::episode_order(&work.title),
            work,
            member_score: 1.0,
            title_stem: String::new(),
            default_selected: true,
            evidence: Vec::new(),
            series_order: None,
            link_order: None,
            link_depth: None,
        })
        .collect::<Vec<_>>();
    let ids_json = serde_json::to_string(&ids)
        .map_err(|e| format!("Failed to encode collection members: {e}"))?;
    let mut options = collection_name_options(conn, &ranked, &ids_json)?;
    // いまの名前も案として残す。付け直すつもりが無かったときに、
    // 戻る先が画面に無いのは不親切である。
    if !options.iter().any(|option| option.name == current_name) {
        options.insert(
            0,
            CollectionNameCandidate {
                source: current_source.to_string(),
                label: "いまの名前".to_string(),
                name: current_name.to_string(),
                ..Default::default()
            },
        );
    }
    Ok(options)
}

/// 種から作った、保存する前の候補。
pub(crate) struct SeedPlan {
    pub seed_ids: Vec<i64>,
    pub proposed_name: String,
    pub name_options: Vec<CollectionNameCandidate>,
    pub collection_kind: String,
    pub score: f64,
    pub evidence_summary: String,
    /// `download_id` は出どころの作品の番号（棚では作業用の表の番号）。
    pub members: Vec<CollectionSuggestionMember>,
}

/// 種の作品から、束の候補を作る。保存はしない。
pub(crate) fn seed_suggestion_plan(
    src: &dyn SuggestionSource,
    seed_download_ids: &[i64],
    limit: Option<i64>,
) -> Result<SeedPlan, String> {
    let mut seed_ids = seed_download_ids
        .iter()
        .copied()
        .filter(|id| *id > 0)
        .collect::<Vec<_>>();
    seed_ids.sort_unstable();
    seed_ids.dedup();
    if seed_ids.is_empty() {
        return Err("At least one seed work is required".to_string());
    }
    if seed_ids.len() > 20 {
        return Err("A suggestion can use at most 20 seed works".to_string());
    }
    let seed_json = serde_json::to_string(&seed_ids)
        .map_err(|e| format!("Failed to encode suggestion seeds: {e}"))?;
    let (seed_works, mut candidate_ids) = {
        let conn = src.corpus()?;
        let seeds = load_suggestion_works(&conn, &seed_ids)?;
        if seeds.len() != seed_ids.len() {
            return Err("One or more seed works no longer exist".to_string());
        }
        // Do not cap this query before scoring: a busy author's arbitrary
        // row order can otherwise hide a strong title or series match.
        // rusqlite streams rows into the deduplicating ID set, keeping the
        // query result itself bounded to one row at a time.
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT candidate.id
                 FROM downloads candidate
                 JOIN downloads seed ON seed.id IN (SELECT value FROM json_each(?1))
                 WHERE candidate.id IN (SELECT value FROM json_each(?1))
                    OR (
                        candidate.author_id != '' AND seed.author_id != ''
                        AND candidate.source = seed.source
                        AND candidate.author_id = seed.author_id
                    )
                    OR (
                        LENGTH(TRIM(candidate.author_name)) >= 2
                        AND candidate.author_name = seed.author_name COLLATE NOCASE
                    )
                    OR EXISTS (
                        SELECT 1 FROM download_series candidate_series
                        JOIN download_series seed_series
                          ON seed_series.series_source = candidate_series.series_source
                         AND seed_series.series_key = candidate_series.series_key
                        WHERE candidate_series.download_id = candidate.id
                          AND seed_series.download_id = seed.id
                    )
                    OR EXISTS (
                        SELECT 1 FROM work_links link
                        WHERE link.status != 'rejected'
                          AND (
                            (link.from_download_id = seed.id AND link.to_download_id = candidate.id)
                            OR (link.to_download_id = seed.id AND link.from_download_id = candidate.id)
                          )
                    )",
            )
            .map_err(|e| format!("Failed to prepare suggestion candidates: {e}"))?;
        let ids = stmt
            .query_map(params![seed_json], |row| row.get::<_, i64>(0))
            .map_err(|e| format!("Failed to query suggestion candidates: {e}"))?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(|e| format!("Failed to read suggestion candidates: {e}"))?;
        (seeds, ids)
    };

    let link_paths = discover_linked_collection_component(src, &seed_works)?;
    candidate_ids.extend(link_paths.keys().copied());

    // 作品から作品を引く経路では、保存済みの作品ベクトル同士を比べる。
    // 自由文の埋め込みは行わないので、提案作成がモデル取得を突然始める
    // ことも、本文断片の多い長編だけが有利になることもない。
    let mut semantic_scores: HashMap<i64, f64> = HashMap::new();
    if src.semantic_ready() {
        match src.similar_works(&seed_ids, 160) {
            Ok(hits) => {
                for (download_id, score) in hits {
                    candidate_ids.insert(download_id);
                    semantic_scores.insert(download_id, score);
                }
            }
            Err(error) => {
                log::warn!("Semantic collection suggestion failed: {error}");
            }
        }
    }
    candidate_ids.extend(seed_ids.iter().copied());
    let candidate_ids = candidate_ids.into_iter().collect::<Vec<_>>();
    let conn = src.corpus()?;
    let candidates = load_suggestion_works(&conn, &candidate_ids)?;
    let seed_id_set = seed_ids.iter().copied().collect::<HashSet<_>>();
    let limit = limit.unwrap_or(60).clamp(2, 200) as usize;
    let mut ranked = Vec::new();
    for candidate in candidates {
        let is_seed = seed_id_set.contains(&candidate.id);
        // 告知・活動報告は作品の連載として束ねない。
        // 1つの連載に見えるのを止める。利用者が自分で基準に選んだものは
        // そのまま尊重するので、除くのは種以外だけ。
        if !is_seed
            && collection_rules::is_administrative_post(
                &candidate.title,
                &candidate.content_type,
                candidate.text_length,
            )
        {
            continue;
        }
        if !is_seed && suggestion_pair_is_rejected(&conn, &candidate, &seed_works)? {
            continue;
        }
        // 手がかりを二つに分ける。
        //
        //   binding  この二作が**同じ束に属する**と言える証拠
        //   context  順位づけと説明にだけ使う、周辺の事情
        //
        // 以前は両方を一つの点数へ足していた。公式シリーズ 0.58 と
        // 同一作者 0.12 を足すと必ず 0.70 になり、採用閾値 0.44 を越える。
        // その結果、151作のシリーズを種にすると「同じ棚に載っている」だけの
        // 作品が60件、チェックの付かないまま並んだ。足すのをやめる。
        let mut evidence = Vec::new();
        let mut binding = 0.0_f64;
        let mut context = 0.0_f64;
        let link_order = None;
        let link_depth = link_paths.get(&candidate.id).map(|path| path.depth);
        if is_seed {
            binding = 1.0;
            evidence.push(CollectionSuggestionEvidence {
                kind: "seed".to_string(),
                label: "選択した基準作品".to_string(),
                contribution: 1.0,
            });
        }

        // --- 束の証拠 ------------------------------------------------

        if let Some(path) = link_paths.get(&candidate.id).copied().filter(|_| !is_seed) {
            let direct = strongest_link_with_seeds(&conn, candidate.id, &seed_json)?;
            let (label, confidence) = if let Some((relation, confidence, _)) = direct {
                (
                    format!("本文・キャプションの直接リンク（{relation}）"),
                    confidence,
                )
            } else {
                (
                    format!("本文のリンクを{}回たどってつながる", path.depth),
                    path.confidence,
                )
            };
            let depth_decay = path.depth.saturating_sub(1) as f64 * 0.08;
            let contribution = (confidence * (0.82 - depth_decay)).clamp(0.46, 0.80);
            add_suggestion_evidence(
                &mut evidence,
                &mut binding,
                "content_link",
                &label,
                contribution,
            );
        }

        let (candidate_stem, _) = title_stem_and_order(&candidate.title);
        // 話数は表示側の語彙で読む。`title_stem_and_order` は検索用の
        // 正規化を通すので、丸数字が裸の数字に化けて読めなくなっていた。
        let episode_order = collection_rules::episode_order(&candidate.title);
        let title_similarity = seed_works
            .iter()
            .map(|seed| {
                let (seed_stem, _) = title_stem_and_order(&seed.title);
                if seed_stem.is_empty() || candidate_stem.is_empty() {
                    0.0
                } else {
                    normalized_levenshtein(&seed_stem, &candidate_stem)
                }
            })
            .fold(0.0_f64, f64::max);
        if title_similarity >= 0.58 && !is_seed {
            let contribution = (((title_similarity - 0.58) / 0.42) * 0.48 + 0.14).clamp(0.14, 0.62);
            add_suggestion_evidence(
                &mut evidence,
                &mut binding,
                "title_similarity",
                // 割合ではなく、何が同じなのかを書く。74% は根拠にならない。
                &format!(
                    "題名が「{}」で共通",
                    collection_rules::clamp_name(
                        &collection_rules::display_title_stem(&candidate.title),
                        18
                    )
                ),
                contribution,
            );
        }

        // 同じシリーズに載っていることと、続けて読むべきことは違う。
        // 話数が隣り合っているときだけ束の証拠として数える。
        let series_run = shared_series_run_with_seeds(&conn, candidate.id, &seed_json)?;
        if let Some((title, gap)) = series_run.as_ref().filter(|_| !is_seed) {
            if *gap <= SERIES_ADJACENT_GAP {
                add_suggestion_evidence(
                    &mut evidence,
                    &mut binding,
                    "series_run",
                    &format!("公式シリーズ「{title}」で話数が隣接"),
                    0.52,
                );
            }
        }

        // --- 文脈 ----------------------------------------------------

        let shared_series = shared_series_with_seeds(&conn, candidate.id, &seed_json)?;
        let series_order = shared_series.iter().filter_map(|value| value.1).min();
        if let Some((title, _)) = shared_series.first() {
            let already_bound = evidence.iter().any(|value| value.kind == "series_run");
            if !already_bound {
                add_suggestion_evidence(
                    &mut evidence,
                    &mut context,
                    "official_series",
                    &format!("公式シリーズ「{title}」に一緒に入っている"),
                    0.10,
                );
            }
        }

        let same_author = seed_works.iter().any(|seed| {
            (!candidate.author_id.is_empty()
                && candidate.source == seed.source
                && candidate.author_id == seed.author_id)
                || (!candidate.author_name.trim().is_empty()
                    && candidate
                        .author_name
                        .eq_ignore_ascii_case(&seed.author_name))
        });
        if same_author && !is_seed {
            add_suggestion_evidence(
                &mut evidence,
                &mut context,
                "same_author",
                if seed_works
                    .iter()
                    .any(|seed| seed.source != candidate.source)
                {
                    "同じ作者名（取得元をまたぐ）"
                } else {
                    "同じ作者"
                },
                0.08,
            );
        }

        // タグは方針の文書に書いてあったのに使われていなかった。
        // 束の証拠にはしない（同じタグの作品は棚に何百とある）が、
        // 何が共通なのかを言い当てるのはたいていタグである。
        if !is_seed {
            let tags = shared_tags_with_seeds(&conn, candidate.id, &seed_json)?;
            if tags.len() >= 2 {
                let contribution = (tags.len() as f64 * 0.03).clamp(0.06, 0.14);
                add_suggestion_evidence(
                    &mut evidence,
                    &mut context,
                    "shared_tags",
                    &format!(
                        "共有タグ {}",
                        tags.iter().take(3).cloned().collect::<Vec<_>>().join("・")
                    ),
                    contribution,
                );
            }
        }

        // 投稿時期。続き物は近い時期に出ることが多い。
        if !is_seed && !candidate.published_at.is_empty() {
            let nearest = seed_works
                .iter()
                .filter_map(|seed| {
                    published_days_apart(&candidate.published_at, &seed.published_at)
                })
                .min();
            if let Some(days) = nearest.filter(|days| *days <= PUBLISH_NEARBY_DAYS) {
                add_suggestion_evidence(
                    &mut evidence,
                    &mut context,
                    "published_nearby",
                    &if days == 0 {
                        "同じ日に投稿".to_string()
                    } else {
                        format!("投稿が{days}日違い")
                    },
                    0.06,
                );
            }
        }

        if let Some(semantic_score) = semantic_scores.get(&candidate.id).copied() {
            if semantic_score >= 0.45 && !is_seed {
                let contribution = ((semantic_score - 0.4) * 0.35).clamp(0.02, 0.20);
                add_suggestion_evidence(
                    &mut evidence,
                    &mut context,
                    "semantic_similarity",
                    "本文の内容が近い",
                    contribution,
                );
            }
        }

        // **束の証拠が一つも無いものは候補にしない。** ここが要である。
        // 文脈だけで並ぶ候補は、利用者が選べないまま画面を埋めるだけだった。
        if !is_seed && binding <= 0.0 {
            continue;
        }
        let score = (binding + context).clamp(0.0, 1.0);
        // 設計提案 6-6 は弱い信号だけで 2 段以上連鎖させないとしている。
        // 遠いリンク先は候補として見せるが、既定では選ばない。題名や公式順で
        // 裏付けが取れているものはこの制限から外す。
        let corroborated = series_order.is_some()
            || episode_order.is_some()
            || evidence
                .iter()
                .any(|value| matches!(value.kind.as_str(), "title_similarity" | "series_run"));
        let distant_link = link_depth.is_some_and(|depth| depth >= 2) && !corroborated;
        let default_selected = is_seed || (binding >= 0.45 && !distant_link);
        ranked.push(RankedSuggestionMember {
            work: candidate,
            member_score: score,
            title_stem: candidate_stem,
            default_selected,
            evidence,
            series_order,
            link_order,
            link_depth,
            episode_order,
        });
    }
    assign_link_graph_order(&conn, &mut ranked)?;
    assign_unnumbered_opening_order(&mut ranked);
    flag_duplicate_content_members(&mut ranked);
    ranked.sort_by(compare_ranked_suggestion_members);
    let effective_limit = limit.max(seed_ids.len());
    if ranked.len() > effective_limit {
        let mut retained = ranked
            .iter()
            .filter(|value| seed_id_set.contains(&value.work.id))
            .cloned()
            .collect::<Vec<_>>();
        let remaining = effective_limit.saturating_sub(retained.len());
        retained.extend(
            ranked
                .iter()
                .filter(|value| !seed_id_set.contains(&value.work.id))
                .take(remaining)
                .cloned(),
        );
        ranked = retained;
        ranked.sort_by(compare_ranked_suggestion_members);
    }
    if !ranked
        .iter()
        .any(|value| !seed_id_set.contains(&value.work.id))
    {
        return Err("関連作品は見つかりませんでした".to_string());
    }
    let name_options = collection_name_options(&conn, &ranked, &seed_json)?;
    let proposed_name = name_options
        .first()
        .map(|value| value.name.clone())
        .unwrap_or_else(|| "読書コレクション".to_string());
    // 「1 件でも順序を持てば全体が順序付き」にはしない。20 件の短編集に
    // たまたま話数付きが 1 件混じっただけで、通読順のある連載として
    // 提示してしまうためである。過半数が順序を持つときだけ順序付きとする。
    let ordered_members = ranked
        .iter()
        .filter(|value| {
            value.series_order.is_some()
                || value.link_order.is_some()
                || value.episode_order.is_some()
        })
        .count();
    let collection_kind = if ranked.len() > 1 && ordered_members * 2 > ranked.len() {
        "ordered"
    } else {
        "unordered"
    }
    .to_string();
    let overall_score = ranked
        .iter()
        .filter(|value| !seed_id_set.contains(&value.work.id))
        .map(|value| value.member_score)
        .reduce(f64::max)
        .unwrap_or(0.0);
    let members = ranked
        .into_iter()
        .enumerate()
        .map(|(position, value)| CollectionSuggestionMember {
            source: value.work.source,
            source_id: value.work.source_id,
            corpus_key: Some(value.work.id),
            work: None,
            title: value.work.title,
            author_name: value.work.author_name,
            cover_path: value.work.cover_path,
            text_length: value.work.text_length,
            proposed_position: position as i64,
            score: value.member_score,
            selected: value.default_selected,
            evidence: value.evidence,
        })
        .collect::<Vec<_>>();
    let evidence_summary = suggestion_evidence_summary(&members, &seed_id_set);
    Ok(SeedPlan {
        seed_ids,
        proposed_name,
        name_options,
        collection_kind,
        score: overall_score,
        evidence_summary,
        members,
    })
}

pub(crate) fn accepted_suggestion_name_source(
    suggestion: &CollectionSuggestion,
    selected_name: Option<&str>,
) -> String {
    match selected_name {
        Some(name) => suggestion
            .name_options
            .iter()
            .find(|option| option.name == name)
            .map(|option| option.source.clone())
            .unwrap_or_else(|| "manual".to_string()),
        None => suggestion
            .name_options
            .first()
            .map(|option| option.source.clone())
            .unwrap_or_else(|| "title".to_string()),
    }
}

/// 命名の材料（題名・作者・シリーズ名・タグ）。本文は含めない。
pub(crate) fn naming_works_in(
    conn: &Connection,
    download_ids: &[i64],
) -> Result<Vec<crate::assist::NamingWork>, String> {
    if download_ids.is_empty() {
        return Ok(Vec::new());
    }
    let encoded = serde_json::to_string(download_ids)
        .map_err(|e| format!("Failed to encode naming work IDs: {e}"))?;
    let mut stmt = conn
        .prepare(
            "SELECT d.title, d.author_name,
                    (SELECT ds.title FROM download_series ds WHERE ds.download_id = d.id LIMIT 1),
                    COALESCE((
                      SELECT json_group_array(t.name)
                      FROM download_tags dt JOIN tags t ON t.id = dt.tag_id
                      WHERE dt.download_id = d.id
                    ), '[]')
             FROM downloads d
             WHERE d.id IN (SELECT value FROM json_each(?1))
             ORDER BY COALESCE(d.source_created_at, d.downloaded_at) ASC, d.id ASC",
        )
        .map_err(|e| format!("Failed to prepare naming works: {e}"))?;
    let rows = stmt
        .query_map(params![encoded], |row| {
            let tags = row
                .get::<_, Option<String>>(3)?
                .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
                .unwrap_or_default();
            Ok(crate::assist::NamingWork {
                title: row.get(0)?,
                author_name: row.get(1)?,
                series_title: row
                    .get::<_, Option<String>>(2)?
                    .filter(|title| !collection_rules::is_administrative_series_label(title)),
                tags,
            })
        })
        .map_err(|e| format!("Failed to query naming works: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read naming works: {e}"))
}

#[cfg(test)]
mod collection_write_tests {
    use super::*;

    #[test]
    fn accepted_suggestion_keeps_the_selected_name_provenance() {
        let suggestion = CollectionSuggestion {
            id: "suggestion-test".to_string(),
            proposed_name: "題名からの案".to_string(),
            name_options: vec![
                CollectionNameCandidate {
                    source: "title".to_string(),
                    name: "題名からの案".to_string(),
                    label: "題名".to_string(),
                    ..Default::default()
                },
                CollectionNameCandidate {
                    source: "llm".to_string(),
                    name: "モデルの案".to_string(),
                    label: "モデル".to_string(),
                    ..Default::default()
                },
            ],
            collection_kind: "ordered".to_string(),
            track: "sequence".to_string(),
            origin: "sweep".to_string(),
            evidence_summary: String::new(),
            score: 1.0,
            rule_version: "test".to_string(),
            state: "pending".to_string(),
            members: Vec::new(),
            created_at: String::new(),
            updated_at: String::new(),
        };

        assert_eq!(
            accepted_suggestion_name_source(&suggestion, Some("モデルの案")),
            "llm"
        );
        assert_eq!(
            accepted_suggestion_name_source(&suggestion, Some("自分で付けた名前")),
            "manual"
        );
        assert_eq!(accepted_suggestion_name_source(&suggestion, None), "title");
    }
}
