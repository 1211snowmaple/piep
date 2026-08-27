//! 棚全体を一度なめて、束になりそうなものを洗い出す。
//!
//! これまでの入口は「作品ページ → 種1件 → 提案1件」しか無く、3,938作の棚を
//! 誰も一度も見渡していなかった。見つかっていない束は、探しに行かないかぎり
//! 見つからない。
//!
//! 出す束は二系統に分ける。証拠の種類が違えば、順序の意味も名前の付け方も
//! 違うためである。
//!
//!   sequence  本文リンク・題名の連番・公式シリーズの連番。読む順がある
//!   theme     共有タグと本文ベクトル。順序は無いが、味が同じ
//!
//! 一つの点数に混ぜたのが、そもそもの間違いだった。

use super::*;

use crate::database::collection_rules;

/// 題名族を束と認める最小の作品数。
const MIN_BUNDLE: usize = 2;
/// 一つの束に入れる上限。これを超えるものは束ではなく棚である。
const MAX_BUNDLE: usize = 40;
/// 二つの束を同じ塊とみなす、小さいほうに対する重なりの割合。
///
/// ここを 50% にしたら、タグ起点の束が数珠つなぎに接合した。作品は複数の
/// タグを持つので、半分の重なりは「たまたま」で起きる。
const MERGE_OVERLAP_PERCENT: usize = 80;
/// テーマの起点にするタグが、これより多くの作品に付いていたら使わない。
///
/// 「催眠」759作は、まとまりではなく**絞り込みの結果**である。束として出さず、
/// 保存した検索として提案するほうが正しい。
const THEME_TAG_MAX: usize = 60;
/// テーマの起点にするタグの最小。2作では「味が同じ」とは言えない。
const THEME_TAG_MIN: usize = 3;
/// 束として認める、棚の水準からの隔たり。
///
/// 余弦の絶対値をしきい値にしてはいけない。同じ題材ばかりの棚では、
/// **無作為に選んだ2作でも余弦が 0.94 ある**（実測）。0.72 のような数字は
/// 何も弾かず、走査は棚の半分を束にしてしまった。
///
/// 測るべきは「その棚のふつうの近さと比べて、どれだけ近いか」である。
/// 対の余弦の平均と標準偏差を棚から取り、そこからの隔たりで判断する。
/// 実測では上位の束が z = 2.1、タグ束の中央値が z = 0.85 だった。
const THEME_MIN_Z: f64 = 2.0;
/// 棚の水準を測るために取る対の数。
const BASELINE_PAIRS: usize = 20_000;
/// テーマの束に入れる上限。
///
/// 40作の「まとまり」は、まとまりではなく棚である。読む単位として持てる
/// 大きさに収まるまで締める。
const THEME_MAX_MEMBERS: usize = 24;
/// 走査でいちどに作る候補の上限。系統ごとに分けて数える。
///
/// 一つの上限を共有すると、数の多いほうが少ないほうを押し出す。実データでは
/// テーマ331件が続き物を69件まで削っていた。
const MAX_SWEEP_PER_TRACK: usize = 200;
/// 保存した検索として勧めるタグの数。多く出しても、どれも保存されない。
const MAX_SAVED_SEARCH_SUGGESTIONS: usize = 8;

/// 走査に使う、作品ひとつぶんの最小限。
#[derive(Debug, Clone)]
struct SweepWork {
    id: i64,
    source: String,
    source_id: String,
    title: String,
    author_name: String,
    author_id: String,
    cover_path: Option<String>,
    text_length: i64,
    published_at: String,
}

impl SweepWork {
    /// 取得元をまたいで同じ作者を指す鍵。
    ///
    /// `author_id` は取得元ごとに違う。同じ人が pixiv では `3259258`、FANBOX では
    /// `kimodebu-kun` を名乗る。ID で束ねると**取得元をまたげない** — pixiv の
    /// 前編と FANBOX の後編を同じまとまりにする、というコレクションの一番の
    /// 目的が果たせなくなる。
    ///
    /// だから表示名で束ねる。同姓同名の別人が同じ題名で書いている確率より、
    /// 同じ人が二つの取得元に同じ作品を出している確率のほうがはるかに高い。
    /// 呼び出し側は題名の一致も同時に要求するので、名前だけでは束ねない。
    fn author_key(&self) -> String {
        let name = self.author_name.trim();
        if name.is_empty() {
            format!("{}:{}", self.source, self.author_id)
        } else {
            crate::database::search::normalize_search_text(name)
        }
    }
}

/// 束の見つかり方。根拠の文はここから、**最後の顔ぶれに対して**組み立てる。
///
/// 文字列を先に作って持ち回ると、束をまとめたあとも古い件数が残る。実データで
/// 「本文も近い3作です」と書かれた40作の束が出た。数は数え直すしかない。
#[derive(Debug, Clone)]
enum BundleKind {
    Link,
    TitleRun,
    SeriesRun(String),
    Theme { tag: String },
}

/// 洗い出した束ひとつ。保存する前の、まだ提案でしかない状態。
struct SweepBundle {
    ids: Vec<i64>,
    track: &'static str,
    kind: BundleKind,
    /// 束ごとの証拠。メンバー全員に同じものが付く。
    member_evidence: Vec<CollectionSuggestionEvidence>,
}

