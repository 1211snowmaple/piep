// cargo doc の出力を VitePress の public/ へ運ぶ。
//
// rustdoc は HTML を吐く。VitePress の Markdown には混ぜられないので、
// public/backend/ に丸ごと置いて静的資産として配る。相対リンクも検索も
// rustdoc が生成したまま動く。

import { cp, rm, mkdir, stat } from "node:fs/promises";
import { resolve, dirname } from "node:path";

const here = import.meta.dirname;
const from = resolve(here, "..", "..", "src-tauri", "target", "doc");
const to = resolve(here, "..", "site", "public", "backend");

try {
  await stat(from);
} catch {
  console.error(`${from} が無い。先に "npm run rustdoc" を実行すること。`);
  process.exit(1);
}

// 前回分を消してから入れる。消さないと、削除した項目の頁が残り続ける。
await rm(to, { recursive: true, force: true });
await mkdir(dirname(to), { recursive: true });
await cp(from, to, { recursive: true });

console.log(`Rust のドキュメントを ${to} へ複製した`);
