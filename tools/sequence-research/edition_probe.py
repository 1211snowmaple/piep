"""サービス跨ぎ候補で、続編と本文包含を区別する小実験。本文を出力・送信しない。"""
from __future__ import annotations

import argparse
import collections
import hashlib
import html
import json
from pathlib import Path
import re
import sqlite3
import unicodedata


def body_text(document):
    if isinstance(document.get("text"), str):
        return document["text"]
    body = document.get("body") or {}
    if isinstance(body.get("text"),str):
        return body["text"]
    return "\n".join(block["text"] for block in body.get("blocks",[]) if isinstance(block.get("text"),str))


def normalize(text):
    text = html.unescape(text)
    text = re.sub(r"<[^>]+>|\[(?:newpage|chapter:[^\]]*)\]", "", text)
    return "".join(c for c in unicodedata.normalize("NFKC",text) if not c.isspace())


def coverage(left,right):
    if len(left) < 1000 or len(right) < 1000:
        return None
    width = 64
    stride = max(width,len(left)//300)
    samples = [left[i:i+width] for i in range(0,len(left)-width+1,stride)]
    return sum(sample in right for sample in samples)/len(samples)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--db",type=Path,required=True)
    parser.add_argument("--pairs",type=Path,required=True)
    parser.add_argument("--output",type=Path,required=True)
    args = parser.parse_args()
    pairs = json.loads(args.pairs.read_text(encoding="utf-8"))["structured_v2"]
    pairs = [pair for pair in pairs if len(set(pair["sources"])) > 1]
    c = sqlite3.connect(args.db.resolve().as_uri()+"?mode=ro",uri=True)
    paths = dict(c.execute("SELECT id,json_path FROM downloads"))
    c.close()
    cache, fingerprints = {}, {}
    def load(did):
        if did not in cache:
            try:
                raw = Path(paths[did]).read_bytes()
                fingerprints[did] = hashlib.sha256(raw).hexdigest()
                cache[did] = normalize(body_text(json.loads(raw)))
            except (OSError,ValueError,KeyError,TypeError):
                cache[did] = ""
        return cache[did]
    rows = []
    for pair in pairs:
        a,b = pair["ids"]
        left,right = load(a),load(b)
        ab,ba = coverage(left,right),coverage(right,left)
        outcome = "unknown_or_nonoverlapping"
        if ab is None or ba is None:
            outcome = "body_unavailable_or_short"
        elif min(ab,ba) >= 0.8:
            outcome = "near_equivalent_body_candidate"
        elif max(ab,ba) >= 0.8:
            outcome = "asymmetric_containment_candidate"
        rows.append({"ids":[a,b],"normalized_lengths":[len(left),len(right)],
                     "sample_containment":[ab,ba],"outcome":outcome})
    summary = {"scope":"19-or-current title-v2 new cross-source pairs only; development sample, not representative precision",
               "pairs":len(rows),"outcomes":dict(collections.Counter(r["outcome"] for r in rows)),
               "method":"Each direction checks up to approximately 300 literal 64-character body samples; 0.8 is an exploratory threshold.",
               "limitations":["A high containment is evidence for edition/excerpt/omnibus review, not an automatic identity verdict.",
                              "Rewrites, unsupported JSON body formats and missing paid content can be missed.",
                              "Body files are read at run time, not copied with the SQLite snapshot; per-file hashes are saved locally."]}
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(summary,ensure_ascii=False,indent=2),encoding="utf-8")
    private = args.output.with_name("private-edition-overlap.json")
    private.write_text(json.dumps({"pairs":rows,"body_hashes":fingerprints},ensure_ascii=False,indent=2),encoding="utf-8")
    print(json.dumps(summary,ensure_ascii=False,indent=2))


if __name__ == "__main__":
    main()
