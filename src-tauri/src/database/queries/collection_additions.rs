//! すでにあるコレクションへ、あとから入れるとよさそうな作品を探す。
//!
//! 束は作った時点で閉じない。新作は毎日届くし、旧作をあとから保存することも
//! ある。「作ったときの顔ぶれ」に縛られる理由が無いのは、名前と同じである。
//!
//! 棚全体の走査（[`super::collection_sweep`]）とは目的が違う。走査は
//! **まだ無い束**を探すが、ここは**すでにある束の欠けている一片**を探す。
//! だから起点は無作為の棚ではなく、いま入っている顔ぶれそのものである。
//!
//! 測り方は走査と同じ土俵に乗せる。棚の水準からの隔たり（z）で近さを測り、
//! 規則で見つかる根拠（本文リンク、公式シリーズ、作者、題名族、共有タグ）は
//! 別に数える。**強いほうを採る** — 意味索引が無い棚でも規則だけで答えが出るし、
//! 規則で拾えない題材の一致は本文ベクトルが拾う。

use super::*;

use crate::database::collection_rules;
use crate::database::queries::collection_sweep::{
    cosine, shelf_baseline, theme_strength, THEME_MIN_Z,
};

/// 一度に出す候補の数。
///
/// 走査と同じ考え方で少数に絞る。1件ずつ「入れるか」を決める操作なので、
/// 一画面で片付く数でなければ、結局まとめて閉じられる。
const MAX_ADDITION_CANDIDATES: usize = 6;
/// これを下回る確からしさの作品は出さない。
///
/// 「候補が無い」と正直に言うほうが、薄い候補で埋めるよりよい。
const MIN_ADDITION_CONFIDENCE: f64 = 0.62;
/// 束へ足す候補は、棚から新しい束を見つけるときより厳しく。すでにある束を
/// 汚すほうが、見つけ損なうより高くつく。組み上げの時点で守らせる。
const _: () = assert!(
    MIN_ADDITION_CONFIDENCE >= crate::database::queries::collection_sweep::MIN_STRENGTH
);
/// 重み付き抽出の効き具合。走査より強く上位へ寄せる。
///
/// 走査は「棚に何があるか」を見せる操作なので幅が要るが、こちらは
/// 「この束に足すなら何か」なので、確からしさを優先する。
const ADDITION_SELECTION_SHARPNESS: f64 = 12.0;
/// 近さを測る起点にする、束のメンバー数の上限。
///
/// 40作の束で全員と比べても答えは変わらず、時間だけ40倍かかる。等間隔に
/// 抜くのは、先頭だけを見ると「1話に似た作品」しか出てこなくなるためである。
const MAX_SEED_MEMBERS: usize = 24;
/// 束のタグ像に入れるタグの数。
const MAX_PROFILE_TAGS: usize = 12;
/// 「同じ公式シリーズ」を強い根拠として認める、シリーズの大きさの上限。
///
/// 取得元の「シリーズ」は、読む順のある連載とはかぎらない。実測では、ある束の
/// メンバーが属していた pixiv シリーズ 8434120 は**151作**あり、中身は対魔忍も
/// アイマスも混ざった「依頼もの置き場」だった。これを 0.95 の根拠にすると、
/// 4作の束に無関係な151作が「同じ公式シリーズです」と言って並ぶ。
///
/// 読む単位として持てる大きさは束と同じである（`MAX_BUNDLE` と同じ40）。
/// これを超えるシリーズは、連載ではなく**棚**である。
const SERIES_AS_STRONG_EVIDENCE_MAX: usize = 40;
/// 大きすぎるシリーズに残す確からしさ。
///
/// 0 にはしない。同じ置き場に入れている以上、作者の中では近いものではある。
/// ただし**それだけでは束に入れない** — 下限(0.62)を割る値にしてあるので、
/// タグや本文の近さがもう一つ要る。
const OVERSIZED_SERIES_CONFIDENCE: f64 = 0.60;

/// 候補ひとつぶんの、根拠を持った採点結果。
struct ScoredCandidate {
    work: AdditionWork,
    confidence: f64,
    reason: String,
    evidence: Vec<CollectionSuggestionEvidence>,
}

/// 追加候補を測るために要る、作品ひとつぶん。
#[derive(Debug, Clone)]
struct AdditionWork {
    id: i64,
    source: String,
    source_id: String,
    title: String,
    author_name: String,
    author_id: String,
    cover_path: Option<String>,
    text_length: i64,
    published_at: String,
    content_type: String,
}

impl AdditionWork {
    /// 取得元をまたいで同じ作者を指す鍵。走査と同じ規則を使う。
    fn author_key(&self) -> String {
        let name = self.author_name.trim();
        if name.is_empty() {
            format!("{}:{}", self.source, self.author_id)
        } else {
            crate::database::search::normalize_search_text(name)
        }
    }
}

