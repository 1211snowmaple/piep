"""監査の機能の目録（IPC・HTTP の口・画面の道）をコードから作る。文書からは拾わない。

使い方（リポジトリの根で）:
    python -I tools/audit/features.py docs/audit/2026-11-25-features.md

既に目録があれば、経路・場面・使われ方・状態の欄は名前で引き継ぐ。
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

COLUMNS = ["種類", "名前", "定義", "経路", "場面", "使われ方", "状態"]
MANUAL = ["経路", "場面", "使われ方", "状態"]


def tracked(*paths: str) -> list[str]:
    out = subprocess.run(["git", "ls-files", "-z", "--", *paths], check=True, capture_output=True).stdout
    return [f for f in out.decode("utf-8").split("\0") if f]


def read(path: str) -> str:
    return Path(path).read_text(encoding="utf-8")


def ipc_commands() -> list[tuple[str, str, str]]:
    lib = read("src-tauri/src/lib.rs")
    body = re.search(r"generate_handler!\[(.*?)\]", lib, re.S).group(1)
    # 行の注釈を先に落とす。注釈の行と次の名前が同じ区切りに入ると、名前ごと落ちていた。
    body = re.sub(r"//[^\n]*", "", body)
    names = [c.strip() for c in body.split(",") if c.strip()]
    defs: dict[str, str] = {}
    for f in tracked("src-tauri/src"):
        if not f.endswith(".rs"):
            continue
        text = read(f)
        for m in re.finditer(r"#\[tauri::command[^\]]*\]\s*(?:#\[[^\]]*\]\s*(?://[^\n]*\s*)?)*pub(?:\([^)]*\))?\s+(?:async\s+)?fn\s+(\w+)", text):
            defs.setdefault(m.group(1), f"{f}:{text.count(chr(10), 0, m.start()) + 1}")
    return [("IPC", n.split("::")[-1], defs.get(n.split("::")[-1], n)) for n in names]


def http_routes() -> list[tuple[str, str, str]]:
    rows = []
    for f in tracked("src-tauri/src/server", "android/src-tauri/src"):
        if not f.endswith(".rs") or "tests" in f:
            continue
        text = read(f)
        # 試験の中で立てる口（`#[cfg(test)]` の後ろ）は製品の口ではない。
        test_module = text.find("#[cfg(test)]\nmod tests")
        if test_module >= 0:
            text = text[:test_module]
        for m in re.finditer(r'\.route\(\s*"([^"]+)"\s*,\s*([^\n]+)', text):
            methods = ",".join(sorted(set(re.findall(r"\b(get|post|put|delete|patch)\(", m.group(2)))))
            rows.append(("HTTP", f"{methods.upper() or '?'} {m.group(1)}", f"{f}:{text.count(chr(10), 0, m.start()) + 1}"))
    return rows


def desktop_screens() -> list[tuple[str, str, str]]:
    seen: dict[str, str] = {}
    for f in tracked("src"):
        if not f.endswith((".ts", ".tsx")) or ".test." in f:
            continue
        text = read(f)
        for m in re.finditer(r'(?:matchPath|useRouteParams)\("(/[^"]*)"', text):
            seen.setdefault(m.group(1), f"{f}:{text.count(chr(10), 0, m.start()) + 1}")
        for m in re.finditer(r'pathname === "(/[^"]*)"', text):
            seen.setdefault(m.group(1), f"{f}:{text.count(chr(10), 0, m.start()) + 1}")
    return [("画面（デスクトップ）", k, v) for k, v in sorted(seen.items())]


def mobile_screens() -> list[tuple[str, str, str]]:
    text = read("android/src/appHistory.tsx")
    rows = []
    for m in re.finditer(r'name:\s*"([a-z-]+)"', text):
        rows.append(("画面（携帯）", m.group(1), f"android/src/appHistory.tsx:{text.count(chr(10), 0, m.start()) + 1}"))
    uniq: dict[str, tuple[str, str, str]] = {}
    for r in rows:
        uniq.setdefault(r[1], r)
    return list(uniq.values())


def mobile_commands() -> list[tuple[str, str, str]]:
    lib = read("android/src-tauri/src/lib.rs")
    m = re.search(r"generate_handler!\[(.*?)\]", lib, re.S)
    if not m:
        return []
    return [("IPC（携帯）", c.strip().split("::")[-1], "android/src-tauri/src/lib.rs") for c in m.group(1).split(",") if c.strip() and not c.strip().startswith("//")]


def existing(target: Path) -> dict[tuple[str, str], dict[str, str]]:
    rows = {}
    if not target.exists():
        return rows
    for line in target.read_text(encoding="utf-8").splitlines():
        if not line.startswith("| ") or line.startswith("| 種類"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) == len(COLUMNS):
            row = dict(zip(COLUMNS, cells))
            rows[(row["種類"], row["名前"].strip("`"))] = row
    return rows


def main() -> int:
    target = Path(sys.argv[1])
    old = existing(target)
    items = ipc_commands() + mobile_commands() + http_routes() + desktop_screens() + mobile_screens()
    head = target.read_text(encoding="utf-8").split("\n| 種類 |", 1)[0].rstrip() if target.exists() else (
        "# 2026-11-25 監査の機能の目録\n\n"
        "`python -I tools/audit/features.py docs/audit/2026-11-25-features.md` でコードから作る"
        "（IPC は `generate_handler!`、HTTP は `.route(`、画面は道の照合から）。経路・場面・使われ方・状態だけを手で書く。\n\n"
        "場面の記号: 正=正常 異=異常 欠=データの欠け 重=重複 並=並列 中=中断 再=再起動 復=復旧 オ=オフライン 端=別の端末"
    )
    out = [head, "", "| " + " | ".join(COLUMNS) + " |", "|" + "---|" * len(COLUMNS)]
    counts: dict[str, int] = {}
    for kind, name, where in items:
        prev = old.get((kind, name), {})
        counts[kind] = counts.get(kind, 0) + 1
        vals = {"種類": kind, "名前": f"`{name}`", "定義": f"`{where}`", **{k: prev.get(k, "未確認" if k == "状態" else "—") for k in MANUAL}}
        out.append("| " + " | ".join(vals[c] for c in COLUMNS) + " |")
    target.write_text("\n".join(out) + "\n", encoding="utf-8", newline="\n")
    print(" / ".join(f"{k} {v}" for k, v in counts.items()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
