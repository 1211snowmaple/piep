#!/usr/bin/env python3
"""リンクの読み取り専用集計。ライブラリの本文やID一覧を出力しない。

告知・ハブ除外は現行相当にするが、版の代表選択、題名族との統合、採否の反映、
8件の選択は含まない。本番実装ではなく辺の選び方を比較する実験である。
以下の合成例は架空で、実データを含まない。
"""

from __future__ import annotations

import argparse
from collections import Counter, defaultdict
from datetime import datetime, timezone
import json
from pathlib import Path
import re
import sqlite3
import unicodedata


ADMIN = re.compile("お知らせ|告知|活動報告|活動記録|雑談|近況|予定|スケジュール|プラン|支援サイト|ご挨拶|アンケート|募集|目次|公開方針|進捗|展望|ご報告|作家様紹介|まとめ記事")
NEXT = ("続き", "次話", "次編", "後編", "next")
PREV = ("前話", "前編", "前作", "previous", "prev")
SUPPLEMENT = ("補足", "番外", "おまけ", "関連")
URL = re.compile(r"https?://[^\s<>\"'\\\[\]{}]+", re.I)
SEPARATOR = re.compile(r"[\n\r。；;|]")


def administrative(title: str, content_type: str, text_length: int) -> bool:
    return bool(ADMIN.search(unicodedata.normalize("NFKC", title))) or (
        content_type == "article" and text_length < 1200
    )


def normalize(text: str) -> str:
    text = unicodedata.normalize("NFKC", text).lower()
    return "".join(chr(ord(c) - 0x60) if "ァ" <= c <= "ヶ" else c for c in text)


def marker_flags(text: str) -> tuple[bool, bool, bool]:
    text = normalize(text)
    return tuple(any(marker in text for marker in group) for group in (NEXT, PREV, SUPPLEMENT))


def current_relation(text: str) -> str:
    following, previous, supplement = marker_flags(text)
    if following:
        return "continues_to"
    if previous:
        return "continues_from"
    return "supplement" if supplement else "mentions"


def cautious_relation(text: str) -> str:
    following, previous, supplement = marker_flags(text)
    if following and previous:
        return "abstain"
    if following:
        return "continues_to"
    if previous:
        return "continues_from"
    return "supplement" if supplement else "mentions"


def local_context(text: str, matches: list[re.Match], index: int) -> str:
    """URL直前を優先するプレーンテキストの試作。HTMLアンカー解析ではない。

    直前のURLと文・行の境界で区切り、方向ラベルがなければ直後の文を試す。
    選んだ文の中で方向が競合すれば棄権する。
    """
    match = matches[index]
    previous_end = matches[index - 1].end() if index else 0
    prefix = SEPARATOR.split(text[previous_end:match.start()])[-1][-100:]
    if any(marker_flags(prefix)):
        return prefix
    next_start = matches[index + 1].start() if index + 1 < len(matches) else len(text)
    suffix = SEPARATOR.split(text[match.end():next_start])[0][:100]
    return suffix


SYNTHETIC = [
    {"name": "separate_lines_prev_next", "text": "前編 https://example.test/1\n後編 https://example.test/2", "expected": ["continues_from", "continues_to"]},
    {"name": "same_line_prev_next", "text": "前話 https://example.test/1 次話 https://example.test/2", "expected": ["continues_from", "continues_to"]},
    {"name": "mixed_one_label", "text": "前編と後編はこちら https://example.test/1", "expected": ["abstain"]},
    {"name": "reference_then_next", "text": "参考資料 https://example.test/1\n続き https://example.test/2", "expected": ["mentions", "continues_to"]},
    {"name": "previous_then_supplement", "text": "前話 https://example.test/1\n番外編 https://example.test/2", "expected": ["continues_from", "supplement"]},
    {"name": "unmarked_continuation", "text": "https://example.test/1", "expected": ["continues_to"], "note": "True continuation is intentionally not inferable from this text; a recall counterexample."},
    {"name": "url_then_label", "text": "https://example.test/1 前話", "expected": ["continues_from"]},
    {"name": "english_separate_lines", "text": "previous https://example.test/1\nnext https://example.test/2", "expected": ["continues_from", "continues_to"]},
    {"name": "story_word_without_relation", "text": "前作のテーマを論じる参考資料 https://example.test/1", "expected": ["mentions"], "note": "Keyword spotting still confuses narrative discussion with a link relation."},
]