impl Database {
    /// この束に、あとから入れるとよさそうな作品を探す。
    ///
    /// 保存はしない。返すのは案だけで、入れるかどうかは利用者が決める。
    pub fn suggest_collection_additions(
        &self,
        collection_id: &str,
    ) -> Result<CollectionAdditionResult, String> {
        let collection_id = validate_collection_id(collection_id)?.to_string();
        let conn = self.read_conn()?;

        let (collection_name, member_ids) = load_collection_shape(&conn, &collection_id)?;
        let empty = |note: &str| CollectionAdditionResult {
            collection_id: collection_id.clone(),
            collection_name: collection_name.clone(),
            candidates: Vec::new(),
            semantic_used: false,
            note: Some(note.to_string()),
            eligible_count: 0,
        };
        if member_ids.is_empty() {
            return Ok(empty(
                "この束にはまだ棚の作品が入っていないので、似ているものを測る起点がありません。",
            ));
        }

        let works = load_addition_works(&conn)?;
        let member_id_set = member_ids.iter().copied().collect::<HashSet<_>>();
        let members = works
            .iter()
            .filter(|work| member_id_set.contains(&work.id))
            .cloned()
            .collect::<Vec<_>>();
        if members.is_empty() {
            return Ok(empty(
                "この束の作品が棚に見つかりません。保存し直すと候補を探せます。",
            ));
        }

        // 規則の材料をまとめて読む。1件ずつ問い合わせると、候補の数だけ
        // 往復が増えて棚の大きさに比例して遅くなる。
        let profile = MemberProfile::load(&conn, &works, &members, &member_ids)?;
        let seeds = pick_seed_members(&members);

        // 意味索引は「あれば使う」。無い棚でも規則だけで答えは出る。
        let wanted = works.iter().map(|work| work.id).collect::<HashSet<_>>();
        let (centroids, semantic_note) =
            match crate::database::semantic_index::work_centroids(&self.storage_dir, &wanted) {
                Ok(values) if values.len() >= 2 => (values, None),
                Ok(_) => (
                    HashMap::new(),
                    Some(
                        "本文ベクトルがまだ足りないので、題名・作者・タグ・リンクだけで探しました。"
                            .to_string(),
                    ),
                ),
                Err(error) => {
                    log::warn!("Collection additions ran without semantic index: {error}");
                    (
                        HashMap::new(),
                        Some(
                            "意味索引が読めないので、題名・作者・タグ・リンクだけで探しました。"
                                .to_string(),
                        ),
                    )
                }
            };
        let semantic_used = !centroids.is_empty();
        let baseline = semantic_used.then(|| shelf_baseline(&centroids));
        let seed_vectors = seeds
            .iter()
            .filter_map(|work| centroids.get(&work.id))
            .collect::<Vec<_>>();

        let mut scored = Vec::new();
        for work in &works {
            if member_id_set.contains(&work.id) {
                continue;
            }
            if collection_rules::is_administrative_post(
                &work.title,
                &work.content_type,
                work.text_length,
            ) {
                continue;
            }
            // 「この二作は同じ束ではない」と言われた組は、そのまま尊重する。
            if profile.rejected_against_members(work) {
                continue;
            }
            let rules = rule_confidence(work, &profile);
            let semantic = match (&baseline, centroids.get(&work.id)) {
                (Some(baseline), Some(vector)) if !seed_vectors.is_empty() => {
                    let mut total = 0.0;
                    let mut compared = 0usize;
                    for seed in &seed_vectors {
                        if let Some(value) = cosine(seed, vector) {
                            total += value;
                            compared += 1;
                        }
                    }
                    // 平均を採る。走査の `similar_works` は最大を採るが、それは
                    // 「1作にだけ似ていればよい」という規則である。束へ足すなら
                    // **束全体に馴染むか**を見たいので、最大では緩すぎる。
                    if compared == 0 {
                        None
                    } else {
                        let z = baseline.z(total / compared as f64);
                        (z >= THEME_MIN_Z).then(|| (theme_strength(z), z))
                    }
                }
                _ => None,
            };

            let Some((confidence, reason, evidence)) = combine(rules, semantic) else {
                continue;
            };
            if confidence < MIN_ADDITION_CONFIDENCE {
                continue;
            }
            scored.push(ScoredCandidate {
                work: work.clone(),
                confidence,
                reason,
                evidence,
            });
        }

        let eligible_count = scored.len() as i64;
        let candidates = select_candidates(scored)
            .into_iter()
            .map(|scored| CollectionAdditionCandidate {
                source: scored.work.source,
                source_id: scored.work.source_id,
                download_id: scored.work.id,
                title: scored.work.title,
                author_name: scored.work.author_name,
                cover_path: scored.work.cover_path,
                text_length: scored.work.text_length,
                published_at: scored.work.published_at,
                confidence: scored.confidence,
                reason: scored.reason,
                evidence: scored.evidence,
            })
            .collect::<Vec<_>>();

        Ok(CollectionAdditionResult {
            collection_id,
            collection_name,
            candidates,
            semantic_used,
            note: semantic_note,
            eligible_count,
        })
    }
}

