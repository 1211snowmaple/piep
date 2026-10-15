// 日本語で効かない強調記法を探す。
//
// CommonMark では、閉じる `**` は「右隣接」でなければ閉じ記号にならない。
// 直前が約物で、直後が空白でも約物でもない文字だと、この条件を満たさない。
//
//   **「今どうなっているか」**を書く    → 閉じない。`**` がそのまま出る
//   **「今どうなっているか」を書く**    → 閉じる
//
// 日本語では鉤括弧で括った語を強調したくなるので、まず間違いなく踏む。
// 目で見つけるのは無理なので機械に探させる。

import { readFileSync } from "node:fs";
import { globSync } from "node:fs";
import { relative, resolve } from "node:path";

/** Unicode の約物のうち、日本語で強調の末尾に来やすいもの。 */
const PUNCT = /[」』）】〉》、。！？…："'）\)\]\}.,!?;:]/;
/** 空白か約物か。 */
const CLOSER_OK = /[\s」』）】〉》、。！？…："'）\)\]\}.,!?;:]/;

const roots = process.argv.slice(2);
if (!roots.length) {
  console.error("使い方: node check-japanese-emphasis.mjs <ファイルかディレクトリ...>");
  process.exit(2);
}

const files = roots.flatMap((r) =>
  globSync(r.endsWith(".md") ? r : `${r}/**/*.md`, { cwd: process.cwd() }),
);

let found = 0;
for (const file of files) {
  const text = readFileSync(file, "utf8");
  const lines = text.split("\n");
  let inFence = false;
  lines.forEach((line, i) => {
    if (/^\s*```/.test(line)) inFence = !inFence;
    if (inFence) return;
    // `**` の出現位置を順に見て、偶数番目を開き、奇数番目を閉じとみなす。
    const marks = [...line.matchAll(/\*\*/g)];
    marks.forEach((m, idx) => {
      if (idx % 2 === 0) return; // 開き側は見ない
      const before = line[m.index - 1] ?? " ";
      const after = line[m.index + 2] ?? " ";
      if (PUNCT.test(before) && !CLOSER_OK.test(after)) {
        found += 1;
        console.log(`${file}:${i + 1}`);
        console.log(`  ${line.trim()}`);
        console.log(`  → 「${before}」の直後で閉じられない。強調の内側に「${after}」まで入れるか、約物を外へ出す`);
      }
    });
  });
}

if (found) {
  console.error(`\n効かない強調が ${found} 件ある。`);
  process.exit(1);
}
console.log(`強調記法の問題なし (${files.length} ファイル)`);
