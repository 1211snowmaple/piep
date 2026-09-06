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

mod discovery;

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
pub(super) const THEME_MIN_Z: f64 = 2.0;
/// 棚の水準を測るために取る対の数。
const BASELINE_PAIRS: usize = 20_000;
/// テーマの束に入れる上限。
///
/// 40作の「まとまり」は、まとまりではなく棚である。読む単位として持てる
/// 大きさに収まるまで締める。
const THEME_MAX_MEMBERS: usize = 24;
/// 候補は安定した順序で最大200件。画面では一覧から一件ずつ確認する。
const MAX_SWEEP_PER_TRACK: usize = 200;
/// 弱い候補で件数を埋めない。
pub(super) const MIN_STRENGTH: f64 = 0.55;

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
    /// 表示名は候補検索の鍵であり、本人確認の証拠ではない。呼び出し側は
    /// 題名も照合し、同じ取得元に異なる作者IDがある組合せは統合しない。
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

/// 題材の束を探した結果。
///
/// 束と、束にしなかったタグと、**そもそも探しきれなかった事情**の三つを返す。
/// 三つ目を落とすと、意味索引が読めないことと題材の束が無いことが、画面では
/// 同じ「見つかりませんでした」になる。
struct ThemeSweep {
    bundles: Vec<SweepBundle>,
    saved_searches: Vec<SavedSearchSuggestion>,
    /// 探しきれなかった理由。最後まで探せたなら `None`。
    note: Option<String>,
}

impl ThemeSweep {
    /// 意味索引に届かなかったとき。続き物だけは出せるので、失敗にはしない。
    fn skipped(saved_searches: Vec<SavedSearchSuggestion>, note: &str) -> Self {
        Self {
            bundles: Vec::new(),
            saved_searches,
            note: Some(note.to_string()),
        }
    }
}