/// 束の名前と、いま入っている作品。
fn load_collection_shape(
    conn: &Connection,
    collection_id: &str,
) -> Result<(String, Vec<i64>), String> {
    let name = conn
        .query_row(
            "SELECT name FROM work_collections WHERE id = ?1",
            params![collection_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to read collection: {e}"))?
        .ok_or_else(|| "Collection not found".to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT download_id
               FROM work_collection_members
              WHERE collection_id = ?1 AND download_id IS NOT NULL
              ORDER BY position ASC, created_at ASC",
        )
        .map_err(|e| format!("Failed to prepare collection members: {e}"))?;
    // 棚に本体が無い行も束のメンバーではあるが、近さは測れないので数えない。
    let rows = stmt
        .query_map(params![collection_id], |row| row.get::<_, i64>(0))
        .map_err(|e| format!("Failed to query collection members: {e}"))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row.map_err(|e| format!("Failed to read collection members: {e}"))?);
    }
    Ok((name, ids))
}

/// 候補になりうる作品を、棚から一度に読む。
fn load_addition_works(conn: &Connection) -> Result<Vec<AdditionWork>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, source, source_id, title, author_name, COALESCE(author_id, ''),
                    cover_path, text_length, COALESCE(source_created_at, downloaded_at, ''),
                    content_type
             FROM downloads",
        )
        .map_err(|e| format!("Failed to prepare addition works: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(AdditionWork {
                id: row.get(0)?,
                source: row.get(1)?,
                source_id: row.get(2)?,
                title: row.get(3)?,
                author_name: row.get(4)?,
                author_id: row.get(5)?,
                cover_path: row.get(6)?,
                text_length: row.get(7)?,
                published_at: row.get(8)?,
                content_type: row.get(9)?,
            })
        })
        .map_err(|e| format!("Failed to query addition works: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| format!("Failed to read addition works: {e}"))?);
    }
    Ok(out)
}

/// 近さを測る起点にするメンバー。多すぎる束からは等間隔に抜く。
///
/// 先頭から24作を採ると、長い連載では「1話のあたりに似た作品」しか出てこない。
/// 束の端から端までを代表させたいので、等間隔に散らす。
fn pick_seed_members(members: &[AdditionWork]) -> Vec<AdditionWork> {
    if members.len() <= MAX_SEED_MEMBERS {
        return members.to_vec();
    }
    let step = members.len() as f64 / MAX_SEED_MEMBERS as f64;
    (0..MAX_SEED_MEMBERS)
        .map(|index| {
            let position = ((index as f64 * step).floor() as usize).min(members.len() - 1);
            members[position].clone()
        })
        .collect()
}

/// 規則で見つかる根拠をまとめて持つ、束の像。
struct MemberProfile {
    /// 束に入っている作者（表示名を正規化した鍵）。
    author_keys: HashSet<String>,
    /// 束に入っている題名族の鍵。
    family_keys: HashSet<String>,
    /// 束が属する公式シリーズのうち、読む単位として通るもの。
    series_keys: HashSet<String>,
    /// 束が属するシリーズのうち、大きすぎて連載とは呼べないもの。
    oversized_series_keys: HashSet<String>,
    /// 束のよく使うタグ。多い順に上位だけ。
    profile_tags: HashSet<String>,
    /// 束のメンバーと本文リンクでつながっている作品と、その確信度。
    linked: HashMap<i64, f64>,
    /// 作品ごとのタグ。共有タグを数えるのに使う。
    tags_by_work: HashMap<i64, HashSet<String>>,
    /// 作品ごとの公式シリーズ。
    series_by_work: HashMap<i64, HashSet<String>>,
    /// 束のメンバーとの組で「二度と出さない」と言われた作品。
    rejected: HashSet<i64>,
}

