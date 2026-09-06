"""未シリーズ連続物の題名候補を比較する。DBは読み取り専用、結果はscratchへ。

本番のルール出力をexporterから受け取り、選択8件より前の題名候補だけを比較する。
合成例は回帰用であり、実データの精度ではない。本文リンクは弱い参考ラベルとして
のみ使い、正解とは呼ばない。外部サービス・追加Python依存は使わない。
"""
from __future__ import annotations

import argparse
import collections
import functools
import hashlib
import itertools
import json
import re
import sqlite3
import subprocess
import time
import unicodedata
from pathlib import Path


def key(text):
    text = unicodedata.normalize("NFKC", text).lower()
    text = "".join(chr(ord(c) - 0x60) if "ァ" <= c <= "ヶ" else c for c in text)
    return "".join(c for c in text if c.isalnum())


def number(text):
    if text.isdigit():
        return int(text)
    digits = dict(zip("〇零一二三四五六七八九", [0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9]))
    total = current = 0
    for char in text:
        if char in digits:
            current = current * 10 + digits[char]
        elif char in "十百":
            total += (current or 1) * {"十": 10, "百": 100}[char]
            current = 0
        else:
            return None
    return total + current


NUM = r"[0-9〇零一二三四五六七八九十百]{1,4}"
EPISODE = re.compile(
    rf"第\s*({NUM})\s*(?:話|章|回|編|部|夜|巻|周目|節|幕)"
    rf"|({NUM})\s*(?:話|章|回|編|部|夜|巻|周目|節|幕)"
    rf"|(?:#|その|\bpart\s*|\bep\.?\s*)({NUM})", re.I
)
STAGE = re.compile(r"前編|中編|後編|前篇|中篇|後篇|[（(【\[](上|中|下)[）)】\]]")
EDITION = re.compile(r"[【\[(（](?:サンプル|完全版|先行公開|再掲|FANBOX|pixiv|おまけ付き)[】\])）]", re.I)
ADMIN = re.compile(r"^(?:新作の)?お知らせ|活動報告|活動記録|進捗|スケジュール")
BRACKET = re.compile(r"[【\[(（]([^】\])）]+)[】\])）]")


def parse_v2(title):
    """先頭の配布注記を本題にせず、章・前後編・丸数字を順序の経路として残す。

    v1で観察した誤結合を修正した開発用の第二案。独立テストでの改善ではない。
    """
    text = "".join(f"第{ord(c)-0x245F}話" if "①" <= c <= "⑳" else c for c in title)
    text = unicodedata.normalize("NFKC", text)
    def remove_metadata(match):
        content = match.group(1).strip()
        metadata = re.fullmatch(r"(?:(?:ファンボ|FANBOX|pixiv)[・/\s]*)?(?:サンプル|完全版|先行公開|再掲|おまけ付き)", content, re.I)
        length = re.fullmatch(r"(?:本編|本文)?(?:約|全)?[0-9,.万千]+(?:文字|字)", content)
        return "" if metadata or length else match.group(0)
    text = BRACKET.sub(remove_metadata, text)
    tokens = []
    for match in EPISODE.finditer(text):
        value = number(next(g for g in match.groups() if g is not None))
        tokens.append((match.start(),match.end(),value,"numbered"))
    for match in STAGE.finditer(text):
        raw = match.group(1) or match.group(0)[0]
        tokens.append((match.start(),match.end(),{"前":1,"上":1,"中":2,"後":3,"下":3}[raw],"stage"))
    if not tokens:
        parsed = parse(text)
        parsed["order_path"] = (parsed["order"],) if parsed["order"] is not None else ()
        return parsed
    tokens.sort()
    before, after = text[:tokens[0][0]],text[tokens[-1][1]:]
    stem = before if key(before) else after
    return {"stem":key(stem), "order":tokens[0][2], "kind":tokens[0][3],
            "order_path":tuple(t[2] for t in tokens),
            "brackets":tuple(key(v) for v in BRACKET.findall(stem)),
            "administrative":bool(ADMIN.search(text) or re.search(r"投票ページ|投票結果|結果報告",text))}


