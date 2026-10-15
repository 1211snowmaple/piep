// 使い方の説明と、実際の画面がずれていないかを見る。
//
// IPC 契約には抽出器とドリフト検査があるのに、人が読む説明には何も無かった。
// 画面が一つ増えても、増えたことを誰も教えてくれない。README の「画面」節が
// 実物より古くなっても、CI は緑のままだった。
//
// 突き合わせる相手は `e2e/readme-shots.spec.ts` の `shots` 表である。ここは
// 「どの画面を撮るか」の一次資料で、経路と主題文まで持っている。スクリーン
// ショットを撮る側と説明を書く側が同じ表を見ていれば、片方だけが増えたときに
// 気づける。
//
// 説明の側の目印は `<!--screen:home-->` のような HTML コメント。見出しの
// 文言は日本語で自由に付けたいので、対応づけは文言ではなく目印で行う。

import { readFileSync, existsSync } from "node:fs";
import { resolve } from "node:path";

const here = import.meta.dirname;
const repo = resolve(here, "..", "..");
const specPath = resolve(repo, "e2e", "readme-shots.spec.ts");
const guidePath = resolve(repo, "docs", "guide", "03-screens.md");

const problems = [];

// ------------------------------------------------------- 撮っている画面

if (!existsSync(specPath)) {
  console.error(`${specPath} が無い。画面の一次資料が消えている。`);
  process.exit(2);
}
const spec = readFileSync(specPath, "utf8");
const table = spec.match(/const shots:[\s\S]*?\n\];/);
if (!table) {
  console.error("readme-shots.spec.ts から `shots` 表を読めなかった。表の書き方が変わった可能性がある。");
  process.exit(2);
}

/** 画面名 → 撮っているテーマ。 */
const shots = new Map();
for (const m of table[0].matchAll(/^\s*\["([a-z-]+)",\s*"([^"]+)",\s*"[^"]*",\s*\[([^\]]*)\]/gm)) {
  const themes = [...m[3].matchAll(/"(light|dark)"/g)].map((t) => t[1]);
  shots.set(m[1], { route: m[2], themes });
}
if (shots.size === 0) {
  console.error("`shots` 表から画面を一つも読めなかった。");
  process.exit(2);
}

// ------------------------------------------------------- 説明している画面

if (!existsSync(guidePath)) {
  console.error(`${guidePath} が無い。`);
  process.exit(2);
}
const guide = readFileSync(guidePath, "utf8");
const described = new Set([...guide.matchAll(/<!--screen:([a-z-]+)-->/g)].map((m) => m[1]));

// ------------------------------------------------------- 突き合わせ

for (const name of shots.keys()) {
  if (!described.has(name)) {
    problems.push(
      `画面 "${name}" (${shots.get(name).route}) を撮っているのに、docs/guide/03-screens.md に説明が無い。` +
        `\n    → 節を足して \`<!--screen:${name}-->\` を置くこと`,
    );
  }
}

for (const name of described) {
  if (!shots.has(name)) {
    problems.push(
      `docs/guide/03-screens.md が画面 "${name}" を説明しているが、readme-shots.spec.ts は撮っていない。` +
        `\n    → 画面が消えたなら節も消す。残すなら shots 表に足す`,
    );
  }
}

// 貼ってある絵が本当にあるか。撮り直しの範囲を変えたときに残骸が出る。
for (const m of guide.matchAll(/!\[[^\]]*\]\(([^)]+)\)/g)) {
  const src = m[1];
  const file = resolve(repo, "docs", "guide", src);
  if (!existsSync(file)) {
    problems.push(`貼ってある画像が無い: ${src}`);
  }
}

// 撮っていないテーマの絵を貼っていないか。
for (const [name, { themes }] of shots) {
  for (const theme of ["light", "dark"]) {
    const referenced = guide.includes(`${name}-${theme}.png`);
    if (referenced && !themes.includes(theme)) {
      problems.push(
        `${name}-${theme}.png を貼っているが、shots 表はその画面を ${theme} で撮っていない。`,
      );
    }
  }
}

if (problems.length) {
  console.error("使い方の説明と実際の画面がずれている。\n");
  for (const p of problems) console.error(`  - ${p}`);
  console.error(`\n${problems.length} 件。`);
  process.exit(1);
}

console.log(`画面の説明は実物と一致 (${shots.size} 画面)`);