impl MemberProfile {
    fn load(
        conn: &Connection,
        works: &[AdditionWork],
        members: &[AdditionWork],
        member_ids: &[i64],
    ) -> Result<Self, String> {
        let member_json = serde_json::to_string(member_ids)
            .map_err(|e| format!("Failed to encode collection members: {e}"))?;

        // タグは棚ぶんまとめて読む。候補ごとに引くと、棚の作品数だけ往復する。
        let mut tags_by_work: HashMap<i64, HashSet<String>> = HashMap::new();
        {
            let mut stmt = conn
                .prepare(
                    "SELECT dt.download_id, t.name
                       FROM download_tags dt JOIN tags t ON t.id = dt.tag_id",
                )
                .map_err(|e| format!("Failed to prepare addition tags: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| format!("Failed to query addition tags: {e}"))?;
            for row in rows {
                let (download_id, name) =
                    row.map_err(|e| format!("Failed to read addition tags: {e}"))?;
                if !collection_rules::is_informative_tag(&name) {
                    continue;
                }
                tags_by_work.entry(download_id).or_default().insert(name);
            }
        }

        let mut series_by_work: HashMap<i64, HashSet<String>> = HashMap::new();
        // シリーズごとの大きさと呼び名も一緒に数える。「同じシリーズ」がどれだけ
        // のことを言っているかは、その大きさで決まる。
        let mut series_size: HashMap<String, usize> = HashMap::new();
        let mut series_label: HashMap<String, String> = HashMap::new();
        {
            let mut stmt = conn
                .prepare("SELECT download_id, series_source, series_key, title FROM download_series")
                .map_err(|e| format!("Failed to prepare addition series: {e}"))?;
            let rows = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        format!("{}:{}", row.get::<_, String>(1)?, row.get::<_, String>(2)?),
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(|e| format!("Failed to query addition series: {e}"))?;
            for row in rows {
                let (download_id, key, title) =
                    row.map_err(|e| format!("Failed to read addition series: {e}"))?;
                // 同じ作品が同じシリーズに二行あっても、大きさは一度だけ数える。
                if series_by_work
                    .entry(download_id)
                    .or_default()
                    .insert(key.clone())
                {
                    *series_size.entry(key.clone()).or_default() += 1;
                }
                series_label.entry(key).or_insert(title);
            }
        }

        // 束のメンバーへ、どちらの向きからでもつながっている作品。
        let mut linked: HashMap<i64, f64> = HashMap::new();
        {
            let mut stmt = conn
                .prepare(
                    "SELECT from_download_id, to_download_id, confidence
                       FROM work_links
                      WHERE status != 'rejected'
                        AND from_download_id IS NOT NULL
                        AND to_download_id IS NOT NULL
                        AND confidence >= 0.6
                        AND (from_download_id IN (SELECT value FROM json_each(?1))
                          OR to_download_id IN (SELECT value FROM json_each(?1)))",
                )
                .map_err(|e| format!("Failed to prepare addition links: {e}"))?;
            let rows = stmt
                .query_map(params![member_json], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, f64>(2)?,
                    ))
                })
                .map_err(|e| format!("Failed to query addition links: {e}"))?;
            let member_set = member_ids.iter().copied().collect::<HashSet<_>>();
            for row in rows {
                let (from, to, confidence) =
                    row.map_err(|e| format!("Failed to read addition links: {e}"))?;
                // 束の外側の端点だけを候補として拾う。両端が束の中なら、
                // それは束の内部の話であって、足す相手ではない。
                for outside in [from, to] {
                    if !member_set.contains(&outside) {
                        linked
                            .entry(outside)
                            .and_modify(|value| *value = value.max(confidence))
                            .or_insert(confidence);
                    }
                }
            }
        }

        // 「この組合せは二度と出さない」と言われた相手。束のどのメンバーに
        // 対してでも一度そう言われていれば、この束の候補からは外す。
        let mut rejected = HashSet::new();
        {
            let mut stmt = conn
                .prepare(
                    "SELECT left_source, left_source_id, right_source, right_source_id
                       FROM collection_pair_feedback
                      WHERE decision = 'reject' AND rule_version = ?1",
                )
                .map_err(|e| format!("Failed to prepare addition feedback: {e}"))?;
            let rows = stmt
                .query_map(params![COLLECTION_SUGGEST_RULE_VERSION], |row| {
                    Ok((
                        WorkKey {
                            source: row.get::<_, String>(0)?,
                            source_id: row.get::<_, String>(1)?,
                        },
                        WorkKey {
                            source: row.get::<_, String>(2)?,
                            source_id: row.get::<_, String>(3)?,
                        },
                    ))
                })
                .map_err(|e| format!("Failed to query addition feedback: {e}"))?;
            let member_work_keys = members
                .iter()
                .map(|work| (work.source.clone(), work.source_id.clone()))
                .collect::<HashSet<_>>();
            // 候補側の id は、すでに読んである棚から引く。同じ表をもう一度
            // なめる理由は無い。
            let id_by_key = works
                .iter()
                .map(|work| ((work.source.clone(), work.source_id.clone()), work.id))
                .collect::<HashMap<_, _>>();
            let mut pairs = Vec::new();
            for row in rows {
                pairs.push(row.map_err(|e| format!("Failed to read addition feedback: {e}"))?);
            }
            for (left, right) in pairs {
                let left_key = (left.source.clone(), left.source_id.clone());
                let right_key = (right.source.clone(), right.source_id.clone());
                let outside = if member_work_keys.contains(&left_key) {
                    Some(right_key)
                } else if member_work_keys.contains(&right_key) {
                    Some(left_key)
                } else {
                    None
                };
                if let Some(key) = outside {
                    if let Some(id) = id_by_key.get(&key) {
                        rejected.insert(*id);
                    }
                }
            }
        }

