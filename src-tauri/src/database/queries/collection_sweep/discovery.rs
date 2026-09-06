//! 続き物の提案を、公式の入れ物・本文の包含・リンクの向きで確認する。
use super::*;
use unicode_normalization::UnicodeNormalization;

#[derive(Default)]
pub(super) struct DiscoveryContext {
    pub details: HashMap<(Vec<i64>, i64), Vec<CollectionSuggestionEvidence>>,
    pub edges: Vec<(i64, i64)>,
}

fn evidence(kind: &str, label: String) -> CollectionSuggestionEvidence {
    CollectionSuggestionEvidence {
        kind: kind.into(),
        label,
        contribution: 0.0,
    }
}

pub(super) fn contextualize(
    db: &Database,
    bundles: &mut Vec<SweepBundle>,
    works: &HashMap<i64, SweepWork>,
    aliases: &HashMap<i64, i64>,
) -> Result<DiscoveryContext, String> {
    let mut context = DiscoveryContext::default();
    let mut order_conflicts = Vec::new();
    let mut link_details: HashMap<(i64, i64), CollectionSuggestionEvidence> = HashMap::new();
    let mut membership: HashMap<i64, HashSet<String>> = HashMap::new();
    let mut series: HashMap<String, (String, HashSet<i64>)> = HashMap::new();
    {
        let conn = db.read_conn()?;
        let mut stmt = conn
            .prepare("SELECT download_id,series_source,series_key,title FROM download_series")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    format!("{}:{}", r.get::<_, String>(1)?, r.get::<_, String>(2)?),
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (original, key, title) = row.map_err(|e| e.to_string())?;
            let id = aliases.get(&original).copied().unwrap_or(original);
            membership.entry(id).or_default().insert(key.clone());
            series
                .entry(key)
                .or_insert_with(|| (title, HashSet::new()))
                .1
                .insert(id);
        }
        let mut stmt = conn.prepare("SELECT from_download_id,to_download_id,relation_type,context_text FROM work_links WHERE status != 'rejected' AND from_download_id IS NOT NULL AND to_download_id IS NOT NULL AND confidence >= 0.6 AND relation_type IN ('continues_from','continues_to')").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (from, to, relation, quote) = row.map_err(|e| e.to_string())?;
            if ambiguous_link_context(quote.as_deref().unwrap_or_default()) {
                continue;
            }
            let from = aliases.get(&from).copied().unwrap_or(from);
            let to = aliases.get(&to).copied().unwrap_or(to);
            if from == to || !works.contains_key(&from) || !works.contains_key(&to) {
                continue;
            }
            if same_episode(&works[&from], &works[&to]) {
                continue;
            }
            let relation = super::super::work_link_evidence::classify_link_context(
                quote.as_deref().unwrap_or_default(),
            )
            .map(|(kind, _)| kind)
            .unwrap_or(&relation);
            let (before, after) = if relation == "continues_from" {
                (to, from)
            } else {
                (from, to)
            };
            let left = collection_rules::parse_sequence_title(&works[&before].title);
            let right = collection_rules::parse_sequence_title(&works[&after].title);
            if left.key == right.key
                && !left.order_path.is_empty()
                && !right.order_path.is_empty()
                && left.order_path > right.order_path
            {
                order_conflicts.push((before, after));
                continue;
            }
            context.edges.push((before, after));
            link_details.insert(
                (before, after),
                evidence(
                    "sequence_link",
                    format!(
                        "前の作品：{}{}",
                        works[&before].title,
                        quote
                            .filter(|q| !q.is_empty())
                            .map(|q| format!(" — {}", truncate_chars(&q, 160)))
                            .unwrap_or_default()
                    ),
                ),
            );
        }
    }
    let parsed = works
        .iter()
        .map(|(&id, w)| (id, collection_rules::parse_sequence_title(&w.title)))
        .collect::<HashMap<_, _>>();
    let mut body_cache: HashMap<i64, String> = HashMap::new();
    for bundle in bundles.iter_mut() {
        let mut details_by_id: HashMap<i64, Vec<CollectionSuggestionEvidence>> = HashMap::new();
        // 種類を問わず全件比較するのではなく、版・合本が紛れやすい対に本文検査を限定する。
        let mut remove = HashSet::new();
        for i in 0..bundle.ids.len() {
            for j in (i + 1)..bundle.ids.len() {
                let (a, b) = (bundle.ids[i], bundle.ids[j]);
                let (Some(left), Some(right)) = (works.get(&a), works.get(&b)) else {
                    continue;
                };
                let (pa, pb) = (&parsed[&a], &parsed[&b]);
                if left.source == right.source
                    && !pa.is_composite
                    && !pb.is_composite
                    && !(pa.key == pb.key && (pa.order_path.is_empty() || pb.order_path.is_empty()))
                {
                    continue;
                }
                for id in [a, b] {
                    body_cache.entry(id).or_insert_with(|| {
                        db.get_reader_document(id, None)
                            .ok()
                            .map(|doc| {
                                doc.plain_text
                                    .nfkc()
                                    .filter(|c| !c.is_whitespace())
                                    .collect()
                            })
                            .unwrap_or_default()
                    });
                }
                let (left_body, right_body) = (&body_cache[&a], &body_cache[&b]);
                let ab = containment(left_body, right_body);
                let ba = containment(right_body, left_body);
                if ab.max(ba) < 0.85 {
                    continue;
                }
                let (larger, smaller) = if left_body.len() >= right_body.len() {
                    (a, b)
                } else {
                    (b, a)
                };
                let drop = if parsed[&larger].order_path.is_empty() || parsed[&larger].is_composite
                {
                    larger
                } else {
                    smaller
                };
                remove.insert(drop);
                let keep = if drop == a { b } else { a };
                details_by_id.entry(keep).or_default().push(evidence(
                    "edition_overlap",
                    format!(
                        "「{}」と本文が重なるため、重複して読む並びから外しました",
                        works[&drop].title
                    ),
                ));
            }
        }
        bundle.ids.retain(|id| !remove.contains(id));
        let common = bundle
            .ids
            .first()
            .and_then(|id| membership.get(id))
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|key| {
                bundle
                    .ids
                    .iter()
                    .all(|id| membership.get(id).is_some_and(|s| s.contains(key)))
            })
            .collect::<Vec<_>>();
        // 管理用のシリーズでも普通の名前でも、内側の小さな前後編を拾う。
        let already_organized = common.iter().any(|key| {
            let (title, members) = &series[key];
            !collection_rules::is_administrative_series_label(title)
                && members.len() == bundle.ids.len()
                && bundle.ids.iter().all(|id| members.contains(id))
        });
        if already_organized {
            bundle.ids.clear();
            continue;
        }
        let has_cross_source = bundle
            .ids
            .iter()
            .filter_map(|id| works.get(id))
            .map(|w| &w.source)
            .collect::<HashSet<_>>()
            .len()
            > 1;
        let numbered = bundle
            .ids
            .iter()
            .filter(|id| {
                parsed[id].order_path.len() > 1
                    || parsed[id].ordinal_label.as_ref().is_some_and(|label| {
                        ![
                            "前編", "中編", "後編", "前篇", "中篇", "後篇", "上", "中", "下",
                        ]
                        .contains(&label.as_str())
                    })
            })
            .filter_map(|id| parsed[id].order_path.first().copied())
            .collect::<std::collections::BTreeSet<_>>();
        let gap = numbered
            .iter()
            .zip(numbered.iter().skip(1))
            .any(|(a, b)| *b - *a > 1 && *b < 98);
        let members = bundle
            .ids
            .iter()
            .filter_map(|id| works.get(id))
            .collect::<Vec<_>>();
        let cyclic = has_cycle(&members, &context.edges);
        let contrary = order_conflicts
            .iter()
            .any(|(a, b)| bundle.ids.contains(a) && bundle.ids.contains(b));
        if matches!(bundle.kind, BundleKind::SeriesRun(_)) {
            bundle.strength = 0.65;
        }
        for &id in &bundle.ids {
            let details = details_by_id.entry(id).or_default();
            for (&(before, after), detail) in &link_details {
                if after == id && bundle.ids.contains(&before) {
                    details.push(detail.clone());
                }
            }
            if let Some(label) = &parsed[&id].ordinal_label {
                details.push(evidence("episode_order", format!("題名の順序：{label}")));
            } else {
                details.push(evidence(
                    "order_unconfirmed",
                    "題名に話数がありません。前後の根拠を確認してください".into(),
                ));
            }
            for key in &common {
                let (title, members) = &series[key];
                details.push(evidence(
                    "series_subset",
                    format!("「{title}」{}作品の中の続き物", members.len()),
                ));
            }
            if !membership.contains_key(&id) {
                details.push(evidence(
                    "unregistered",
                    "公式シリーズに登録されていない作品".into(),
                ));
            }
            if has_cross_source {
                details.push(evidence("cross_source","pixiv・FANBOXなど複数の取得元にまたがる候補。作者と前後のつながりを確認してください".into()));
                if body_cache
                    .get(&id)
                    .is_none_or(|body| body.chars().count() < 1000)
                {
                    details.push(evidence(
                        "body_unconfirmed",
                        "保存本文が短いか読み込めず、別版との重複を十分に確認できません".into(),
                    ));
                }
            }
            if gap {
                details.push(evidence(
                    "sequence_gap",
                    "話数が飛んでいます。未収録の作品や前後編の表記を確認してください".into(),
                ));
            }
            if contrary {
                details.push(evidence("order_conflict","リンクの向きと話数が一致しません。仮の話数順なので、前後の本文を確認してください".into()));
            }
            if cyclic {
                details.push(evidence(
                    "order_conflict",
                    "前後リンクが循環しています。仮の題名順なので、読む順を確認してください".into(),
                ));
            }
        }
        for (id, mut details) in details_by_id {
            details.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.label.cmp(&b.label)));
            details.dedup_by(|a, b| a.kind == b.kind && a.label == b.label);
            context.details.insert((bundle.ids.clone(), id), details);
        }
    }
    bundles.retain(|b| b.ids.len() >= MIN_BUNDLE);
    Ok(context)
}