impl SweepBundle {
    /// なぜこれが束なのかを、いまの顔ぶれで一行にする。
    fn evidence_sentence(&self, author_count: usize) -> String {
        let count = self.ids.len();
        match &self.kind {
            BundleKind::Link => format!("本文のリンクで{count}作がつながっています"),
            BundleKind::TitleRun => format!("題名が連番になっている{count}作です"),
            BundleKind::SeriesRun(title) => {
                format!("公式シリーズ「{title}」で{count}作が連番です")
            }
            BundleKind::Theme { tag } => {
                if author_count > 1 {
                    format!("「{tag}」を共有し、本文も近い{count}作です（作者{author_count}人）")
                } else {
                    format!("「{tag}」を共有し、本文も近い{count}作です")
                }
            }
        }
    }

    /// テーマの束は、共有タグが名前の第一候補になる。
    fn anchor_tag(&self) -> Option<&str> {
        match &self.kind {
            BundleKind::Theme { tag } => Some(tag.as_str()),
            _ => None,
        }
    }
}

impl Database {
    /// 棚全体を走査して、束の候補を作り直す。
    ///
    /// 走査で作った候補は毎回入れ替える。前回の走査で出たものが残り続けると、
    /// 「あとで」と送ったはずのものが規則を直しても消えない。**利用者が
    /// 「二度と出さない」と言ったものだけ**は `collection_pair_feedback` が
    /// 覚えているので、作り直しても戻ってこない。
    ///
    /// 1作から広げた候補（`origin = 'seed'`）には触らない。あれは利用者が
    /// 自分で作らせたものである。
    pub fn sweep_collection_candidates(&self) -> Result<CollectionSweepResult, String> {
        // 別版は代表だけを残す。残さないと「【おまけ付き】【モモ編】…」と
        // 「【モモ編】…」が別の作品として同じ束に二度並ぶ。
        let works = keep_edition_representatives(self.load_sweep_works()?);
        if works.len() < MIN_BUNDLE {
            return Ok(CollectionSweepResult {
                bundles: Vec::new(),
                saved_search_suggestions: Vec::new(),
            });
        }
        let by_id = works
            .iter()
            .map(|work| (work.id, work.clone()))
            .collect::<HashMap<_, _>>();

        let mut bundles = Vec::new();
        bundles.extend(self.sweep_link_components(&by_id)?);
        bundles.extend(sweep_title_families(&works));
        bundles.extend(self.sweep_series_runs(&by_id)?);
        let sequences = merge_overlapping(bundles);

        let (themes, saved_search_suggestions) = self.sweep_themes(&by_id)?;

        // 続き物を先に置く。読む順のある束は、見つかったときの価値が大きい。
        // 上限は系統ごとにかける — 数の多いほうが少ないほうを押し出さないため。
        let mut all = cap_track(sequences);
        all.extend(cap_track(themes));

        let bundles = self.replace_swept_suggestions(&all, &by_id)?;
        Ok(CollectionSweepResult {
            bundles,
            saved_search_suggestions,
        })
    }

