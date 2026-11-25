"""監査のファイルの目録を `git ls-files` から作り、手で書いた欄を残したまま突き合わせる。

使い方（リポジトリの根で）:
    python -I tools/audit/inventory.py docs/audit/2026-11-25-files.md

- 追跡している全ファイルを一行ずつ載せる。手では選ばない。
- 既に目録があれば、担当・見た観点・発見・状態の欄はパスで引き継ぐ。
- 消えたファイルの行は「消えた」として末尾に残す（黙って落とさない）。
- 最後に、目録の行と `git ls-files` の差（漏れ・余り）を数えて出す。
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

COLUMNS = ["ファイル", "区分", "行数", "担当", "見た観点", "発見", "状態"]
MANUAL = ["担当", "見た観点", "発見", "状態"]

CONFIG_NAMES = {
    ".gitignore", ".oxlintrc.json", "package.json", "package-lock.json", "tsconfig.json",
    "tsconfig.node.json", "tsconfig.e2e.json", "vite.config.ts", "vite.admin.config.ts",
    "vitest.config.ts", "playwright.config.ts", "postcss.config.cjs", "rust-toolchain.toml",
    "index.html", "Cargo.toml", "Cargo.lock", "tauri.conf.json", "build.rs", "rustfmt.toml",
    "clippy.toml", "capabilities", "gradle.properties", "settings.gradle", "build.gradle",
    "build.gradle.kts", "settings.gradle.kts", "proguard-rules.pro", "AndroidManifest.xml",
}
IMAGE_EXT = {".png", ".jpg", ".jpeg", ".webp", ".ico", ".icns", ".gif", ".bmp", ".svg", ".ttf", ".woff", ".woff2"}
TEST_MARKERS = (".test.", "_tests.rs", "/tests/", "tests.rs", ".spec.")


def classify(path: str) -> str:
    p = path
    name = p.rsplit("/", 1)[-1]
    ext = Path(name).suffix.lower()
    if p.startswith("e2e/__screenshots__/"):
        return "視覚の基準"
    if p.startswith(".github/workflows/"):
        return "CI"
    if p.startswith((".github/", ".vscode/")):
        return "設定"
    if p.startswith(("tools/", "docs-tools/")):
        return "道具"
    if p.startswith("e2e/") or any(m in p for m in TEST_MARKERS) or "/mocks/" in p or p.startswith("src/mocks/"):
        return "試験"
    if p.startswith("docs/") or ext == ".md":
        return "文書"
    if name in CONFIG_NAMES or p.endswith((".json", ".toml", ".yml", ".yaml", ".gradle", ".kts", ".properties", ".pro")) and not p.startswith(("src/", "android/src/")):
        if not p.startswith(("crates/", "shared/")) or name in CONFIG_NAMES:
            return "設定"
    if p.startswith("android/"):
        return "Mobile"
    if p.startswith(("crates/", "shared/")):
        return "Shared"
    if p.startswith(("src-tauri/src/server/", "src-tauri/src/bin/", "src/admin/")) or "admin" in name and p.startswith("vite."):
        return "Server"
    if p.startswith(("src-tauri/", "src/", "public/")) or ext in IMAGE_EXT:
        return "Desktop"
    return "設定"


def line_count(path: str) -> str:
    try:
        data = Path(path).read_bytes()
    except OSError:
        return "—"
    if b"\0" in data[:8000]:
        return "二進"
    return str(data.count(b"\n") + (0 if data.endswith(b"\n") or not data else 1))


def read_existing(target: Path) -> dict[str, dict[str, str]]:
    rows: dict[str, dict[str, str]] = {}
    if not target.exists():
        return rows
    for line in target.read_text(encoding="utf-8").splitlines():
        if not line.startswith("| `"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) != len(COLUMNS):
            continue
        row = dict(zip(COLUMNS, cells))
        rows[row["ファイル"].strip("`")] = row
    return rows


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    target = Path(sys.argv[1])
    files = subprocess.run(["git", "ls-files", "-z"], check=True, capture_output=True).stdout.decode("utf-8").split("\0")
    files = sorted(f for f in files if f)
    old = read_existing(target)

    head = target.read_text(encoding="utf-8").split("\n| ファイル |", 1)[0].rstrip() if target.exists() else (
        "# 2026-11-25 監査のファイルの目録\n\n"
        "`python -I tools/audit/inventory.py docs/audit/2026-11-25-files.md` で `git ls-files` から作る。"
        "手で行を足したり消したりしない。担当・見た観点・発見・状態の欄だけを手で書き、作り直しても残る。\n\n"
        "観点の記号は台帳（[2026-11-25.md](2026-11-25.md) の「観点の一覧」）の番号。"
    )

    out = [head, "", "| " + " | ".join(COLUMNS) + " |", "|" + "---|" * len(COLUMNS)]
    present = set(files)
    for f in files:
        prev = old.get(f, {})
        row = {
            "ファイル": f"`{f}`",
            "区分": classify(f),
            "行数": line_count(f),
            **{k: prev.get(k, "—" if k != "状態" else "未確認") for k in MANUAL},
        }
        out.append("| " + " | ".join(row[c] for c in COLUMNS) + " |")
    gone = sorted(set(old) - present)
    for f in gone:
        prev = old[f]
        if prev.get("状態", "").startswith("消えた"):
            row = prev
        else:
            row = {**prev, "状態": "消えた（" + prev.get("状態", "") + "）"}
        out.append("| " + " | ".join(row.get(c, "—") for c in COLUMNS) + " |")
    target.write_text("\n".join(out) + "\n", encoding="utf-8", newline="\n")

    listed = set(read_existing(target)) - set(gone)
    missing = present - listed
    extra = listed - present
    by_class: dict[str, int] = {}
    by_state: dict[str, int] = {}
    for f in files:
        by_class[classify(f)] = by_class.get(classify(f), 0) + 1
    for f, r in read_existing(target).items():
        if f in present:
            by_state[r["状態"]] = by_state.get(r["状態"], 0) + 1
    print(f"git ls-files: {len(files)} 件 / 目録: {len(listed)} 件 / 漏れ {len(missing)} / 余り {len(extra)} / 消えた行 {len(gone)}")
    print("区分:", ", ".join(f"{k} {v}" for k, v in sorted(by_class.items(), key=lambda x: -x[1])))
    print("状態:", ", ".join(f"{k} {v}" for k, v in sorted(by_state.items(), key=lambda x: -x[1])))
    return 0 if not missing and not extra else 1


if __name__ == "__main__":
    raise SystemExit(main())
