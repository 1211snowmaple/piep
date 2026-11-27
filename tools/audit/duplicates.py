"""ファイルをまたいで同じ並びの行（空白を詰めた後で WINDOW 行以上）を挙げる。似た実装の手がかりを道具で拾う。

使い方（リポジトリの根で）:
    python -I tools/audit/duplicates.py [WINDOW]   # 既定 8 行

import・空行・括弧だけの行は数えない。試験・生成物・視覚の基準は外す。出た組は候補で、まとめるかどうかは
意図した違いかを読んで決める。
"""

from __future__ import annotations

import re
import subprocess
import sys
from collections import defaultdict

SKIP = re.compile(r"^(import |use |export \{|\}|\)|\]|;|//|/\*|\*|#\[|<\/|\);|\},|\}\);|\]\);)")


def tracked() -> list[str]:
    out = subprocess.run(["git", "ls-files", "-z"], check=True, capture_output=True).stdout.decode("utf-8")
    keep = []
    for f in out.split("\0"):
        if not f.endswith((".rs", ".ts", ".tsx", ".css", ".mjs", ".ps1")):
            continue
        if any(x in f for x in (".test.", "_tests.rs", "/tests/", "tests.rs", "e2e/", "/gen/", "vendor/", "mocks/")):
            continue
        keep.append(f)
    return keep


def main() -> int:
    window = int(sys.argv[1]) if len(sys.argv) > 1 else 8
    seen: dict[tuple[str, ...], list[tuple[str, int]]] = defaultdict(list)
    for f in tracked():
        try:
            lines = open(f, encoding="utf-8").read().splitlines()
        except (OSError, UnicodeDecodeError):
            continue
        norm = []
        for i, raw in enumerate(lines, 1):
            s = re.sub(r"\s+", " ", raw.strip())
            if len(s) < 4 or SKIP.match(s):
                continue
            norm.append((s, i))
        for k in range(len(norm) - window + 1):
            key = tuple(s for s, _ in norm[k : k + window])
            seen[key].append((f, norm[k][1]))
    groups: dict[frozenset[str], list[tuple[str, int]]] = {}
    for key, places in seen.items():
        files = frozenset(f for f, _ in places)
        if len(files) < 2:
            continue
        groups.setdefault(files, places)
    pairs = sorted(groups.items(), key=lambda kv: sorted(kv[0]))
    for files, places in pairs:
        firsts = {}
        for f, line in places:
            firsts.setdefault(f, line)
        print("  ".join(f"{f}:{line}" for f, line in sorted(firsts.items())))
    print(f"# ファイルの組 {len(pairs)}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