    /// 走査の対象になる作品。告知・活動報告はここで落とす。
    fn load_sweep_works(&self) -> Result<Vec<SweepWork>, String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, source, source_id, title, author_name, COALESCE(author_id, ''),
                        cover_path, text_length, content_type,
                        COALESCE(source_created_at, downloaded_at, '')
                 FROM downloads",
            )
            .map_err(|e| format!("Failed to prepare sweep works: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    SweepWork {
                        id: row.get(0)?,
                        source: row.get(1)?,
                        source_id: row.get(2)?,
                        title: row.get(3)?,
                        author_name: row.get(4)?,
                        author_id: row.get(5)?,
                        cover_path: row.get(6)?,
                        text_length: row.get(7)?,
                        published_at: row.get(9)?,
                    },
                    row.get::<_, String>(8)?,
                ))
            })
            .map_err(|e| format!("Failed to query sweep works: {e}"))?;
        let mut out = Vec::new();
        for row in rows {
            let (work, content_type) =
                row.map_err(|e| format!("Failed to read sweep works: {e}"))?;
            if collection_rules::is_administrative_post(
                &work.title,
                &content_type,
                work.text_length,
            ) {
                continue;
            }
            out.push(work);
        }
        Ok(out)
    }

    /// 本文リンクでつながっている塊。ハブは橋にしない。
    fn sweep_link_components(
        &self,
        by_id: &HashMap<i64, SweepWork>,
    ) -> Result<Vec<SweepBundle>, String> {
        let conn = self.read_conn()?;
        let hubs = load_link_hub_ids(&conn)?;
        let mut stmt = conn
            .prepare(
                "SELECT from_download_id, to_download_id, confidence
                 FROM work_links
                 WHERE status != 'rejected'
                   AND from_download_id IS NOT NULL
                   AND to_download_id IS NOT NULL
                   AND confidence >= 0.6",
            )
            .map_err(|e| format!("Failed to prepare sweep links: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, f64>(2)?,
                ))
            })
            .map_err(|e| format!("Failed to query sweep links: {e}"))?;

        let mut adjacency: HashMap<i64, HashSet<i64>> = HashMap::new();
        for row in rows {
            let (from, to, _confidence) =
                row.map_err(|e| format!("Failed to read sweep links: {e}"))?;
            // 告知として落とした作品はここに居ない。ハブは端点にはなれるが、
            // 渡って先へ行く橋にはしない。
            if !by_id.contains_key(&from) || !by_id.contains_key(&to) {
                continue;
            }
            if hubs.contains(&from) || hubs.contains(&to) {
                continue;
            }
            adjacency.entry(from).or_default().insert(to);
            adjacency.entry(to).or_default().insert(from);
        }

        Ok(connected_components(&adjacency)
            .into_iter()
            .filter(|component| (MIN_BUNDLE..=MAX_BUNDLE).contains(&component.len()))
            .map(|ids| SweepBundle {
                kind: BundleKind::Link,
                member_evidence: vec![CollectionSuggestionEvidence {
                    kind: "content_link".to_string(),
                    label: "本文・キャプションのリンク".to_string(),
                    contribution: 0.8,
                }],
                ids,
                track: "sequence",
            })
            .collect())
    }

    /// 公式シリーズの、話数が連続している区間。
    fn sweep_series_runs(
        &self,
        by_id: &HashMap<i64, SweepWork>,
    ) -> Result<Vec<SweepBundle>, String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT series_source, series_key, title, download_id, content_order
                 FROM download_series
                 WHERE content_order IS NOT NULL
                 ORDER BY series_source, series_key, content_order",
            )
            .map_err(|e| format!("Failed to prepare series runs: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    format!("{}:{}", row.get::<_, String>(0)?, row.get::<_, String>(1)?),
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(|e| format!("Failed to query series runs: {e}"))?;

        let mut out = Vec::new();
        let mut current_key = String::new();
        let mut current_title = String::new();
        let mut run: Vec<(i64, i64)> = Vec::new();
        let flush = |title: &str, run: &mut Vec<(i64, i64)>, out: &mut Vec<SweepBundle>| {
            // 3作以上そろって初めて「連載」と呼ぶ。2作なら題名族が拾う。
            if run.len() >= 3
                && run.len() <= MAX_BUNDLE
                && !collection_rules::is_administrative_series_label(title)
            {
                out.push(SweepBundle {
                    kind: BundleKind::SeriesRun(title.to_string()),
                    member_evidence: vec![CollectionSuggestionEvidence {
                        kind: "series_run".to_string(),
                        label: format!("公式シリーズ「{title}」の連番"),
                        contribution: 0.52,
                    }],
                    ids: run.iter().map(|(id, _)| *id).collect(),
                    track: "sequence",
                });
            }
            run.clear();
        };
        for row in rows {
            let (key, title, download_id, order) =
                row.map_err(|e| format!("Failed to read series runs: {e}"))?;
            if !by_id.contains_key(&download_id) {
                continue;
            }
            let broken =
                key != current_key || run.last().is_some_and(|(_, previous)| order - previous > 1);
            if broken {
                flush(&current_title, &mut run, &mut out);
                current_key = key;
                current_title = title;
            }
            run.push((download_id, order));
        }
        flush(&current_title, &mut run, &mut out);
        Ok(out)
    }

    /// 共有タグを起点にした、味の束。
    ///
    /// タグだけでは足りない — 同じタグの作品は棚に何百とある。逆に本文の
    /// 近さだけでも足りない — 近いだけの作品は言葉で説明できない。
    /// **タグで起点を決め、本文の近さで削る**と、説明のつく束だけが残る。
    fn sweep_themes(
        &self,
        by_id: &HashMap<i64, SweepWork>,
    ) -> Result<(Vec<SweepBundle>, Vec<SavedSearchSuggestion>), String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT t.name, dt.download_id
                 FROM download_tags dt
                 JOIN tags t ON t.id = dt.tag_id
                 ORDER BY t.name, dt.download_id",
            )
            .map_err(|e| format!("Failed to prepare theme tags: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| format!("Failed to query theme tags: {e}"))?;
        let mut by_tag: HashMap<String, Vec<i64>> = HashMap::new();
        for row in rows {
            let (name, download_id) = row.map_err(|e| format!("Failed to read theme tags: {e}"))?;
            if !collection_rules::is_informative_tag(&name) || !by_id.contains_key(&download_id) {
                continue;
            }
            by_tag.entry(name).or_default().push(download_id);
        }

        // 大きすぎるタグの見きわめに、本文ベクトルは要らない。何作に付いて
        // いるかだけで決まる。意味索引が無くても、ここまでは言える。
        let mut oversized = by_tag
            .iter()
            .filter(|(_, ids)| ids.len() > THEME_TAG_MAX)
            .map(|(tag, ids)| SavedSearchSuggestion {
                tag: tag.clone(),
                work_count: ids.len() as i64,
                reason: format!(
                    "「{tag}」は{}作に付いています。読む単位にしては大きいので、束ではなく絞り込みとして持つほうが扱いやすい",
                    ids.len()
                ),
            })
            .collect::<Vec<_>>();
        oversized.sort_by(|left, right| {
            right
                .work_count
                .cmp(&left.work_count)
                .then_with(|| left.tag.cmp(&right.tag))
        });
        oversized.truncate(MAX_SAVED_SEARCH_SUGGESTIONS);

        let wanted = by_id.keys().copied().collect::<HashSet<_>>();
        let centroids =
            match crate::database::semantic_index::work_centroids(&self.storage_dir, &wanted) {
                Ok(values) => values,
                Err(error) => {
                    // 意味索引が無くても走査そのものは成り立つ。続き物だけ出す。
                    log::warn!("Theme sweep skipped, semantic index unavailable: {error}");
                    return Ok((Vec::new(), oversized));
                }
            };
        if centroids.len() < THEME_TAG_MIN {
            return Ok((Vec::new(), oversized));
        }
        // 本文ベクトルの無い作品は、近さを測れないので束の材料から外す。
        for ids in by_tag.values_mut() {
            ids.retain(|id| centroids.contains_key(id));
        }

        // 起点の順序を決めておく。同じ棚を二度走査したら同じ束が出るべきである。
        let mut tags = by_tag.into_iter().collect::<Vec<_>>();
        tags.sort_by(|left, right| {
            left.1
                .len()
                .cmp(&right.1.len())
                .then_with(|| left.0.cmp(&right.0))
        });

        // 棚のふつうの近さを先に測る。ここを測らずに絶対値でしきい値を置くと、
        // 同じ題材ばかりの棚では何も弾けない。
        let baseline = shelf_baseline(&centroids);

        let mut out = Vec::new();
        for (tag, mut ids) in tags {
            if !(THEME_TAG_MIN..=THEME_TAG_MAX).contains(&ids.len()) {
                continue;
            }
            ids.sort_unstable();
            let Some(kept) = tighten_to_baseline(&ids, &centroids, &baseline) else {
                continue;
            };
            out.push(SweepBundle {
                kind: BundleKind::Theme { tag },
                member_evidence: vec![
                    CollectionSuggestionEvidence {
                        kind: "shared_tags".to_string(),
                        label: "共有タグ".to_string(),
                        contribution: 0.5,
                    },
                    CollectionSuggestionEvidence {
                        kind: "semantic_similarity".to_string(),
                        label: "本文の内容が近い".to_string(),
                        contribution: 0.3,
                    },
                ],
                ids: kept,
                track: "theme",
            });
        }
        // 大きい束から先にまとめる。小さいものを先に置くと、あとから来た
        // 大きい束が次々に吸い寄せられて、際限なく育つ。
        out.sort_by(|left, right| {
            right
                .ids
                .len()
                .cmp(&left.ids.len())
                .then_with(|| left.ids.first().cmp(&right.ids.first()))
        });
        Ok((merge_overlapping(out), oversized))
    }

    /// 走査で作った候補を入れ替える。種から作った候補には触らない。
    fn replace_swept_suggestions(
        &self,
        bundles: &[SweepBundle],
        by_id: &HashMap<i64, SweepWork>,
    ) -> Result<Vec<CollectionSuggestion>, String> {
        let now = chrono::Utc::now().to_rfc3339();
        let rule_version = COLLECTION_SUGGEST_RULE_VERSION.to_string();
        let mut prepared = Vec::new();
        {
            let conn = self.read_conn()?;
            for bundle in bundles {
                let members = bundle
                    .ids
                    .iter()
                    .filter_map(|id| by_id.get(id))
                    .collect::<Vec<_>>();
                if members.len() < MIN_BUNDLE {
                    continue;
                }
                if bundle_is_rejected(&conn, &members)? {
                    continue;
                }
                let members = fold_composite_volumes(members);
                if members.len() < MIN_BUNDLE {
                    continue;
                }
                let ordered = order_bundle_members(&members, bundle.track);
                let ids_json = serde_json::to_string(&bundle.ids)
                    .map_err(|e| format!("Failed to encode sweep seeds: {e}"))?;
                // 根拠の文は、まとめ終わったあとの顔ぶれで数え直す。
                // 数えるのは表示名。同じ作者に別の ID が付いていることがあり、
                // 内部の鍵で数えると画面の「作者2人」が実感と合わない。
                let author_count = ordered
                    .iter()
                    .map(|work| work.author_name.as_str())
                    .collect::<HashSet<_>>()
                    .len();
                let evidence = bundle.evidence_sentence(author_count);
                let names = sweep_name_options(&conn, &ordered, &ids_json, bundle.anchor_tag())?;
                let suggestion_members = ordered
                    .iter()
                    .enumerate()
                    .map(|(position, work)| CollectionSuggestionMember {
                        source: work.source.clone(),
                        source_id: work.source_id.clone(),
                        download_id: Some(work.id),
                        title: work.title.clone(),
                        author_name: work.author_name.clone(),
                        cover_path: work.cover_path.clone(),
                        text_length: work.text_length,
                        proposed_position: position as i64,
                        score: 1.0,
                        selected: true,
                        evidence: bundle.member_evidence.clone(),
                    })
                    .collect::<Vec<_>>();
                prepared.push((bundle, ids_json, names, suggestion_members, evidence));
            }
        }

        let mut saved = Vec::new();
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Sweep transaction failed: {e}"))?;
        tx.execute(
            "DELETE FROM collection_suggestions WHERE origin = 'sweep' AND state = 'pending'",
            [],
        )
        .map_err(|e| format!("Failed to clear swept suggestions: {e}"))?;
        for (bundle, ids_json, names, members, evidence) in prepared {
            let id = new_collection_id("sweep");
            let proposed_name = names
                .first()
                .map(|value| value.name.clone())
                .unwrap_or_else(|| "読書コレクション".to_string());
            let members_json = serde_json::to_string(&members)
                .map_err(|e| format!("Failed to encode sweep members: {e}"))?;
            let names_json = serde_json::to_string(&names)
                .map_err(|e| format!("Failed to encode sweep names: {e}"))?;
            let kind = if bundle.track == "sequence" {
                "ordered"
            } else {
                "unordered"
            };
            tx.execute(
                "INSERT INTO collection_suggestions (
                    id, seed_json, proposed_name, collection_kind, members_json,
                    score, rule_version, state, created_at, updated_at,
                    name_options_json, track, origin, evidence_summary
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 1.0, ?6, 'pending', ?7, ?7,
                           ?8, ?9, 'sweep', ?10)",
                params![
                    id,
                    ids_json,
                    proposed_name,
                    kind,
                    members_json,
                    rule_version,
                    now,
                    names_json,
                    bundle.track,
                    evidence,
                ],
            )
            .map_err(|e| format!("Failed to save swept suggestion: {e}"))?;
            saved.push(CollectionSuggestion {
                id,
                proposed_name,
                name_options: names,
                collection_kind: kind.to_string(),
                track: bundle.track.to_string(),
                origin: "sweep".to_string(),
                evidence_summary: evidence,
                score: 1.0,
                rule_version: rule_version.clone(),
                state: "pending".to_string(),
                members,
                created_at: now.clone(),
                updated_at: now.clone(),
            });
        }
        tx.commit()
            .map_err(|e| format!("Sweep commit failed: {e}"))?;
        Ok(saved)
    }
}