def synthetic_probe() -> dict:
    rows = []
    for case in SYNTHETIC:
        matches = list(URL.finditer(case["text"]))
        predicted = {"current_window": [], "window_conflict_abstention": [], "url_local": []}
        for index, match in enumerate(matches):
            context = case["text"][max(0, match.start() - 100):match.end() + 100]
            predicted["current_window"].append(current_relation(context))
            predicted["window_conflict_abstention"].append(cautious_relation(context))
            predicted["url_local"].append(cautious_relation(local_context(case["text"], matches, index)))
        rows.append({**case, **predicted})
    metrics = {}
    for method in ("current_window", "window_conflict_abstention", "url_local"):
        paired = [(expected, got) for row in rows for expected, got in zip(row["expected"], row[method], strict=True)]
        metrics[method] = {
            "labels": len(paired),
            "correct_including_expected_abstention": sum(a == b for a, b in paired),
            "wrong_asserted_direction": sum(b.startswith("continues_") and a != b for a, b in paired),
            "abstentions": sum(b == "abstain" for _, b in paired),
            "missed_true_direction": sum(a.startswith("continues_") and a != b for a, b in paired),
        }
    return {"metrics": metrics, "cases": rows, "interpretation": "Hand-crafted diagnostic fixtures, not a representative accuracy benchmark. The unmarked and story-word cases intentionally expose remaining failures."}


def components(edges: set[tuple[int, int]]) -> list[set[int]]:
    adjacency = defaultdict(set)
    for a, b in edges:
        adjacency[a].add(b)
        adjacency[b].add(a)
    seen = set()
    result = []
    for first in sorted(adjacency):
        if first in seen:
            continue
        component = {first}
        seen.add(first)
        stack = [first]
        while stack:
            for node in adjacency[stack.pop()] - seen:
                component.add(node)
                seen.add(node)
                stack.append(node)
        result.append(component)
    return result


def graph_stats(rows: list[dict], eligible: set[int], no_series: set[int]) -> tuple[dict, list[set[int]]]:
    edges = {tuple(sorted((r["from_download_id"], r["to_download_id"]))) for r in rows if r["from_download_id"] in eligible and r["to_download_id"] in eligible and r["from_download_id"] != r["to_download_id"]}
    groups = components(edges)
    active = set().union(*groups) if groups else set()
    candidates = [group for group in groups if 2 <= len(group) <= 40]
    candidate_nodes = set().union(*candidates) if candidates else set()
    metrics = {
        "distinct_undirected_edges": len(edges),
        "linked_nodes": len(active),
        "components": len(groups),
        "size_histogram": dict(sorted(Counter(len(group) for group in groups).items())),
        "largest_sizes": sorted((len(group) for group in groups), reverse=True)[:10],
        "components_over_40": sum(len(group) > 40 for group in groups),
        "nodes_in_components_over_40": sum(len(group) for group in groups if len(group) > 40),
        "candidate_components_2_to_40": len(candidates),
        "no_series_linked_nodes": len(active & no_series),
        "candidate_components_all_no_series": sum(group <= no_series for group in candidates),
        "candidate_components_with_no_series": sum(bool(group & no_series) for group in candidates),
        "candidate_no_series_nodes": len(candidate_nodes & no_series),
        "eligible_no_series_nodes_without_edge": len((eligible & no_series) - active),
    }
    return metrics, groups


def compare_groups(before: list[set[int]], after: list[set[int]], no_series: set[int]) -> dict:
    after_index = {node: i for i, group in enumerate(after) for node in group}
    before_nodes = set().union(*before) if before else set()
    after_nodes = set(after_index)
    split = partial_loss = vanished = giant_to_candidates = 0
    for group in before:
        child_groups = {after_index[node] for node in group if node in after_index}
        split += len(child_groups) > 1
        partial_loss += bool(child_groups) and bool(group - after_nodes)
        vanished += not child_groups
        if len(group) > 40:
            giant_to_candidates += sum(2 <= len(after[i]) <= 40 for i in child_groups)
    return {
        "before_components_split_into_multiple": split,
        "before_components_partially_lose_nodes": partial_loss,
        "before_components_disappear": vanished,
        "nodes_losing_all_edges": len(before_nodes - after_nodes),
        "no_series_nodes_losing_all_edges": len((before_nodes - after_nodes) & no_series),
        "new_size_eligible_components_from_before_giants": giant_to_candidates,
        "interpretation": "Changes in graph coverage, not verified precision/recall; existing relation labels may be wrong.",
    }