        let member_id_set = member_ids.iter().copied().collect::<HashSet<_>>();

        // 束のメンバーが属するシリーズを、読む単位として通るものと、ただの
        // 置き場に分ける。呼び名そのものが事務的なら（「リクエストシリーズ」
        // など）、大きさによらず置き場として扱う。
        let member_series = member_id_set
            .iter()
            .filter_map(|id| series_by_work.get(id))
            .flatten()
            .cloned()
            .collect::<HashSet<_>>();
        let oversized_series = member_series
            .iter()
            .filter(|key| {
                series_size.get(*key).copied().unwrap_or(0) > SERIES_AS_STRONG_EVIDENCE_MAX
                    || series_label
                        .get(*key)
                        .is_some_and(|title| collection_rules::is_administrative_series_label(title))
            })
            .cloned()
            .collect::<HashSet<String>>();

        let mut tag_counts: HashMap<String, usize> = HashMap::new();
        for id in &member_id_set {
            for tag in tags_by_work.get(id).into_iter().flatten() {
                *tag_counts.entry(tag.clone()).or_default() += 1;
            }
        }
        let mut ranked_tags = tag_counts.into_iter().collect::<Vec<_>>();
        ranked_tags.sort_by(|left, right| {
            right
                .1
                .cmp(&left.1)
                .then_with(|| left.0.cmp(&right.0))
        });
        let profile_tags = ranked_tags
            .into_iter()
            .take(MAX_PROFILE_TAGS)
            .map(|(tag, _)| tag)
            .collect::<HashSet<_>>();

        Ok(Self {
            author_keys: members.iter().map(AdditionWork::author_key).collect(),
            family_keys: members
                .iter()
                .map(|work| collection_rules::family_match_key(&work.title))
                .filter(|key| key.chars().count() >= 9)
                .map(|key| key.chars().take(26).collect())
                .collect(),
            series_keys: member_series
                .iter()
                .filter(|key| !oversized_series.contains(*key))
                .cloned()
                .collect(),
            oversized_series_keys: oversized_series,
            profile_tags,
            linked,
            tags_by_work,
            series_by_work,
            rejected,
        })
    }

    fn rejected_against_members(&self, work: &AdditionWork) -> bool {
        self.rejected.contains(&work.id)
    }

    fn shared_tags(&self, work: &AdditionWork) -> usize {
        self.tags_by_work
            .get(&work.id)
            .map(|tags| tags.iter().filter(|tag| self.profile_tags.contains(*tag)).count())
            .unwrap_or(0)
    }

    /// 読む単位として通るシリーズを共有しているか。
    fn shares_series(&self, work: &AdditionWork) -> bool {
        self.series_by_work
            .get(&work.id)
            .is_some_and(|keys| keys.iter().any(|key| self.series_keys.contains(key)))
    }

    /// 「置き場」を共有しているだけか。近いことは近いが、それだけでは足せない。
    fn shares_oversized_series(&self, work: &AdditionWork) -> bool {
        self.series_by_work
            .get(&work.id)
            .is_some_and(|keys| keys.iter().any(|key| self.oversized_series_keys.contains(key)))
    }

    fn shares_family(&self, work: &AdditionWork) -> bool {
        let key = collection_rules::family_match_key(&work.title);
        if key.chars().count() < 9 {
            return false;
        }
        let key = key.chars().take(26).collect::<String>();
        self.family_keys.contains(&key)
    }
}