/// 合本を、それが含む分冊の下に畳む。
///
/// 「【前編＋中編】…」は前編と後編のあいだにある別の作品ではなく、二つを
/// まとめた版である。並べたままにすると同じ本文が二度出てくる。分冊が
/// そろっているときだけ畳む — **合本しか手元に無いなら、それが本体**だからで、
/// 消してしまえば束から作品そのものが消える。
fn fold_composite_volumes(members: Vec<&SweepWork>) -> Vec<&SweepWork> {
    let singles = members
        .iter()
        .filter_map(|work| {
            collection_rules::episode_order(&work.title)
                .filter(|_| collection_rules::composite_episode_range(&work.title).is_none())
        })
        .collect::<HashSet<_>>();
    members
        .into_iter()
        .filter(|work| {
            let Some((start, end)) = collection_rules::composite_episode_range(&work.title) else {
                return true;
            };
            // 含んでいる話がすべて別に入っているときだけ落とす。
            !(start..=end).all(|order| singles.contains(&order))
        })
        .collect()
}

/// 同じ作品の別版を畳んで、代表だけを残す。
///
/// 版の呼び名を落とした鍵が同じで、作者も同じなら同一作とみなす。鍵は題名
/// 全体から作るので、同じ作者が同じ題名を二度使っていることになる。実データ
/// では 65 組 130 作がこれで、その多くは pixiv と FANBOX に同じ作品を出した
/// もの（本文の長さが違う）だった。
///
/// 代表は本文がいちばん長いもの — サンプルは導入だけのことが多い。
/// **前編と後編は鍵が違う**ので、ここで畳まれることはない。
fn keep_edition_representatives(works: Vec<SweepWork>) -> Vec<SweepWork> {
    let mut groups: HashMap<(String, String), Vec<SweepWork>> = HashMap::new();
    for work in works {
        let key = collection_rules::edition_match_key(&work.title);
        groups
            .entry((work.author_key(), key))
            .or_default()
            .push(work);
    }
    let mut out = Vec::new();
    for (_, mut group) in groups {
        group.sort_by(|left, right| {
            right
                .text_length
                .cmp(&left.text_length)
                .then_with(|| left.id.cmp(&right.id))
        });
        out.push(group.remove(0));
    }
    out.sort_by_key(|work| work.id);
    out
}