def library_probe(db: Path) -> dict:
    connection = sqlite3.connect(db.resolve().as_uri() + "?mode=ro", uri=True)
    connection.row_factory = sqlite3.Row
    connection.execute("PRAGMA query_only=ON")
    works = {r["id"]: dict(r) for r in connection.execute("SELECT id, source, title, content_type, text_length FROM downloads")}
    series_rows = list(connection.execute("SELECT download_id,series_source,series_key,title FROM download_series"))
    series_ids = {r[0] for r in series_rows}
    rows = [dict(r) for r in connection.execute("SELECT from_download_id, to_download_id, relation_type, evidence_type, status, confidence, context_text FROM work_links")]
    connection.close()
    all_ids = set(works)
    no_series = all_ids - series_ids
    non_rejected = [r for r in rows if r["status"] != "rejected"]
    resolved = [r for r in non_rejected if r["from_download_id"] in all_ids and r["to_download_id"] in all_ids]
    confidence_rows = [r for r in resolved if r["confidence"] >= 0.6]
    degree = Counter()
    for row in non_rejected:
        for field in ("from_download_id", "to_download_id"):
            if row[field] is not None:
                degree[row[field]] += 1
    administrative_ids = {i for i, work in works.items() if administrative(work["title"], work["content_type"], work["text_length"])}
    hubs = {i for i, work in works.items() if degree[i] >= 5 and (i in administrative_ids or work["text_length"] < 4000)}
    stages = {"raw_resolved": all_ids, "administrative_and_hub_filtered": all_ids - administrative_ids - hubs}
    relations = sorted({r["relation_type"] for r in rows})
    relation_counts = {}
    for relation in relations:
        relation_counts[relation] = {
            "all_rows": sum(r["relation_type"] == relation for r in rows),
            "resolved_non_rejected_rows": sum(r["relation_type"] == relation for r in resolved),
            "resolved_confidence_at_least_0_6_rows": sum(r["relation_type"] == relation for r in confidence_rows),
            "resolved_both_no_series_rows": sum(r["relation_type"] == relation and r["from_download_id"] in no_series and r["to_download_id"] in no_series for r in confidence_rows),
            "resolved_cross_source_rows": sum(r["relation_type"] == relation and works[r["from_download_id"]]["source"] != works[r["to_download_id"]]["source"] for r in confidence_rows),
        }
    variants = {
        "current_relation_agnostic": confidence_rows,
        "exclude_mentions": [r for r in confidence_rows if r["relation_type"] != "mentions"],
        "continues_only": [r for r in confidence_rows if r["relation_type"] in ("continues_from", "continues_to")],
    }
    graph_results = {}
    for name, eligible in stages.items():
        stats_and_groups = {variant: graph_stats(variant_rows, eligible, no_series) for variant, variant_rows in variants.items()}
        graph_results[name] = {variant: item[0] for variant, item in stats_and_groups.items()}
        graph_results[name]["comparisons_to_current"] = {
            variant: compare_groups(stats_and_groups["current_relation_agnostic"][1], item[1], no_series)
            for variant, item in stats_and_groups.items() if variant != "current_relation_agnostic"
        }
    ambiguous = [r for r in resolved if all(marker_flags(r["context_text"] or "")[:2])]
    context_metrics = {
        "resolved_rows_with_context": sum(bool(r["context_text"]) for r in resolved),
        "resolved_contexts_with_both_prev_and_next_markers": len(ambiguous),
        "ambiguous_contexts_by_stored_relation": dict(Counter(r["relation_type"] for r in ambiguous)),
        "interpretation": "Marker coexistence is an ambiguity signal, not a proven classification error. Stored truncated context cannot reconstruct exact URL-local text.",
    }
    series_members, series_titles = defaultdict(set), {}
    for did, source, sid, title in series_rows:
        series_members[source,sid].add(did)
        series_titles[source,sid] = title
    inner = Counter()
    for series_key, members in series_members.items():
        eligible = members & stages["administrative_and_hub_filtered"]
        stats, groups = graph_stats(variants["continues_only"], eligible, no_series)
        small_groups = [g for g in groups if 2 <= len(g) < len(members)]
        if not small_groups:
            continue
        label = "administrative_label" if re.search(r"有償依頼|リクエスト|支援|サンプル|依頼|投げ銭|お礼|寄稿|まとめ|その他",series_titles[series_key]) else "other_label"
        inner[label+"_series_with_proper_subcomponents"] += 1
        inner[label+"_proper_subcomponents"] += len(small_groups)
        if len(small_groups) >= 2:
            inner[label+"_series_with_multiple_subcomponents"] += 1
    return {
        "works": len(works), "works_without_any_official_series_row": len(no_series),
        "all_link_rows": len(rows), "non_rejected_rows": len(non_rejected),
        "resolved_non_rejected_rows": len(resolved),
        "unresolved_non_rejected_rows": len(non_rejected) - len(resolved),
        "administrative_excluded_works": len(administrative_ids), "hub_excluded_works": len(hubs),
        "relation_counts": relation_counts, "graphs": graph_results, "context_ambiguity": context_metrics,
        "within_official_series": dict(inner),
        "scope": [
            "No mutation, title/body/ID list export, or external requests. Output contains aggregate library data and invented synthetic examples only.",
            "The no-series definition is absence of ANY download_series row, including administrative series labels.",
            "Filtered stage applies current administrative-post and incident-row degree>=5 thin-text<4000 hub rules, but not edition folding, collection feedback, overlap merging, or candidate sampling.",
            "No direction-based reordering is applied: this isolates edge filtering's effect on undirected connectivity.",
            "continues_only can discard genuine unmarked continuations and useful supplements. Old continues labels can also reflect adjacent-link contamination.",
            "No human-verified truth set: graph-size reduction is not demonstrated quality improvement.",
        ],
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = {"generated_at": datetime.now(timezone.utc).isoformat(), "library": library_probe(args.db), "synthetic": synthetic_probe()}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"output": str(args.output), "works": result["library"]["works"], "resolved_links": result["library"]["resolved_non_rejected_rows"], "synthetic_metrics": result["synthetic"]["metrics"]}, ensure_ascii=False))


if __name__ == "__main__":
    main()