/// 規則だけで測った確からしさと、その一行。
///
/// 数字は走査の確度と同じ土俵に置く。本文リンクと公式シリーズは走査でも
/// いちばん強い根拠なので、ここでも同じ高さにする。
///
/// **当てはまったものを順に返すのではなく、いちばん強いものを採る。**
/// 上から順に `return` していたころ、公式シリーズが同じ（0.95）作品に
/// たまたま確信度 0.61 のリンクが一本あると、リンクのほうが先に当たるので
/// 0.61 になった。下限 0.62 を割るので、**ほぼ確実な一作が黙って消えていた。**
/// 根拠が二つある作品は、一つしかない作品より弱くなってはいけない。
fn rule_confidence(
    work: &AdditionWork,
    profile: &MemberProfile,
) -> Option<(f64, String, Vec<CollectionSuggestionEvidence>)> {
    let same_author = profile.author_keys.contains(&work.author_key());
    let shared = profile.shared_tags(work);
    let mut found: Vec<(f64, String, CollectionSuggestionEvidence)> = Vec::new();

    if let Some(confidence) = profile.linked.get(&work.id) {
        found.push((
            confidence.clamp(0.0, 1.0),
            "この束の作品から、本文・キャプションでリンクされています".to_string(),
            CollectionSuggestionEvidence {
                kind: "content_link".to_string(),
                label: "本文・キャプションのリンク".to_string(),
                contribution: 0.8,
            },
        ));
    }
    if profile.shares_series(work) {
        found.push((
            0.95,
            "この束と同じ公式シリーズの作品です".to_string(),
            CollectionSuggestionEvidence {
                kind: "series_run".to_string(),
                label: "同じ公式シリーズ".to_string(),
                contribution: 0.52,
            },
        ));
    } else if profile.shares_oversized_series(work) {
        found.push((
            OVERSIZED_SERIES_CONFIDENCE,
            "この束の作品と同じシリーズ欄に置かれています（大きなまとめなので、これだけでは決め手になりません）"
                .to_string(),
            CollectionSuggestionEvidence {
                kind: "series_bucket".to_string(),
                label: "同じシリーズ欄".to_string(),
                contribution: 0.2,
            },
        ));
    }
    if same_author && profile.shares_family(work) {
        found.push((
            0.88,
            "同じ作者の、題名が同じ族の作品です".to_string(),
            CollectionSuggestionEvidence {
                kind: "title_similarity".to_string(),
                label: "同じ作者・同じ題名族".to_string(),
                contribution: 0.6,
            },
        ));
    }
    if same_author && shared >= 2 {
        found.push((
            0.72,
            format!("同じ作者で、この束のタグを{shared}つ共有しています"),
            CollectionSuggestionEvidence {
                kind: "shared_tags".to_string(),
                label: "同じ作者・共有タグ".to_string(),
                contribution: 0.5,
            },
        ));
    } else if shared >= 3 {
        found.push((
            0.64,
            format!("この束のタグを{shared}つ共有しています"),
            CollectionSuggestionEvidence {
                kind: "shared_tags".to_string(),
                label: "共有タグ".to_string(),
                contribution: 0.5,
            },
        ));
    }

    // いちばん強い根拠が、確からしさと説明の一行を決める。同点は先に挙げた
    // ほうを採る（上ほど確かな根拠を先に並べてある）。
    let best = found
        .iter()
        .enumerate()
        .max_by(|left, right| left.1 .0.total_cmp(&right.1 .0).then(right.0.cmp(&left.0)))
        .map(|(index, _)| index)?;
    let confidence = found[best].0;
    let reason = found[best].1.clone();
    // 証拠の印は、当てはまったものを全部残す。強いほうが説明の一行を決める
    // だけで、弱いほうの手がかりが無かったことにはならない。
    let evidence = found
        .into_iter()
        .map(|(_, _, evidence)| evidence)
        .collect::<Vec<_>>();
    Some((confidence, reason, evidence))
}

/// 規則と意味を一つの答えにする。
///
/// 強いほうを採り、両方が同じ作品を指しているときだけ少しだけ足す。掛け合わせ
/// にはしない — 片方しか無い作品（意味索引の無い棚、タグの付いていない新作）を
/// 0 にしてしまうためである。
fn combine(
    rules: Option<(f64, String, Vec<CollectionSuggestionEvidence>)>,
    semantic: Option<(f64, f64)>,
) -> Option<(f64, String, Vec<CollectionSuggestionEvidence>)> {
    let semantic_evidence = || CollectionSuggestionEvidence {
        kind: "semantic_similarity".to_string(),
        label: "本文の内容が近い".to_string(),
        contribution: 0.3,
    };
    match (rules, semantic) {
        (Some((rule_confidence, reason, mut evidence)), Some((semantic_confidence, _))) => {
            evidence.push(semantic_evidence());
            // 二つの独立した根拠が同じ作品を指している。どちらか一方より
            // 確かだが、足し合わせて 1.0 を超えさせはしない。
            let confidence = (rule_confidence.max(semantic_confidence) + 0.05).min(1.0);
            Some((confidence, format!("{reason}。本文の内容も近いです"), evidence))
        }
        (Some(found), None) => Some(found),
        (None, Some((confidence, z))) => Some((
            confidence,
            format!(
                "本文の内容が、この束の作品に近いです（棚のふつうより{:.1}倍ぶん近い）",
                z
            ),
            vec![semantic_evidence()],
        )),
        (None, None) => None,
    }
}