/// 同一作者・共通語幹・どれかに話数の語がある題名の並び。
fn sweep_title_families(works: &[SweepWork]) -> Vec<SweepBundle> {
    let mut families: HashMap<(String, String), Vec<&SweepWork>> = HashMap::new();
    for work in works {
        let key = collection_rules::family_match_key(&work.title);
        // 短い語幹は違う作品どうしを結びつけてしまう。
        if key.chars().count() < 9 {
            continue;
        }
        families
            .entry((work.author_key(), key.chars().take(26).collect()))
            .or_default()
            .push(work);
    }
    let mut out = families
        .into_values()
        .filter(|group| (MIN_BUNDLE..=MAX_BUNDLE).contains(&group.len()))
        // 語幹が同じだけでは足りない。どれかに話数の語が要る。同じ書き出しの
        // 独立した短編を、連載として束ねないため。
        .filter(|group| {
            group
                .iter()
                .any(|work| collection_rules::has_ordinal_marker(&work.title))
        })
        .map(|group| SweepBundle {
            kind: BundleKind::TitleRun,
            member_evidence: vec![CollectionSuggestionEvidence {
                kind: "title_similarity".to_string(),
                label: "題名の連番".to_string(),
                contribution: 0.6,
            }],
            ids: group.iter().map(|work| work.id).collect(),
            track: "sequence",
        })
        .collect::<Vec<_>>();
    // 走査のたびに同じ順で出す。HashMap の順は実行ごとに変わる。
    out.sort_by(|left, right| left.ids.first().cmp(&right.ids.first()));
    out
}