/// 完全一致の長い断片を複数地点で照合する。短い引用や未保存本文は版の証拠にしない。
fn containment(left: &str, right: &str) -> f64 {
    let chars = left.chars().collect::<Vec<_>>();
    if chars.len() < 1_000 || right.chars().count() < 1_000 {
        return 0.0;
    }
    let stride = (chars.len() / 128).max(64);
    let samples = (0..chars.len().saturating_sub(64))
        .step_by(stride)
        .map(|i| chars[i..i + 64].iter().collect::<String>())
        .collect::<Vec<_>>();
    if samples.is_empty() {
        return 0.0;
    }
    samples
        .iter()
        .filter(|sample| right.contains(sample.as_str()))
        .count() as f64
        / samples.len() as f64
}

pub(super) fn ambiguous_link_context(text: &str) -> bool {
    super::super::work_link_evidence::classify_link_context(text)
        .is_some_and(|(kind, _)| !matches!(kind, "continues_from" | "continues_to"))
}

pub(super) fn same_episode(left: &SweepWork, right: &SweepWork) -> bool {
    let a = collection_rules::parse_sequence_title(&left.title);
    let b = collection_rules::parse_sequence_title(&right.title);
    !a.key.is_empty() && a.key == b.key && a.order_path == b.order_path
}

