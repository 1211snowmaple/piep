"""TypeScript の export と Rust の pub 項目のうち、定義した場所の外で名前が一度も出ない物を挙げる。

使い方（リポジトリの根で）:
    python -I tools/audit/unused_exports.py ts    # src・shared・android/src・e2e の export
    python -I tools/audit/unused_exports.py rust  # src-tauri・crates・android/src-tauri の pub fn/struct/enum/const

名前の一致で数えるだけなので、出てきた物は候補。消す前に呼び出しの関係（動的な呼び出し・IPC の名前・
serde の名前・マクロ）で確かめる。同じ名前が別の物にも使われていると、漏れる（候補に出ない）。
"""

from __future__ import annotations

import re
import subprocess
import sys
from collections import defaultdict

TS_EXPORT = re.compile(
    r"^export\s+(?:default\s+)?(?:declare\s+)?(?:async\s+)?(?:function\*?|const|let|class|interface|type|enum)\s+([A-Za-z_$][\w$]*)",
    re.M,
)
RS_PUB = re.compile(r"^\s*pub(?:\([^)]*\))?\s+(?:async\s+)?(?:unsafe\s+)?(?:fn|struct|enum|const|static|trait|type)\s+([A-Za-z_]\w*)", re.M)
WORD = re.compile(r"[A-Za-z_$][\w$]*")


def tracked(patterns: list[str]) -> list[str]:
    out = subprocess.run(["git", "ls-files", "-z", "--", *patterns], check=True, capture_output=True).stdout
    return [f for f in out.decode("utf-8").split("\0") if f]


def main() -> int:
    kind = sys.argv[1] if len(sys.argv) > 1 else "ts"
    if kind == "ts":
        defs_files = [f for f in tracked(["src", "shared", "android/src", "e2e"]) if f.endswith((".ts", ".tsx")) and not f.endswith(".d.ts")]
        pattern = TS_EXPORT
    else:
        defs_files = [f for f in tracked(["src-tauri/src", "crates", "android/src-tauri/src", "tools"]) if f.endswith(".rs")]
        pattern = RS_PUB
    corpus_files = [f for f in tracked(["."]) if f.endswith((".ts", ".tsx", ".rs", ".mjs", ".js", ".json", ".html", ".sql", ".yml", ".ps1"))]

    counts: dict[str, int] = defaultdict(int)
    per_file: dict[str, dict[str, int]] = {}
    texts: dict[str, str] = {}
    for f in corpus_files:
        try:
            text = open(f, encoding="utf-8").read()
        except (OSError, UnicodeDecodeError):
            continue
        texts[f] = text
        local: dict[str, int] = defaultdict(int)
        for w in WORD.findall(text):
            local[w] += 1
        per_file[f] = local
        for w, n in local.items():
            counts[w] += n

    rows = []
    for f in defs_files:
        text = texts.get(f, "")
        for m in pattern.finditer(text):
            name = m.group(1)
            if name in {"default", "main", "new"}:
                continue
            here = per_file.get(f, {}).get(name, 0)
            elsewhere = counts[name] - here
            if elsewhere == 0:
                line = text.count("\n", 0, m.start()) + 1
                rows.append((f, line, name, here))
    for f, line, name, here in sorted(rows):
        print(f"{f}:{line}\t{name}\t同じファイルで {here} 回")
    print(f"# 候補 {len(rows)} 件", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