def parse(title):
    # 丸数字の意味をNFKCの前に保存する。
    text = "".join(f"第{ord(c)-0x245F}話" if "①" <= c <= "⑳" else c for c in title)
    text = EDITION.sub("", unicodedata.normalize("NFKC", text))
    match = EPISODE.search(text)
    order = None
    kind = None
    if match:
        order = number(next(g for g in match.groups() if g is not None))
        kind = "numbered"
    else:
        match = STAGE.search(text)
        if match:
            raw = match.group(1) or match.group(0)[0]
            order = {"前": 1, "上": 1, "中": 2, "後": 3, "下": 3}[raw]
            kind = "stage"
    if not match:
        match = re.search(r"^\s*([0-9]{1,3})\s*[：:．.、_\-）)】\]]", text)
        if not match:
            match = re.search(r"\s([0-9]{1,3})\s*$", text)
        if match:
            order, kind = int(match.group(1)), "bare"
    if not match:
        match = re.search(r"\s(I|II|III|IV|V|VI|VII|VIII|IX|X)\s*$", text)
        if match:
            order = ["I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X"].index(match.group(1)) + 1
            kind = "roman"
    stem = text
    if match:
        before, after = text[:match.start()], text[match.end():]
        # 話数の後ろにある各話副題は別フィールドとして残せる。先頭連番なら後ろが本題。
        stem = before if key(before) else after
    brackets = tuple(key(v) for v in re.findall(r"[【\[(（]([^】\])）]+)[】\])）]", stem))
    return {"stem": key(stem), "order": order, "kind": kind,
            "brackets": brackets, "administrative": bool(ADMIN.search(text))}


@functools.lru_cache(maxsize=40_000)
def ngrams(text):
    return {text[i:i+3] for i in range(max(1, len(text)-2))} if text else set()


def dice(left, right):
    a, b = ngrams(left), ngrams(right)
    return 2 * len(a & b) / (len(a) + len(b)) if a and b else 0.0


def ordinal_compatible(left, right):
    a, b = left["order"], right["order"]
    if a is not None and b is not None:
        return a != b
    # 無番号初回は自動確定せず候補に残す。最低限の照合可能な題名を要求する。
    return (a or b) in (2, 3) and min(len(left["stem"]), len(right["stem"])) >= 4


def proposed(left, right, method, threshold=0.75):
    if left["administrative"] or right["administrative"]:
        return False
    a, b = left["stem"], right["stem"]
    if min(len(a), len(b)) < 2:
        return False
    if method == "broad_ngram":
        return dice(a, b) >= threshold
    compatible = ordinal_compatible(left,right)
    if method.startswith("structured_v2") and left.get("order_path") and right.get("order_path"):
        compatible = left["order_path"] != right["order_path"]
    if not compatible:
        return False
    if a == b:
        return True
    if method in ("structured_exact", "structured_v2"):
        return False
    # 意味のある括弧は削除しない。同一人物と断定するための近似照合はしない。
    if left["brackets"] != right["brackets"]:
        return False
    return min(len(a), len(b)) >= 9 and dice(a, b) >= threshold


def baseline_pair(left, right):
    a, b = left["family_match_key"], right["family_match_key"]
    return (not left["is_administrative_post"] and not right["is_administrative_post"]
            and left["edition_match_key"] != right["edition_match_key"]
            and min(len(a), len(b)) >= 9 and a[:26] == b[:26]
            and (left["has_ordinal_marker"] or right["has_ordinal_marker"]))


def components(pairs):
    adjacency = collections.defaultdict(set)
    for a, b in pairs:
        adjacency[a].add(b)
        adjacency[b].add(a)
    result, seen = [], set()
    for a in sorted(adjacency):
        if a in seen:
            continue
        stack, group = [a], set()
        while stack:
            b = stack.pop()
            if b in seen:
                continue
            seen.add(b)
            group.add(b)
            stack.extend(adjacency[b] - seen)
        result.append(group)
    return result