/// 棚のふつうの近さ。
///
/// 対の余弦の平均と散らばり。同じ題材ばかりの棚では平均が 0.94 まで上がり、
/// 別々の棚では別の値になる。**しきい値は棚ごとに違う**ので、固定値は置けない。
struct ShelfBaseline {
    mean: f64,
    deviation: f64,
}

impl ShelfBaseline {
    /// 棚のふつうから、いくつぶん離れているか。
    fn z(&self, value: f64) -> f64 {
        if self.deviation <= f64::EPSILON {
            return 0.0;
        }
        (value - self.mean) / self.deviation
    }
}

/// 棚から対を拾って、ふつうの近さを測る。
///
/// 乱数は使わない。同じ棚を二度走査したら同じ答えが出るべきで、走らせるたびに
/// 束の顔ぶれが揺れると「直したのか壊れたのか」が分からなくなる。互いに素な
/// 歩幅で拾えば、規則的でありながら偏らない。
fn shelf_baseline(centroids: &HashMap<i64, Vec<f32>>) -> ShelfBaseline {
    let mut ids = centroids.keys().copied().collect::<Vec<_>>();
    ids.sort_unstable();
    if ids.len() < 2 {
        return ShelfBaseline {
            mean: 0.0,
            deviation: 1.0,
        };
    }
    let count = ids.len();
    // 件数と互いに素になるまで歩幅をずらす。割り切れると同じ対ばかり拾う。
    let mut stride = (count / 3).max(1);
    while stride > 1 && gcd(stride, count) != 1 {
        stride -= 1;
    }
    let samples = BASELINE_PAIRS.min(count.saturating_mul(4));
    let mut values = Vec::with_capacity(samples);
    for step in 0..samples {
        let left = ids[step % count];
        let right = ids[(step * stride + 1) % count];
        if left == right {
            continue;
        }
        // 比べられない組は、棚のふつうを測る材料にしない。
        if let Some(value) = cosine(&centroids[&left], &centroids[&right]) {
            values.push(value);
        }
    }
    if values.is_empty() {
        return ShelfBaseline {
            mean: 0.0,
            deviation: 1.0,
        };
    }
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance = values
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f64>()
        / values.len() as f64;
    ShelfBaseline {
        mean,
        deviation: variance.sqrt(),
    }
}

fn gcd(mut left: usize, mut right: usize) -> usize {
    while right != 0 {
        let next = left % right;
        left = right;
        right = next;
    }
    left
}

/// 棚の水準を超えるまで束を締める。
///
/// いちばん浮いているメンバーを落としながら、隔たりがしきい値に届くのを待つ。
/// 3作を切ったら束として認めない。
fn tighten_to_baseline(
    ids: &[i64],
    centroids: &HashMap<i64, Vec<f32>>,
    baseline: &ShelfBaseline,
) -> Option<Vec<i64>> {
    let mut kept = ids
        .iter()
        .copied()
        .filter(|id| centroids.contains_key(id))
        .collect::<Vec<_>>();
    while kept.len() >= THEME_TAG_MIN {
        let mut averages = Vec::with_capacity(kept.len());
        let mut total = 0.0;
        for left in &kept {
            let mut sum = 0.0;
            let mut compared = 0usize;
            for right in &kept {
                if left == right {
                    continue;
                }
                if let Some(value) = cosine(&centroids[left], &centroids[right]) {
                    sum += value;
                    compared += 1;
                }
            }
            // 比べた相手の数で割る。比べられなかった組まで分母に数えると、
            // 近さが実際より低く出て、まとまるはずの束が落ちる。
            //
            // 誰とも比べられなければ 0 とする。この作品は束の近さを語れない
            // ので、次の周回で真っ先に外れる（外すのは平均が最も低いもの）。
            let average = if compared == 0 {
                0.0
            } else {
                sum / compared as f64
            };
            total += average;
            averages.push((average, *left));
        }
        let mean = total / kept.len() as f64;
        // 近さだけでなく大きさも条件にする。棚のふつうより近くても、24作を
        // 超える束は「読む単位」ではなく絞り込みの結果である。
        if baseline.z(mean) >= THEME_MIN_Z && kept.len() <= THEME_MAX_MEMBERS {
            kept.sort_unstable();
            return Some(kept);
        }
        // いちばん浮いているものを落とす。同点は id で決めて、走査を再現可能に。
        averages.sort_by(|left, right| {
            left.0
                .total_cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
        });
        let drop = averages[0].1;
        kept.retain(|id| *id != drop);
    }
    None
}

/// 二つの向きの近さ。比べられないときは `None`。
///
/// どちらも長さ1にそろえてあるので、内積がそのまま余弦になる。
///
/// **長さが違うものを比べない。** `zip` は短いほうで打ち切るので、次元の違う
/// ベクトルを渡すと**前半だけの内積**を「近さ」として返してしまう。埋め込みの
/// モデルを替えれば次元は変わるし、入れ替えの途中では新旧が混ざる。そこで
/// 静かに間違った数を返すと、無関係な作品が束にまとまる形で表に出る。
fn cosine(left: &[f32], right: &[f32]) -> Option<f64> {
    if left.is_empty() || left.len() != right.len() {
        return None;
    }
    Some(
        left.iter()
            .zip(right.iter())
            .map(|(a, b)| (*a as f64) * (*b as f64))
            .sum(),
    )
}