/// 出す候補を選ぶ。走査と同じ、確度を重みにした籤。
///
/// 同じ束で二度押したときに、まったく同じ6件しか出ないと、7番目以降は永久に
/// 見えない。強いものはほぼ必ず入るが、下位にも席が回る。
fn select_candidates(mut scored: Vec<ScoredCandidate>) -> Vec<ScoredCandidate> {
    if scored.len() <= MAX_ADDITION_CANDIDATES {
        scored.sort_by(|left, right| {
            right
                .confidence
                .total_cmp(&left.confidence)
                .then_with(|| left.work.id.cmp(&right.work.id))
        });
        return scored;
    }
    let mut keyed = scored
        .into_iter()
        .map(|candidate| {
            let weight = candidate
                .confidence
                .powf(ADDITION_SELECTION_SHARPNESS)
                .max(f64::MIN_POSITIVE);
            let uniform = rand::random::<f64>().clamp(f64::MIN_POSITIVE, 1.0);
            (uniform.ln() / weight, candidate)
        })
        .collect::<Vec<_>>();
    keyed.sort_by(|left, right| {
        right
            .0
            .total_cmp(&left.0)
            .then_with(|| left.1.work.id.cmp(&right.1.work.id))
    });
    keyed.truncate(MAX_ADDITION_CANDIDATES);
    let mut out = keyed
        .into_iter()
        .map(|(_, candidate)| candidate)
        .collect::<Vec<_>>();
    out.sort_by(|left, right| {
        right
            .confidence
            .total_cmp(&left.confidence)
            .then_with(|| left.work.id.cmp(&right.work.id))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_profile() -> MemberProfile {
        MemberProfile {
            author_keys: HashSet::new(),
            family_keys: HashSet::new(),
            series_keys: HashSet::new(),
            oversized_series_keys: HashSet::new(),
            profile_tags: HashSet::new(),
            linked: HashMap::new(),
            tags_by_work: HashMap::new(),
            series_by_work: HashMap::new(),
            rejected: HashSet::new(),
        }
    }

    fn work(id: i64, title: &str, author: &str) -> AdditionWork {
        AdditionWork {
            id,
            source: "pixiv".to_string(),
            source_id: id.to_string(),
            title: title.to_string(),
            author_name: author.to_string(),
            author_id: String::new(),
            cover_path: None,
            text_length: 4000,
            published_at: "2026-01-01".to_string(),
            content_type: "novel".to_string(),
        }
    }

    #[test]
    fn seed_members_span_the_whole_bundle() {
        // 長い連載から起点を抜くとき、先頭だけを見ない。1話の周辺に似た作品
        // ばかりが候補になるのを避けたい。
        let members = (0..100)
            .map(|index| work(index, &format!("連載 その{index}"), "作者"))
            .collect::<Vec<_>>();
        let seeds = pick_seed_members(&members);
        assert_eq!(seeds.len(), MAX_SEED_MEMBERS);
        assert_eq!(seeds.first().unwrap().id, 0);
        // 最後の起点が末尾の近くにある＝束の端まで代表できている。
        assert!(seeds.last().unwrap().id >= 90, "{}", seeds.last().unwrap().id);
    }

    #[test]
    fn a_short_bundle_uses_every_member_as_a_seed() {
        let members = (0..5).map(|index| work(index, "短い", "作者")).collect::<Vec<_>>();
        assert_eq!(pick_seed_members(&members).len(), 5);
    }

    #[test]
    fn two_agreeing_signals_beat_either_alone_without_exceeding_one() {
        let rules = Some((
            0.95,
            "同じ公式シリーズ".to_string(),
            vec![CollectionSuggestionEvidence {
                kind: "series_run".to_string(),
                label: "同じ公式シリーズ".to_string(),
                contribution: 0.52,
            }],
        ));
        let (both, _, evidence) = combine(rules.clone(), Some((0.8, 3.0))).unwrap();
        let (rule_only, _, _) = combine(rules, None).unwrap();
        assert!(both > rule_only);
        assert!(both <= 1.0);
        // 二つの根拠が両方とも残る。片方を上書きしない。
        assert_eq!(evidence.len(), 2);
    }

    /// 根拠が二つある作品が、一つしかない作品より弱くなってはいけない。
    ///
    /// 上から順に返していたころ、公式シリーズが同じ（0.95）作品に確信度 0.61 の
    /// リンクが一本あるだけで 0.61 になり、下限 0.62 を割って黙って消えていた。
    #[test]
    fn the_strongest_rule_decides_not_the_first_one_that_matched() {
        let mut profile = empty_profile();
        let candidate = work(7, "続きの一作", "作者");
        profile.series_keys.insert("pixiv:series-1".to_string());
        profile
            .series_by_work
            .insert(7, HashSet::from(["pixiv:series-1".to_string()]));
        // たまたま弱いリンクが一本ある。これが強い根拠を押しのけてはいけない。
        profile.linked.insert(7, 0.61);

        let (confidence, reason, evidence) =
            rule_confidence(&candidate, &profile).expect("根拠がある");
        assert!(confidence >= MIN_ADDITION_CONFIDENCE, "{confidence}");
        assert_eq!(confidence, 0.95);
        assert!(reason.contains("公式シリーズ"), "{reason}");
        // 弱いほうの手がかりも、無かったことにはしない。
        assert_eq!(evidence.len(), 2);
    }

    /// 強いリンクは、そのままリンクとして通る。
    #[test]
    fn a_strong_link_still_speaks_for_itself() {
        let mut profile = empty_profile();
        profile.linked.insert(7, 0.98);
        let (confidence, reason, _) =
            rule_confidence(&work(7, "つながっている", "作者"), &profile).expect("根拠がある");
        assert_eq!(confidence, 0.98);
        assert!(reason.contains("リンク"), "{reason}");
    }

    /// 取得元の「シリーズ」は、連載とはかぎらない。
    ///
    /// 実測では151作の「依頼もの置き場」を共有しているだけで、4作の束へ
    /// 無関係な作品が 0.95 の顔をして並んでいた。
    #[test]
    fn a_catch_all_series_is_not_a_reason_on_its_own() {
        let mut profile = empty_profile();
        profile
            .oversized_series_keys
            .insert("pixiv:8434120".to_string());
        profile
            .series_by_work
            .insert(7, HashSet::from(["pixiv:8434120".to_string()]));

        let (confidence, _, _) =
            rule_confidence(&work(7, "無関係な依頼もの", "作者"), &profile).expect("手がかりはある");
        // 手がかりではあるが、これだけでは束に入れない。
        assert_eq!(confidence, OVERSIZED_SERIES_CONFIDENCE);
        assert!(confidence < MIN_ADDITION_CONFIDENCE, "{confidence}");
    }

    /// 置き場でも、ほかの手がかりと重なれば通る。
    #[test]
    fn a_catch_all_series_still_counts_alongside_a_real_signal() {
        let mut profile = empty_profile();
        profile
            .oversized_series_keys
            .insert("pixiv:8434120".to_string());
        profile
            .series_by_work
            .insert(7, HashSet::from(["pixiv:8434120".to_string()]));
        profile.author_keys.insert("作者".to_string());
        profile.profile_tags.insert("催眠".to_string());
        profile.profile_tags.insert("NTR".to_string());
        profile
            .tags_by_work
            .insert(7, HashSet::from(["催眠".to_string(), "NTR".to_string()]));

        let (confidence, reason, evidence) =
            rule_confidence(&work(7, "同じ作者の作品", "作者"), &profile).expect("手がかりはある");
        assert_eq!(confidence, 0.72);
        assert!(reason.contains("同じ作者"), "{reason}");
        assert_eq!(evidence.len(), 2, "置き場の印も残る");
    }

    /// 手がかりが何も無ければ、規則は黙る。
    #[test]
    fn a_work_with_nothing_in_common_matches_no_rule() {
        assert!(rule_confidence(&work(7, "無関係", "別の作者"), &empty_profile()).is_none());
    }

    #[test]
    fn a_candidate_with_no_evidence_at_all_is_not_a_candidate() {
        assert!(combine(None, None).is_none());
    }

    #[test]
    fn selection_keeps_the_strongest_and_never_exceeds_the_cap() {
        // 籤を引くが、いちばん強い候補は事実上必ず残る。確度の12乗を重みに
        // しているので、0.99 と 0.63 では重みが100倍以上ちがう。
        let scored = (0..40)
            .map(|index| ScoredCandidate {
                work: work(index, "作品", "作者"),
                confidence: if index == 0 { 0.99 } else { 0.63 },
                reason: String::new(),
                evidence: Vec::new(),
            })
            .collect::<Vec<_>>();
        let picked = select_candidates(scored);
        assert_eq!(picked.len(), MAX_ADDITION_CANDIDATES);
        assert_eq!(picked.first().unwrap().work.id, 0);
        // 出す順は確度の順。籤の順のままにしない。
        for pair in picked.windows(2) {
            assert!(pair[0].confidence >= pair[1].confidence);
        }
    }

    #[test]
    fn a_small_pool_is_returned_whole_and_in_confidence_order() {
        let scored = vec![
            ScoredCandidate {
                work: work(1, "弱い", "作者"),
                confidence: 0.65,
                reason: String::new(),
                evidence: Vec::new(),
            },
            ScoredCandidate {
                work: work(2, "強い", "作者"),
                confidence: 0.9,
                reason: String::new(),
                evidence: Vec::new(),
            },
        ];
        let picked = select_candidates(scored);
        assert_eq!(picked.len(), 2);
        assert_eq!(picked[0].work.id, 2);
    }
}