def metrics(predictions, fixtures):
    counts = collections.Counter()
    for predicted, fixture in zip(predictions, fixtures):
        counts[(fixture["positive"], predicted)] += 1
    tp, fp = counts[True, True], counts[False, True]
    fn, tn = counts[True, False], counts[False, False]
    return {"tp": tp, "fp": fp, "fn": fn, "tn": tn,
            "precision": tp/(tp+fp) if tp+fp else None, "recall": tp/(tp+fn),
            "false_positive_ids": [f["id"] for p,f in zip(predictions, fixtures) if p and not f["positive"]],
            "false_negative_ids": [f["id"] for p,f in zip(predictions, fixtures) if not p and f["positive"]]}


def fixture_experiment(exe, filename="fixtures.json"):
    fixtures = json.loads(Path(__file__).with_name(filename).read_text(encoding="utf-8"))
    titles = [f[k] for f in fixtures for k in ("left", "right")]
    completed = subprocess.run([str(exe), "--titles"], input=json.dumps(titles, ensure_ascii=False),
                               text=True, encoding="utf-8", capture_output=True, check=True)
    rust = json.loads(completed.stdout)
    parsed = [parse(t) for t in titles]
    parsed_v2 = [parse_v2(t) for t in titles]
    results = {}
    for method in ["baseline", "structured_exact", "broad_ngram", "structured_ngram", "structured_v2", "structured_v2_ngram"]:
        predictions = []
        for i, fixture in enumerate(fixtures):
            chosen = parsed_v2 if method.startswith("structured_v2") else parsed
            value = baseline_pair(rust[2*i], rust[2*i+1]) if method == "baseline" else proposed(chosen[2*i], chosen[2*i+1], method)
            predictions.append(value and fixture.get("same_author", True))
        results[method] = metrics(predictions, fixtures)
    results["threshold_sensitivity"] = {}
    for threshold in (0.65, 0.75, 0.85, 0.9):
        predictions = [proposed(parsed[2*i], parsed[2*i+1], "structured_ngram", threshold)
                       and f.get("same_author", True) for i, f in enumerate(fixtures)]
        results["threshold_sensitivity"][str(threshold)] = metrics(predictions, fixtures)
    return results


def author_key(w):
    # 取得元内の作者ID。取得元をまたぐ表示名一致は別の候補経路として扱う。
    return (w["source"], w["author_id"] or w["author_name"])