fn has_cycle(members: &[&SweepWork], edges: &[(i64, i64)]) -> bool {
    let mut pending = members.iter().map(|w| w.id).collect::<HashSet<_>>();
    while !pending.is_empty() {
        let next = pending
            .iter()
            .find(|id| !edges.iter().any(|(a, b)| b == *id && pending.contains(a)))
            .copied();
        if let Some(id) = next {
            pending.remove(&id);
        } else {
            return true;
        }
    }
    false
}

pub(super) fn ordered<'a>(members: &[&'a SweepWork], edges: &[(i64, i64)]) -> Vec<&'a SweepWork> {
    let fallback = order_bundle_members(members, "sequence");
    let mut result = Vec::new();
    let mut pending = fallback;
    while !pending.is_empty() {
        let index = pending.iter().position(|work| {
            !edges
                .iter()
                .any(|(a, b)| *b == work.id && pending.iter().any(|w| w.id == *a))
        });
        let Some(index) = index else {
            // 循環時に独断で一辺を消さず、未確定の題名順を残して確認してもらう。
            result.extend(pending);
            break;
        };
        result.push(pending.remove(index));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn containment_is_directional_and_short_quotes_do_not_count() {
        let part = (0..1200)
            .map(|i| format!("星{i}の記録"))
            .collect::<String>();
        let other = (0..1500)
            .map(|i| format!("別{i}の風景"))
            .collect::<String>();
        let full = format!("{other}{part}");
        assert!(containment(&part, &full) > 0.99);
        assert!(containment(&full, &part) < 0.6);
        assert_eq!(containment("短い引用", &full), 0.0);
    }
}