/// ほぼ同じ塊を見つけた束を、ひとつにまとめる。
///
/// 起点の違う規則が同じ塊を別々に見つけることがある。同じ作品を含む束が二つ
/// 並ぶと、どちらを採ればよいのか判断できない。
///
/// ただし**まとめすぎてはいけない**。作品は複数のタグを持つので、少し重なる
/// だけでまとめると、タグ起点の束が次々に接合して「フェラ」しか共通点の無い
/// 36作の塊ができる（実データで起きた）。小さいほうがほぼ丸ごと含まれている
/// ときだけ、同じ塊とみなす。
fn merge_overlapping(bundles: Vec<SweepBundle>) -> Vec<SweepBundle> {
    let mut out: Vec<SweepBundle> = Vec::new();
    for bundle in bundles {
        let ids = bundle.ids.iter().copied().collect::<HashSet<_>>();
        let mut merged = false;
        for existing in out.iter_mut() {
            let overlap = existing.ids.iter().filter(|id| ids.contains(id)).count();
            let smaller = ids.len().min(existing.ids.len());
            if smaller == 0 || (overlap * 100) < smaller * MERGE_OVERLAP_PERCENT {
                continue;
            }
            let union = existing
                .ids
                .iter()
                .copied()
                .chain(bundle.ids.iter().copied())
                .collect::<HashSet<_>>();
            // まとめた結果が上限を超えるなら、まとめない。片方を捨てるより、
            // 二つの束として残しておくほうが、利用者に選ぶ余地がある。
            if union.len() > MAX_BUNDLE {
                continue;
            }
            let mut ids = union.into_iter().collect::<Vec<_>>();
            ids.sort_unstable();
            existing.ids = ids;
            for evidence in bundle.member_evidence.iter() {
                if !existing
                    .member_evidence
                    .iter()
                    .any(|value| value.kind == evidence.kind)
                {
                    existing.member_evidence.push(evidence.clone());
                }
            }
            merged = true;
            break;
        }
        if !merged {
            out.push(bundle);
        }
    }
    out
}

/// 束の並び。続き物は話数、無ければ投稿日。テーマは投稿日だけ。
fn order_bundle_members<'a>(members: &[&'a SweepWork], track: &str) -> Vec<&'a SweepWork> {
    let mut ordered = members.to_vec();
    if track == "sequence" {
        ordered.sort_by(|left, right| {
            let a = collection_rules::episode_order(&left.title);
            let b = collection_rules::episode_order(&right.title);
            match (a, b) {
                (Some(a), Some(b)) => a.cmp(&b),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            }
            .then_with(|| left.published_at.cmp(&right.published_at))
            .then_with(|| left.id.cmp(&right.id))
        });
    } else {
        ordered.sort_by(|left, right| {
            left.published_at
                .cmp(&right.published_at)
                .then_with(|| left.id.cmp(&right.id))
        });
    }
    ordered
}