def library_experiment(rows, conn):
    eligible = [w for w in rows if not w["is_administrative_post"]]
    editions = collections.defaultdict(list)
    for w in eligible:
        editions[(w["normalized_author_key"], w["edition_match_key"])].append(w)
    works, alias = [], {}
    for members in editions.values():
        representative = min(members, key=lambda w: (-w["text_length"], w["id"]))
        works.append(representative)
        alias.update({w["id"]: representative["id"] for w in members})
    works.sort(key=lambda w: w["id"])
    by_id = {w["id"]: w for w in works}
    parsed = {w["id"]: parse(w["title"]) for w in works}
    parsed_v2 = {w["id"]: parse_v2(w["title"]) for w in works}
    series = collections.defaultdict(set)
    series_names = {}
    for did, source, sid, title in conn.execute("SELECT download_id,series_source,series_key,title FROM download_series"):
        series[did].add((source,sid))
        series_names[source,sid] = title
    # 別版の所属を代表へ移す。この集合は優先度と集計専用で、題名候補の入力ではない。
    effective_series = collections.defaultdict(set)
    for did, representative in alias.items():
        effective_series[representative].update(series[did])
    unregistered = {did for did in by_id if not effective_series[did]}
    groups = collections.defaultdict(list)
    for w in works:
        k = w["family_match_key"]
        if len(k) >= 9:
            groups[(w["normalized_author_key"], k[:26])].append(w)
    baseline = set()
    valid_families = []
    for group in groups.values():
        if 2 <= len(group) <= 40 and any(w["has_ordinal_marker"] for w in group):
            valid_families.append([w["id"] for w in group])
            baseline.update(tuple(sorted((a["id"], b["id"]))) for a,b in itertools.combinations(group,2))

    # 作者内3-gram転置索引で比較候補を作る。短題は語幹一致からも取る。
    postings = collections.defaultdict(list)
    exact = collections.defaultdict(list)
    candidates = set()
    all_author_pairs = 0
    authors = collections.Counter(author_key(w) for w in works)
    all_author_pairs = sum(n*(n-1)//2 for n in authors.values())
    for w in works:
        did, author, stem = w["id"], author_key(w), parsed[w["id"]]["stem"]
        blocks = [("account",author)]
        if w["author_name"].strip():
            blocks.append(("cross_source_name",w["normalized_author_key"]))
        for block in blocks:
            def allow(previous):
                return block[0] == "account" or by_id[previous]["source"] != w["source"]
            for gram in ngrams(stem):
                for previous in postings[block, gram]:
                    if allow(previous):
                        candidates.add((previous, did))
                postings[block, gram].append(did)
            for previous in exact[block, stem]:
                if allow(previous):
                    candidates.add((previous, did))
            exact[block, stem].append(did)
    methods = {"baseline_title": baseline}
    for method in ("structured_exact", "broad_ngram", "structured_ngram"):
        methods[method] = {(a,b) for a,b in candidates if proposed(parsed[a],parsed[b],method)}
    # 和集合は検索入口の試作で、全ペアを続編として採用する方法ではない。
    v2_postings = collections.defaultdict(list)
    v2_grams = collections.defaultdict(list)
    v2_candidates = set()
    v2_pairs = set()
    for w in works:
        did = w["id"]
        blocks = [("account",author_key(w))]
        if w["author_name"].strip():
            blocks.append(("cross_source_name",w["normalized_author_key"]))
        for block in blocks:
            for gram in ngrams(parsed_v2[did]["stem"]):
                for previous in v2_grams[block,gram]:
                    if block[0] == "account" or by_id[previous]["source"] != w["source"]:
                        v2_candidates.add((previous,did))
                v2_grams[block,gram].append(did)
            for previous in v2_postings[block,parsed_v2[did]["stem"]]:
                if (block[0] == "account" or by_id[previous]["source"] != w["source"]) and proposed(parsed_v2[previous],parsed_v2[did],"structured_v2"):
                    v2_pairs.add((previous,did))
            v2_postings[block,parsed_v2[did]["stem"]].append(did)
    methods["structured_v2"] = v2_pairs
    methods["structured_v2_ngram"] = v2_pairs | {(a,b) for a,b in v2_candidates if proposed(parsed_v2[a],parsed_v2[b],"structured_v2_ngram")}
    methods["union_retrieval"] = baseline | v2_pairs

    weak = set()
    resolved_continuation_rows = 0
    links_lost_before_alias = 0
    for a,b,relation in conn.execute("SELECT from_download_id,to_download_id,relation_type FROM work_links WHERE status!='rejected' AND from_download_id IS NOT NULL AND to_download_id IS NOT NULL"):
        if a in alias and b in alias and (a not in by_id or b not in by_id):
            links_lost_before_alias += 1
        if relation not in ("continues_from","continues_to") or a not in alias or b not in alias:
            continue
        resolved_continuation_rows += 1
        a,b = alias[a],alias[b]
        if a != b and a in unregistered and b in unregistered:
            weak.add(tuple(sorted((a,b))))
    result = {"total": len(rows), "eligible": len(eligible), "representatives": len(works),
              "unregistered_representatives": len(unregistered),
              "edition_nonrepresentatives": len(eligible)-len(works),
              "resolved_links_touching_nonrepresentatives": links_lost_before_alias,
              "resolved_continuation_rows_eligible": resolved_continuation_rows,
              "weak_continuation_pairs_both_unregistered": len(weak),
              "all_author_pairs": all_author_pairs, "ngram_candidate_comparisons": len(candidates),
              "baseline_families": len(valid_families), "methods": {}}
    def scope(pair):
        a,b = pair
        common = effective_series[a] & effective_series[b]
        if common:
            administrative = any(re.search(r"有償依頼|リクエスト|支援|サンプル|依頼|投げ銭|お礼|寄稿|まとめ|その他",series_names[s]) for s in common)
            return "inside_administrative_series" if administrative else "inside_other_series"
        if not effective_series[a] and not effective_series[b]:
            return "both_unregistered"
        return "different_or_one_series"
    private = {}
    for method, pairs in methods.items():
        method_parsed = parsed_v2 if method.startswith("structured_v2") or method == "union_retrieval" else parsed
        unregistered_pairs = {(a,b) for a,b in pairs if a in unregistered and b in unregistered}
        new = unregistered_pairs - baseline
        comps = components(unregistered_pairs)
        result["methods"][method] = {"pairs": len(pairs), "both_unregistered_pairs": len(unregistered_pairs),
            "new_both_unregistered_pairs_vs_baseline": len(new),
            "components_both_unregistered": len(comps), "largest_component": max(map(len, comps), default=0),
            "weak_link_pairs_retrieved": len(unregistered_pairs & weak),
            "weak_link_coverage": len(unregistered_pairs & weak)/len(weak) if weak else None,
            "scope_pairs": dict(collections.Counter(scope(pair) for pair in pairs)),
            "new_scope_pairs": dict(collections.Counter(scope(pair) for pair in pairs-baseline)),
            "cross_source_pairs": sum(by_id[a]["source"] != by_id[b]["source"] for a,b in pairs),
            "new_cross_source_pairs": sum(by_id[a]["source"] != by_id[b]["source"] for a,b in pairs-baseline)}
        # 規則順位の監査用。真の続編かどうかは別に人が確認する。
        ranked = sorted(pairs-baseline, key=lambda pair: (-dice(method_parsed[pair[0]]["stem"], method_parsed[pair[1]]["stem"]), pair))
        private[method] = [{"ids": [a,b], "left": by_id[a]["title"], "right": by_id[b]["title"],
                           "source": by_id[a]["source"], "author": by_id[a]["author_name"],
                           "sources": [by_id[a]["source"],by_id[b]["source"]], "scope": scope((a,b)),
                           "dice": dice(method_parsed[a]["stem"],method_parsed[b]["stem"]),
                           "orders": [method_parsed[a].get("order_path",method_parsed[a]["order"]),method_parsed[b].get("order_path",method_parsed[b]["order"])],
                           "weak_link": (a,b) in weak} for a,b in ranked]
    return result, private


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db", type=Path, required=True)
    parser.add_argument("--exporter", type=Path, required=True)
    parser.add_argument("--out", type=Path, default=Path("scratch/sequence-research"))
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    exe = args.exporter.resolve()
    db_fingerprint = hashlib.sha256(args.db.read_bytes()).hexdigest()
    start = time.perf_counter()
    exported = subprocess.run([str(exe), str(args.db.resolve())], capture_output=True,
                              text=True, encoding="utf-8", check=True)
    rows = json.loads(exported.stdout)
    export_elapsed = time.perf_counter()-start
    (args.out / "rule-export.json").write_text(exported.stdout, encoding="utf-8")
    fixture_results = fixture_experiment(exe)
    regression_results = fixture_experiment(exe, "regressions.json")
    library_start = time.perf_counter()
    with sqlite3.connect(args.db.resolve().as_uri()+"?mode=ro", uri=True) as conn:
        library, private = library_experiment(rows, conn)
    results = {"scope": "title candidate stage, before full sweep merge/feedback/quota; not end-to-end precision",
               "snapshot_sha256": db_fingerprint,
               "fixtures": fixture_results, "v2_regressions": regression_results,
               "library": library, "elapsed_seconds": time.perf_counter()-start,
               "export_seconds": export_elapsed, "library_comparison_seconds": time.perf_counter()-library_start,
               "snapshot_unchanged": hashlib.sha256(args.db.read_bytes()).hexdigest() == db_fingerprint}
    (args.out / "title-results.json").write_text(json.dumps(results, ensure_ascii=False, indent=2), encoding="utf-8")
    (args.out / "private-title-pairs.json").write_text(json.dumps(private, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(results, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