/// 洗い出した束ひとつ。保存する前の、まだ提案でしかない状態。
struct SweepBundle {
    ids: Vec<i64>,
    track: &'static str,
    kind: BundleKind,
    /// 束ごとの証拠。メンバー全員に同じものが付く。
    member_evidence: Vec<CollectionSuggestionEvidence>,
    /// この束を出すに値するか、0.0〜1.0 で。
    ///
    /// **大きさは確度ではない。** これまでは大きい束から順に200件を採っていた
    /// が、それは「いちばん自信のある束」ではなく「いちばん大きい束」を選ぶ
    /// 規則である。24作のテーマ束より、本文リンクでつながった3作のほうが
    /// ずっと確かなことは日常的に起きる。
    ///
    /// 見つけ方ごとに意味の違う数（リンクの確信度、話数の揃い、棚の水準からの
    /// 隔たり）を、ここで一本の尺度へそろえる。
    strength: f64,
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
    /// 保存データを走査し、同じ規則・顔ぶれには同じ候補IDを割り当てる。
    /// 採否の記録と、作品から作った候補は維持する。
    pub fn sweep_collection_candidates(&self) -> Result<CollectionSweepResult, String> {
        // 別版は代表だけを残す。残さないと「【おまけ付き】【モモ編】…」と
        // 「【モモ編】…」が別の作品として同じ束に二度並ぶ。
        let (works, aliases) = keep_edition_representatives(self.load_sweep_works()?);
        if works.len() < MIN_BUNDLE {
            return Ok(CollectionSweepResult {
                bundles: Vec::new(),
                saved_search_suggestions: Vec::new(),
                semantic_used: false,
                note: Some("棚に作品が足りないので、まとまりを探せません。".to_string()),
            });
        }
        let by_id = works
            .iter()
            .map(|work| (work.id, work.clone()))
            .collect::<HashMap<_, _>>();

        let mut bundles = Vec::new();
        bundles.extend(self.sweep_link_components(&by_id, &aliases)?);
        bundles.extend(sweep_title_families(&works));
        bundles.extend(self.sweep_series_runs(&by_id)?);
        let mut sequences = deduplicate_sequences(bundles);
        let context = discovery::contextualize(self, &mut sequences, &by_id, &aliases)?;

        sequences = deduplicate_sequences(sequences);
        let mut themes = self.sweep_themes(&by_id)?;

        // すでに利用者が作ったコレクションを、走査の「新しい発見」として
        // もう一度出してはいけない。選抜したあとで落とすと、その重複が
        // 枠を一つ使い、まだ見ていない束を押し出すため、系統ごとの選抜より前に
        // 除く。順序付きコレクションでも、ここで知りたいのは顔ぶれの一致である。
        let (existing, rejected_pairs) = {
            let conn = self.read_conn()?;
            (
                load_existing_collection_member_sets(&conn)?,
                load_rejected_pairs(&conn)?,
            )
        };
        let allowed = |bundle: &SweepBundle| {
            let members = bundle
                .ids
                .iter()
                .filter_map(|id| by_id.get(id))
                .collect::<Vec<_>>();
            !bundle_is_rejected(&rejected_pairs, &members).unwrap_or(true)
        };
        sequences.retain(allowed);
        themes.bundles.retain(allowed);
        sequences.retain(|bundle| !bundle_matches_existing_collection(bundle, &by_id, &existing));
        themes
            .bundles
            .retain(|bundle| !bundle_matches_existing_collection(bundle, &by_id, &existing));

        // 続き物を先に置く。読む順のある束は、見つかったときの価値が大きい。
        // 上限は系統ごとにかける — 数の多いほうが少ないほうを押し出さないため。
        // シリーズ未登録の作品を含む発見から確認できるよう、優先枠を先に取る。
        let (unregistered, within_series): (Vec<_>, Vec<_>) =
            sequences.into_iter().partition(|bundle| {
                bundle.ids.iter().any(|id| {
                    context
                        .details
                        .get(&(bundle.ids.clone(), *id))
                        .is_some_and(|items| items.iter().any(|e| e.kind == "unregistered"))
                })
            });
        let within_series = select_track(within_series);
        let unregistered = select_track(unregistered);
        let reserved = within_series.len().min(MAX_SWEEP_PER_TRACK / 4);
        let mut all = unregistered
            .into_iter()
            .take(MAX_SWEEP_PER_TRACK - reserved)
            .collect::<Vec<_>>();
        let remaining = MAX_SWEEP_PER_TRACK.saturating_sub(all.len());
        all.extend(within_series.into_iter().take(remaining));
        all.extend(select_track(themes.bundles));

        let bundles = self.replace_swept_suggestions(&all, &by_id, &context)?;
        Ok(CollectionSweepResult {
            bundles,
            saved_search_suggestions: themes.saved_searches,
            semantic_used: themes.note.is_none(),
            note: themes.note,
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
        aliases: &HashMap<i64, i64>,
    ) -> Result<Vec<SweepBundle>, String> {
        let conn = self.read_conn()?;
        let hubs = load_link_hub_ids(&conn)?;
        let mut stmt = conn
            .prepare(
                "SELECT from_download_id, to_download_id, confidence, context_text
                 FROM work_links
                 WHERE status != 'rejected'
                   AND from_download_id IS NOT NULL
                   AND to_download_id IS NOT NULL
                   AND confidence >= 0.6
                   AND relation_type IN ('continues_from', 'continues_to')",
            )
            .map_err(|e| format!("Failed to prepare sweep links: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|e| format!("Failed to query sweep links: {e}"))?;

        let mut adjacency: HashMap<i64, HashSet<i64>> = HashMap::new();
        // 辺の確信度は捨てない。「本文リンクでつながっている」の一言で束ねて
        // いたが、0.61 でつながった塊と 0.98 でつながった塊は別物である。
        // 鍵は小さいほうの id を先に置いて、向きの違いで二重に数えない。
        let mut edge_confidence: HashMap<(i64, i64), f64> = HashMap::new();
        for row in rows {
            let (from, to, confidence, quote) =
                row.map_err(|e| format!("Failed to read sweep links: {e}"))?;
            if discovery::ambiguous_link_context(quote.as_deref().unwrap_or_default()) {
                continue;
            }
            let from = aliases.get(&from).copied().unwrap_or(from);
            let to = aliases.get(&to).copied().unwrap_or(to);
            if from == to {
                continue;
            }
            // 告知として落とした作品はここに居ない。ハブは端点にはなれるが、
            // 渡って先へ行く橋にはしない。
            if !by_id.contains_key(&from) || !by_id.contains_key(&to) {
                continue;
            }
            if discovery::same_episode(&by_id[&from], &by_id[&to]) {
                continue;
            }
            if hubs.contains(&from) || hubs.contains(&to) {
                continue;
            }
            adjacency.entry(from).or_default().insert(to);
            adjacency.entry(to).or_default().insert(from);
            let key = if from <= to { (from, to) } else { (to, from) };
            // 同じ二作に両向きの行があるなら、強いほうを採る。
            edge_confidence
                .entry(key)
                .and_modify(|value| *value = value.max(confidence))
                .or_insert(confidence);
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
                strength: link_component_strength(&ids, &edge_confidence),
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
                    // 取得元が「これは同じシリーズの何話目である」と言っている
                    // うえ、話数が途切れずに並んでいる。棚の中でいちばん確かな
                    // 根拠なので、推測を重ねずここは高く置く。
                    strength: 0.95,
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
    fn sweep_themes(&self, by_id: &HashMap<i64, SweepWork>) -> Result<ThemeSweep, String> {
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
        let centroids = match crate::database::semantic_index::work_centroids(
            &self.storage_dir,
            &wanted,
        ) {
            Ok(values) => values,
            Err(error) => {
                // 意味索引が無くても走査そのものは成り立つ。続き物だけ出す。
                // ただし**黙って**続き物だけにはしない。索引が壊れている
                // ことと、題材の束が本当に無いことは別の話である。
                log::warn!("Theme sweep skipped, semantic index unavailable: {error}");
                return Ok(ThemeSweep::skipped(
                        oversized,
                        "意味索引が読めないので、題材の束は探せませんでした。続き物だけを出しています。",
                    ));
            }
        };
        if centroids.len() < THEME_TAG_MIN {
            return Ok(ThemeSweep::skipped(
                oversized,
                "本文ベクトルがまだ足りないので、題材の束は探せませんでした。続き物だけを出しています。",
            ));
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
            let Some((kept, z)) = tighten_to_baseline(&ids, &centroids, &baseline) else {
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
                strength: theme_strength(z),
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

        // まとめたあとに、もう一度締める。
        //
        // `merge_overlapping` の上限は MAX_BUNDLE(40) で、テーマの上限
        // THEME_MAX_MEMBERS(24) ではない。だから重なりの大きい二つのテーマ束を
        // まとめると、**24作までと決めたはずの束が40作になって出てくる**。
        // しかも近さを測り直さないので、根拠の一行は「本文も近い40作です」と
        // 言う — その40作でそれを確かめたことは一度も無いのに。
        //
        // 締め直せば、大きさも隔たりも最後の顔ぶれで測った値になる。ここで
        // 落ちる束は、まとめる前は通っていても、まとまった姿では束と呼べない
        // ものである。
        let mut themes = Vec::new();
        for mut bundle in merge_overlapping(out) {
            let Some((kept, z)) = tighten_to_baseline(&bundle.ids, &centroids, &baseline) else {
                continue;
            };
            bundle.ids = kept;
            bundle.strength = theme_strength(z);
            themes.push(bundle);
        }
        Ok(ThemeSweep {
            bundles: themes,
            saved_searches: oversized,
            note: None,
        })
    }

    /// 走査で作った候補を入れ替える。種から作った候補には触らない。
    fn replace_swept_suggestions(
        &self,
        bundles: &[SweepBundle],
        by_id: &HashMap<i64, SweepWork>,
        context: &discovery::DiscoveryContext,
    ) -> Result<Vec<CollectionSuggestion>, String> {
        let now = chrono::Utc::now().to_rfc3339();
        let rule_version = COLLECTION_SUGGEST_RULE_VERSION.to_string();
        let mut prepared = Vec::new();
        {
            let conn = self.read_conn()?;
            let rejected_pairs = load_rejected_pairs(&conn)?;
            for bundle in bundles {
                let members = bundle
                    .ids
                    .iter()
                    .filter_map(|id| by_id.get(id))
                    .collect::<Vec<_>>();
                if members.len() < MIN_BUNDLE {
                    continue;
                }
                if bundle_is_rejected(&rejected_pairs, &members)? {
                    continue;
                }
                let members = if bundle.track == "sequence" {
                    members
                } else {
                    fold_composite_volumes(members)
                };
                if members.len() < MIN_BUNDLE {
                    continue;
                }
                let ordered = if bundle.track == "sequence" {
                    discovery::ordered(&members, &context.edges)
                } else {
                    order_bundle_members(&members, bundle.track)
                };
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
                        evidence: bundle
                            .member_evidence
                            .iter()
                            .cloned()
                            .chain(
                                (bundle.track == "sequence")
                                    .then(|| context.details.get(&(bundle.ids.clone(), work.id)))
                                    .flatten()
                                    .into_iter()
                                    .flatten()
                                    .cloned(),
                            )
                            .collect(),
                    })
                    .collect::<Vec<_>>();
                prepared.push((bundle, ids_json, names, suggestion_members, evidence));
            }
        }

        let mut saved = Vec::new();
        let mut saved_ids = HashSet::new();
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Sweep transaction failed: {e}"))?;
        let previous_names = {
            let mut stmt=tx.prepare("SELECT id,name_options_json FROM collection_suggestions WHERE origin='sweep' AND state='pending'").map_err(|e|e.to_string())?;
            let collected = stmt
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<HashMap<_, _>, _>>()
                .map_err(|e| e.to_string())?;
            collected
        };
        tx.execute(
            "DELETE FROM collection_suggestions WHERE origin = 'sweep' AND state = 'pending'",
            [],
        )
        .map_err(|e| format!("Failed to clear swept suggestions: {e}"))?;
        for (bundle, ids_json, mut names, members, evidence) in prepared {
            use sha2::{Digest, Sha256};
            let mut stable_keys = members
                .iter()
                .map(|m| (&m.source, &m.source_id))
                .collect::<Vec<_>>();
            stable_keys.sort();
            let identity = serde_json::to_vec(&(&rule_version, bundle.track, stable_keys))
                .map_err(|e| e.to_string())?;
            let digest = Sha256::digest(identity);
            let id = format!(
                "sweep-{}",
                digest
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            );
            if !saved_ids.insert(id.clone()) {
                continue;
            }
            let previous_state = tx
                .query_row(
                    "SELECT state FROM collection_suggestions WHERE id=?1",
                    params![id],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if previous_state.is_some_and(|state| state != "pending") {
                continue;
            }
            if let Some(previous) = previous_names.get(&id) {
                let previous: Vec<CollectionNameCandidate> =
                    serde_json::from_str(previous).map_err(|e| e.to_string())?;
                for option in previous.into_iter().filter(|option| option.source == "llm") {
                    if !names.iter().any(|name| name.name == option.name) {
                        names.push(option);
                    }
                }
            }
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
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8, ?8,
                           ?9, ?10, 'sweep', ?11)",
                params![
                    id,
                    ids_json,
                    proposed_name,
                    kind,
                    members_json,
                    bundle.strength,
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
                score: bundle.strength,
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

/// 既存コレクションの顔ぶれを、並び順に依存しない形で一度に読む。
///
/// `download_id` は作品を削除すると NULL になる一時的な解決結果なので、正本の
/// `source + source_id` を使う。欠けた作品を含む既存コレクションは、現在の棚から
/// 作った候補とは完全一致しないため、誤って同一視されない。
fn load_existing_collection_member_sets(
    conn: &Connection,
) -> Result<HashSet<Vec<(String, String)>>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT collection_id, source, source_id
             FROM work_collection_members
             ORDER BY collection_id",
        )
        .map_err(|e| format!("Failed to prepare existing collection members: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| format!("Failed to query existing collection members: {e}"))?;
    let mut by_collection: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for row in rows {
        let (collection_id, source, source_id) =
            row.map_err(|e| format!("Failed to read existing collection members: {e}"))?;
        by_collection
            .entry(collection_id)
            .or_default()
            .push((source, source_id));
    }
    Ok(by_collection
        .into_values()
        .map(canonical_member_set)
        .filter(|members| members.len() >= MIN_BUNDLE)
        .collect())
}

fn bundle_matches_existing_collection(
    bundle: &SweepBundle,
    by_id: &HashMap<i64, SweepWork>,
    existing: &HashSet<Vec<(String, String)>>,
) -> bool {
    let members = bundle
        .ids
        .iter()
        .filter_map(|id| by_id.get(id))
        .collect::<Vec<_>>();
    let visible_members = if bundle.track == "sequence" {
        members
    } else {
        fold_composite_volumes(members)
    };
    let keys = visible_members
        .into_iter()
        .map(|work| (work.source.clone(), work.source_id.clone()))
        .collect();
    existing.contains(&canonical_member_set(keys))
}

fn canonical_member_set(mut members: Vec<(String, String)>) -> Vec<(String, String)> {
    members.sort_unstable();
    members.dedup();
    members
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
fn keep_edition_representatives(works: Vec<SweepWork>) -> (Vec<SweepWork>, HashMap<i64, i64>) {
    let mut groups: HashMap<(String, String), Vec<SweepWork>> = HashMap::new();
    for work in works {
        let key = collection_rules::edition_match_key(&work.title);
        groups
            .entry((work.author_key(), key))
            .or_default()
            .push(work);
    }
    let mut out = Vec::new();
    let mut aliases = HashMap::new();
    for (_, mut group) in groups {
        let mut author_ids: HashMap<&str, HashSet<&str>> = HashMap::new();
        for work in &group {
            author_ids
                .entry(&work.source)
                .or_default()
                .insert(&work.author_id);
        }
        if author_ids.values().any(|ids| ids.len() > 1) {
            for work in group {
                aliases.insert(work.id, work.id);
                out.push(work);
            }
            continue;
        }
        group.sort_by(|left, right| {
            right
                .text_length
                .cmp(&left.text_length)
                .then_with(|| left.id.cmp(&right.id))
        });
        for work in &group {
            aliases.insert(work.id, group[0].id);
        }
        out.push(group.remove(0));
    }
    out.sort_by_key(|work| work.id);
    (out, aliases)
}

/// 同一作者・共通語幹・どれかに話数の語がある題名の並び。
fn sweep_title_families(works: &[SweepWork]) -> Vec<SweepBundle> {
    let parsed = works
        .iter()
        .map(|w| (w.id, collection_rules::parse_sequence_title(&w.title)))
        .collect::<HashMap<_, _>>();
    let mut families: HashMap<(String, String), Vec<&SweepWork>> = HashMap::new();
    for work in works {
        let title = &parsed[&work.id];
        if title.key.chars().count() < 2 || title.is_composite {
            continue;
        }
        families
            .entry((work.author_key(), title.key.clone()))
            .or_default()
            .push(work);
    }
    let mut result = Vec::new();
    let mut groups = Vec::new();
    for group in families.into_values() {
        let mut chapters: HashMap<Vec<i64>, Vec<&SweepWork>> = HashMap::new();
        for work in &group {
            let path = &parsed[&work.id].order_path;
            if path.len() > 1 {
                chapters
                    .entry(path[..path.len() - 1].to_vec())
                    .or_default()
                    .push(*work);
            }
        }
        groups.extend(
            chapters
                .into_values()
                .filter(|chapter| chapter.len() < group.len()),
        );
        groups.push(group);
    }
    for group in groups {
        if !(MIN_BUNDLE..=MAX_BUNDLE).contains(&group.len()) {
            continue;
        }
        // 同一サービスでIDが異なる同名作者を、表示名だけで結ばない。
        let mut authors: HashMap<&str, HashSet<&str>> = HashMap::new();
        for work in &group {
            authors
                .entry(&work.source)
                .or_default()
                .insert(&work.author_id);
        }
        if authors.values().any(|ids| ids.len() > 1) {
            continue;
        }
        let numbered = group
            .iter()
            .filter(|w| !parsed[&w.id].order_path.is_empty())
            .collect::<Vec<_>>();
        if numbered.is_empty() {
            continue;
        }
        let distinct = numbered
            .iter()
            .map(|w| parsed[&w.id].order_path.clone())
            .collect::<HashSet<_>>();
        if distinct.len() < 2
            && !(numbered.len() == 1
                && group.len() == 2
                && parsed[&numbered[0].id]
                    .order_path
                    .first()
                    .is_some_and(|n| *n == 2 || *n == 3))
        {
            continue;
        }
        // 同じ話数の別作品・別版を一つの連載へ混ぜない。本文リンク経路は別に残る。
        if distinct.len() != numbered.len() {
            continue;
        }
        let ratio = numbered.len() as f64 / group.len() as f64;
        result.push(SweepBundle {
            ids: group.iter().map(|w| w.id).collect(),
            track: "sequence",
            kind: BundleKind::TitleRun,
            member_evidence: vec![CollectionSuggestionEvidence {
                kind: "title_similarity".into(),
                label: "本題が一致し、話数・前後編が異なります".into(),
                contribution: 0.6,
            }],
            strength: (0.70 + 0.20 * ratio).min(0.95),
        });
    }
    result.sort_by_key(|b| b.ids[0]);
    result
}

/// 内側の前後編を大きな連載へ吸収しない。顔ぶれが同一の経路だけ統合する。
fn deduplicate_sequences(bundles: Vec<SweepBundle>) -> Vec<SweepBundle> {
    let mut unique: std::collections::BTreeMap<Vec<i64>, SweepBundle> =
        std::collections::BTreeMap::new();
    for mut bundle in bundles {
        bundle.ids.sort_unstable();
        bundle.ids.dedup();
        if let Some(existing) = unique.get_mut(&bundle.ids) {
            if bundle.strength > existing.strength {
                existing.strength = bundle.strength;
                existing.kind = bundle.kind;
            }
            existing.member_evidence.extend(bundle.member_evidence);
        } else {
            unique.insert(bundle.ids.clone(), bundle);
        }
    }
    unique.into_values().collect()
}

/// 本文リンクの塊の確からしさ。
///
/// 塊の中にある辺の確信度の平均。リンク抽出が 0.6 を下限にしているので、
/// ここへ来る値は 0.6〜1.0 に収まる。それをそのまま尺度に使う — 「つながって
/// いる」という事実より「どれだけ確かにつながっているか」で並べたい。
///
/// 辺が一本も見つからないことは起こらない（塊は辺から作られる）が、その場合は
/// 下限を返す。数え損ねを高い確度として通さない。
fn link_component_strength(ids: &[i64], edges: &HashMap<(i64, i64), f64>) -> f64 {
    let members = ids.iter().copied().collect::<HashSet<_>>();
    let mut total = 0.0;
    let mut count = 0usize;
    for ((left, right), confidence) in edges {
        if members.contains(left) && members.contains(right) {
            total += *confidence;
            count += 1;
        }
    }
    if count == 0 {
        return 0.6;
    }
    (total / count as f64).clamp(0.0, 1.0)
}

/// テーマ束の確からしさ。
///
/// 棚の水準からの隔たり（z）を 0.55〜1.0 へ写す。z = 2.0 は束と認める下限
/// なので、そこがちょうど下限の確度になる。z = 6.0 で振り切る — それ以上の
/// 隔たりは実測でもめったに出ず、区別しても意味がない。
pub(super) fn theme_strength(z: f64) -> f64 {
    let span = ((z - THEME_MIN_Z) / 4.0).clamp(0.0, 1.0);
    MIN_STRENGTH + (1.0 - MIN_STRENGTH) * span
}

/// 棚のふつうの近さ。
///
/// 対の余弦の平均と散らばり。同じ題材ばかりの棚では平均が 0.94 まで上がり、
/// 別々の棚では別の値になる。**しきい値は棚ごとに違う**ので、固定値は置けない。
pub(super) struct ShelfBaseline {
    mean: f64,
    deviation: f64,
}

impl ShelfBaseline {
    /// 棚のふつうから、いくつぶん離れているか。
    pub(super) fn z(&self, value: f64) -> f64 {
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
pub(super) fn shelf_baseline(centroids: &HashMap<i64, Vec<f32>>) -> ShelfBaseline {
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
        let index = step % count;
        let round = step / count;
        let left = ids[index];
        // 周回ごとに相手を一つずらす。ずらさないと `(step * stride + 1) % count`
        // は step が count 増えても同じ値へ戻るので、2周目以降は1周目と
        // **まったく同じ対**を数え直すだけになる。実際、3,938作の棚で
        // 15,752回まわして拾えていた対は 3,938 種類しかなかった。
        let right = ids[(index * stride + 1 + round) % count];
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
) -> Option<(Vec<i64>, f64)> {
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
        let z = baseline.z(mean);
        if z >= THEME_MIN_Z && kept.len() <= THEME_MAX_MEMBERS {
            kept.sort_unstable();
            // 隔たりも返す。締め終えた**その顔ぶれ**で測った値なので、
            // あとから件数を削られても食い違わない。捨てると、束の確度を
            // 二度と測り直せなくなる。
            return Some((kept, z));
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
pub(super) fn cosine(left: &[f32], right: &[f32]) -> Option<f64> {
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
            // まとめた束の確度は、強いほうを引き継ぐ。二つの根拠が同じ顔ぶれを
            // 指しているのだから、弱いほうに引きずられる理由は無い。
            existing.strength = existing.strength.max(bundle.strength);
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
            let a = collection_rules::parse_sequence_title(&left.title).order_path;
            let b = collection_rules::parse_sequence_title(&right.title).order_path;
            a.cmp(&b)
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

/// 「二度と出さない」と言われた組を、いまの規則版ぶんだけ一度に読む。
///
/// 組ごとに問い合わせていた。40作の束なら 780 回、走査ぜんぶで数十万回の
/// 往復になる。表は利用者が押した回数ぶんしか行が無いので、丸ごと持てる。
type RejectedPairs = HashSet<(String, String, String, String)>;

fn load_rejected_pairs(conn: &Connection) -> Result<RejectedPairs, String> {
    let mut stmt = conn
        .prepare(
            "SELECT left_source, left_source_id, right_source, right_source_id
               FROM collection_pair_feedback
              WHERE decision = 'reject' AND rule_version = ?1",
        )
        .map_err(|e| format!("Failed to prepare sweep feedback: {e}"))?;
    let rows = stmt
        .query_map(params![COLLECTION_SUGGEST_RULE_VERSION], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|e| format!("Failed to query sweep feedback: {e}"))?;
    let mut out = HashSet::new();
    for row in rows {
        out.insert(row.map_err(|e| format!("Failed to read sweep feedback: {e}"))?);
    }
    Ok(out)
}

/// この組合せを、利用者がすでに「二度と出さない」と言っていないか。
fn bundle_is_rejected(rejected: &RejectedPairs, members: &[&SweepWork]) -> Result<bool, String> {
    if rejected.is_empty() {
        return Ok(false);
    }
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
            if rejected.contains(&(a.source, a.source_id, b.source, b.source_id)) {
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
    if anchor_tag.is_none() {
        let titles = members
            .iter()
            .map(|work| collection_rules::parse_sequence_title(&work.title))
            .collect::<Vec<_>>();
        if let Some(first) = titles
            .first()
            .filter(|first| !first.key.is_empty() && titles.iter().all(|t| t.key == first.key))
        {
            let shared_parent = first.order_path.len() > 1
                && titles.iter().all(|title| {
                    title.order_path.len() == first.order_path.len()
                        && title.order_path[..title.order_path.len() - 1]
                            == first.order_path[..first.order_path.len() - 1]
                });
            let name = if shared_parent {
                let parent = first
                    .ordinal_label
                    .as_deref()
                    .unwrap_or("")
                    .split(" · ")
                    .take(first.order_path.len() - 1)
                    .collect::<Vec<_>>()
                    .join(" · ");
                format!("{} {parent}", first.display_stem)
            } else {
                first.display_stem.clone()
            };
            let name = collection_rules::clamp_name(&name, COLLECTION_NAME_MAX_CHARS);
            options.retain(|option| option.name != name);
            options.insert(
                0,
                CollectionNameCandidate {
                    source: "title".into(),
                    label: "話数を除いた本題".into(),
                    name,
                    ..Default::default()
                },
            );
        } else if options
            .first()
            .is_some_and(|option| option.source == "author")
        {
            // 作者だけの名前が並ぶと候補を見分けられない。共通題名がなくても、
            // 読む順の先頭作品を示す仮名から確認と命名を始められるようにする。
            if let Some(first) = members.first().filter(|work| !work.title.trim().is_empty()) {
                let name = collection_rules::clamp_name(
                    &format!("「{}」からの連作", first.title.trim()),
                    COLLECTION_NAME_MAX_CHARS,
                );
                options.insert(
                    0,
                    CollectionNameCandidate {
                        source: "title".into(),
                        label: "先頭作品からの仮の名前".into(),
                        name,
                        ..Default::default()
                    },
                );
            }
        }
    }

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
                    ..Default::default()
                },
            );
        }
    }
    Ok(options)
}

/// 確認する順序を固定する。同じ棚で再探索しても候補を抽選し直さない。
fn select_track(bundles: Vec<SweepBundle>) -> Vec<SweepBundle> {
    let mut eligible = bundles
        .into_iter()
        .filter(|b| b.strength >= MIN_STRENGTH)
        .collect::<Vec<_>>();
    eligible.sort_by(|a, b| {
        b.strength
            .total_cmp(&a.strength)
            .then_with(|| a.ids.cmp(&b.ids))
    });
    eligible.truncate(MAX_SWEEP_PER_TRACK);
    eligible
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

    #[test]
    fn existing_collection_comparison_uses_the_members_actually_shown() {
        let works = ["星の舟 前編", "星の舟 中編", "【前編＋中編】星の舟"]
            .into_iter()
            .enumerate()
            .map(|(index, title)| {
                let id = index as i64;
                (
                    id,
                    SweepWork {
                        id,
                        source: "pixiv".into(),
                        source_id: id.to_string(),
                        title: title.into(),
                        author_name: "架空作者".into(),
                        author_id: "author".into(),
                        cover_path: None,
                        text_length: 3000,
                        published_at: String::new(),
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        let existing = HashSet::from([vec![
            ("pixiv".into(), "0".into()),
            ("pixiv".into(), "1".into()),
        ]]);
        let mut candidate = bundle(0, 3, 0.9);
        assert!(bundle_matches_existing_collection(
            &candidate, &works, &existing
        ));
        // 本文検査で重複を確定できなかった続き物は、表示する3作で比較する。
        candidate.track = "sequence";
        assert!(!bundle_matches_existing_collection(
            &candidate, &works, &existing
        ));
    }

    fn centroids(entries: &[(i64, &[f32])]) -> HashMap<i64, Vec<f32>> {
        entries
            .iter()
            .map(|(id, vector)| (*id, vector.to_vec()))
            .collect()
    }

    fn bundle(first_id: i64, size: usize, strength: f64) -> SweepBundle {
        SweepBundle {
            ids: (first_id..first_id + size as i64).collect(),
            track: "theme",
            kind: BundleKind::Theme {
                tag: "タグ".to_string(),
            },
            member_evidence: Vec::new(),
            strength,
        }
    }

    /// 出す束は、大きい順ではなく確かな順に選ぶ。
    ///
    /// 以前は `ids.len()` の降順に200件を採っていた。共有タグで24作が集まった
    /// ゆるい束が、本文リンクでつながった3作より先に採られる規則だった。
    #[test]
    fn selection_prefers_the_confident_bundle_over_the_big_one() {
        let picked = select_track(vec![
            bundle(1, 24, 0.58),
            bundle(100, 3, 0.97),
            bundle(200, 18, 0.60),
        ]);
        assert_eq!(picked.len(), 3, "下限を越えた束は全部残る");
        assert_eq!(
            picked.first().unwrap().ids.len(),
            3,
            "いちばん確かな3作の束が先頭に来ていない"
        );
    }

    /// 下限を下回る束は、上位に他が無くても出さない。
    ///
    /// 上限まで埋めるために、根拠の弱い候補を足さない。
    #[test]
    fn a_shelf_with_only_weak_bundles_yields_nothing() {
        let picked = select_track(vec![bundle(1, 10, 0.30), bundle(50, 6, 0.51)]);
        assert!(picked.is_empty(), "{}件出ている", picked.len());
    }

    /// 上限を超えたら根拠の強さで選び、同点の順序も固定する。
    #[test]
    fn selection_caps_the_count_and_orders_by_confidence() {
        let mut bundles = vec![bundle(0, 4, 0.99)];
        for index in 1..230 {
            bundles.push(bundle(index * 10, 4, 0.60));
        }
        let picked = select_track(bundles);
        assert_eq!(picked.len(), MAX_SWEEP_PER_TRACK);
        assert_eq!(picked.first().unwrap().strength, 0.99);
        for pair in picked.windows(2) {
            assert!(pair[0].strength >= pair[1].strength);
        }
    }

    /// 同じ棚を二度走査したら同じ答えになる。
    #[test]
    fn a_small_pool_is_deterministic() {
        let make = || vec![bundle(1, 3, 0.9), bundle(10, 5, 0.7)];
        let first = select_track(make());
        let second = select_track(make());
        let ids = |bundles: &[SweepBundle]| {
            bundles
                .iter()
                .map(|value| value.ids.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&first), ids(&second));
    }

    /// 籤を引く回でも、上限より多い候補から選び直せば顔ぶれは変わりうる。
    ///
    /// 確度が同じ束ばかりを並べて、20回引いて一度も違いが出なければ、
    /// 乱れが効いていない。
    #[test]
    fn repeated_selection_keeps_all_candidates_in_stable_order() {
        let make = || {
            (0..240)
                .map(|i| bundle(i * 10, 4, 0.80))
                .collect::<Vec<_>>()
        };
        let ids = |values: Vec<SweepBundle>| values.into_iter().map(|b| b.ids).collect::<Vec<_>>();
        assert_eq!(ids(select_track(make())), ids(select_track(make())));
        assert_eq!(select_track(make()).len(), MAX_SWEEP_PER_TRACK);
    }

    /// テーマ束の確度は、棚の水準からの隔たりで決まる。
    #[test]
    fn theme_confidence_starts_at_the_bar_and_saturates() {
        assert!((theme_strength(THEME_MIN_Z) - MIN_STRENGTH).abs() < 1e-9);
        assert!(theme_strength(4.0) > theme_strength(2.5));
        assert!(theme_strength(99.0) <= 1.0);
        // 下限に届かない隔たりでも、確度が下限を割り込むことはない
        // （そこへ来る前に `tighten_to_baseline` が束を認めない）。
        assert!(theme_strength(0.0) >= MIN_STRENGTH);
    }

    /// 本文リンクの塊は、辺の確信度の平均で測る。
    #[test]
    fn a_weakly_linked_component_scores_below_a_strongly_linked_one() {
        let mut weak = HashMap::new();
        weak.insert((1, 2), 0.61);
        weak.insert((2, 3), 0.62);
        let mut strong = HashMap::new();
        strong.insert((1, 2), 0.98);
        strong.insert((2, 3), 0.99);
        assert!(
            link_component_strength(&[1, 2, 3], &weak)
                < link_component_strength(&[1, 2, 3], &strong)
        );
        // 塊の外の辺は数えない。
        let mut mixed = strong.clone();
        mixed.insert((9, 10), 0.10);
        assert_eq!(
            link_component_strength(&[1, 2, 3], &mixed),
            link_component_strength(&[1, 2, 3], &strong)
        );
    }

    /// 棚の水準を測る対は、周回ごとに違う相手を見る。
    ///
    /// `(step * stride + 1) % count` は step が count 増えても同じ値へ戻る。
    /// ずらさないと、4周まわしても拾える対は count 種類しか無い。
    #[test]
    fn the_baseline_samples_more_pairs_than_it_has_works() {
        // 同一ベクトルばかりだと散らばりが 0 になるので、少しずつ向きを変える。
        let entries = (0..40)
            .map(|index| {
                let angle = index as f32 * 0.1;
                (index, vec![angle.cos(), angle.sin()])
            })
            .collect::<Vec<_>>();
        let centroids = entries
            .iter()
            .map(|(id, vector)| (*id, vector.clone()))
            .collect::<HashMap<_, _>>();
        let baseline = shelf_baseline(&centroids);
        // 対がすべて同一なら散らばりは 0 になる。周回ごとに相手が変われば、
        // 別の近さが混ざるので 0 にはならない。
        assert!(baseline.deviation > 0.0, "{}", baseline.deviation);
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

        let (kept, z) = tighten_to_baseline(&[1, 2, 3, 4], &centroids, &baseline)
            .expect("そろっている3件で束になる");

        assert_eq!(kept, vec![1, 2, 3], "次元の違う4番が残っている");
        // 隔たりも返る。これが束の確度になるので、捨てずに持ち帰る。
        assert!(z >= THEME_MIN_Z, "{z}");
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