/// この組合せを、利用者がすでに「二度と出さない」と言っていないか。
fn bundle_is_rejected(conn: &Connection, members: &[&SweepWork]) -> Result<bool, String> {
    for (index, left) in members.iter().enumerate() {
        for right in members.iter().skip(index + 1) {
            let (a, b) = canonical_work_key_pair(
                &WorkKey {
                    source: left.source.clone(),
                    source_id: left.source_id.clone(),
                },
                &WorkKey {
                    source: right.source.clone(),
                    source_id: right.source_id.clone(),
                },
            );
            let rejected = conn
                .query_row(
                    "SELECT 1 FROM collection_pair_feedback
                     WHERE left_source = ?1 AND left_source_id = ?2
                       AND right_source = ?3 AND right_source_id = ?4
                       AND decision = 'reject' AND rule_version = ?5",
                    params![
                        a.source,
                        a.source_id,
                        b.source,
                        b.source_id,
                        COLLECTION_SUGGEST_RULE_VERSION
                    ],
                    |_| Ok(()),
                )
                .optional()
                .map_err(|e| format!("Failed to read sweep feedback: {e}"))?
                .is_some();
            if rejected {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// 走査で見つけた束の名前の案。
fn sweep_name_options(
    conn: &Connection,
    members: &[&SweepWork],
    ids_json: &str,
    anchor_tag: Option<&str>,
) -> Result<Vec<CollectionNameCandidate>, String> {
    // 名前の作り方は種からの提案と同じにする。出どころが違うだけで、
    // 束としての性質は変わらない。
    let ranked = members
        .iter()
        .map(|work| RankedSuggestionMember {
            work: SuggestionWork {
                id: work.id,
                source: work.source.clone(),
                source_id: work.source_id.clone(),
                title: work.title.clone(),
                author_name: work.author_name.clone(),
                author_id: work.author_id.clone(),
                cover_path: work.cover_path.clone(),
                text_length: work.text_length,
                published_at: work.published_at.clone(),
                content_type: String::new(),
            },
            member_score: 1.0,
            title_stem: String::new(),
            default_selected: true,
            evidence: Vec::new(),
            series_order: None,
            link_order: None,
            link_depth: None,
            episode_order: collection_rules::episode_order(&work.title),
        })
        .collect::<Vec<_>>();
    let mut options = collection_name_options(conn, &ranked, ids_json)?;
    // テーマの束は、束ねた理由がタグそのものである。共有タグを先頭に置かないと、
    // たまたま並びの先頭に来た1作の題名が束全体の名前になる。
    if anchor_tag.is_some() {
        if let Some(position) = options.iter().position(|option| option.source == "tags") {
            let tags = options.remove(position);
            options.insert(0, tags);
        } else if let Some(tag) = anchor_tag {
            options.insert(
                0,
                CollectionNameCandidate {
                    source: "tags".to_string(),
                    label: "共有タグ".to_string(),
                    name: tag.to_string(),
                },
            );
        }
    }
    Ok(options)
}

/// 大きい束から順に、系統ごとの上限まで残す。
fn cap_track(mut bundles: Vec<SweepBundle>) -> Vec<SweepBundle> {
    bundles.sort_by(|left, right| {
        right
            .ids
            .len()
            .cmp(&left.ids.len())
            .then_with(|| left.ids.first().cmp(&right.ids.first()))
    });
    bundles.truncate(MAX_SWEEP_PER_TRACK);
    bundles
}

/// 無向グラフの連結成分。
fn connected_components(adjacency: &HashMap<i64, HashSet<i64>>) -> Vec<Vec<i64>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    // 走査のたびに同じ順で出す。
    let mut nodes = adjacency.keys().copied().collect::<Vec<_>>();
    nodes.sort_unstable();
    for node in nodes {
        if !seen.insert(node) {
            continue;
        }
        let mut stack = vec![node];
        let mut component = vec![node];
        while let Some(current) = stack.pop() {
            for neighbour in adjacency.get(&current).into_iter().flatten() {
                if seen.insert(*neighbour) {
                    component.push(*neighbour);
                    stack.push(*neighbour);
                }
            }
        }
        component.sort_unstable();
        out.push(component);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn centroids(entries: &[(i64, &[f32])]) -> HashMap<i64, Vec<f32>> {
        entries
            .iter()
            .map(|(id, vector)| (*id, vector.to_vec()))
            .collect()
    }

    /// 長さの違う向きは比べない。
    ///
    /// `zip` は短いほうで打ち切るので、放っておくと**前半だけの内積**が
    /// 「近さ」として返る。埋め込みのモデルを替えれば次元は変わるので、
    /// 入れ替えの途中には新旧が並ぶ。
    #[test]
    fn vectors_of_different_lengths_are_not_comparable() {
        assert_eq!(cosine(&[1.0, 0.0, 0.0], &[1.0, 0.0, 0.0]), Some(1.0));
        assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]), Some(0.0));
        assert_eq!(cosine(&[1.0, 0.0, 0.0], &[1.0, 0.0]), None);
        assert_eq!(cosine(&[], &[]), None);
    }

    /// 次元の合わない作品は、束に**入れない**。
    ///
    /// 前半だけの内積を返していたころ、`[1,0]` は `[1,0,0]` と「完全に同じ」に
    /// 見えた。無関係な作品が束の中に混ざる形で表に出る。
    #[test]
    fn a_work_indexed_by_another_model_is_left_out_of_the_bundle() {
        let centroids = centroids(&[
            (1, &[1.0, 0.0, 0.0]),
            (2, &[1.0, 0.0, 0.0]),
            (3, &[1.0, 0.0, 0.0]),
            // 次元が違う。前半だけを見ると、上の3件と見分けが付かない。
            (4, &[1.0, 0.0]),
        ]);
        let baseline = ShelfBaseline {
            mean: 0.0,
            deviation: 0.4,
        };

        let kept = tighten_to_baseline(&[1, 2, 3, 4], &centroids, &baseline)
            .expect("そろっている3件で束になる");

        assert_eq!(kept, vec![1, 2, 3], "次元の違う4番が残っている");
    }

    /// 棚のふつうを測る材料にも、比べられない組は数えない。
    #[test]
    fn the_shelf_baseline_ignores_pairs_it_cannot_compare() {
        let mixed = shelf_baseline(&centroids(&[
            (1, &[1.0, 0.0, 0.0]),
            (2, &[1.0, 0.0, 0.0]),
            (3, &[1.0, 0.0]),
        ]));
        let clean = shelf_baseline(&centroids(&[(1, &[1.0, 0.0, 0.0]), (2, &[1.0, 0.0, 0.0])]));
        assert!(
            (mixed.mean - clean.mean).abs() < 1e-9,
            "比べられない組が平均を動かしている: {} と {}",
            mixed.mean,
            clean.mean
        );
    }

    /// 歩幅をずらすための互いに素の判定。ここが狂うと同じ対ばかり拾う。
    #[test]
    fn gcd_finds_the_common_divisor() {
        assert_eq!(gcd(12, 18), 6);
        assert_eq!(gcd(7, 13), 1);
        assert_eq!(gcd(5, 0), 5);
        assert_eq!(gcd(0, 5), 5);
    }

    /// つながっているものをひとまとまりにする。
    #[test]
    fn connected_components_groups_what_is_linked() {
        let mut adjacency: HashMap<i64, HashSet<i64>> = HashMap::new();
        adjacency.entry(1).or_default().insert(2);
        adjacency.entry(2).or_default().insert(1);
        adjacency.entry(2).or_default().insert(3);
        adjacency.entry(3).or_default().insert(2);
        adjacency.entry(9).or_default().insert(10);
        adjacency.entry(10).or_default().insert(9);

        let mut groups = connected_components(&adjacency);
        groups.sort();

        assert_eq!(groups, vec![vec![1, 2, 3], vec![9, 10]]);
    }
}
