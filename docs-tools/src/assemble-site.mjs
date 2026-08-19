// 手書きの文書を、サイトを組み立てる場所へ運ぶ。
//
// なぜ docs/ の中で直接組まないのか。VitePress は Markdown を Vue の部品へ
// 変換し、その部品が `vue` を取り込む。Node は取り込み元のファイルから上へ
// node_modules を探すので、docs/ に置いたままだと docs-tools/node_modules に
// 届かず "Cannot find package 'vue'" で落ちる。
//
// 道具を隔離したまま解くには、組み立てを道具の側で行えばよい。docs/ は
// 内容だけを持ち、ここへ複製してから VitePress を回す。

import { cp, rm, mkdir } from "node:fs/promises";
import { existsSync } from "node:fs";
import { resolve } from "node:path";

const here = import.meta.dirname;
const docs = resolve(here, "..", "..", "docs");
const site = resolve(here, "..", "site");

/** 複製するもの。`public/` は生成器が直接ここへ書くので含めない。 */
const ITEMS = ["index.md", "policy", "reference"];

// .vitepress と public は消さない。前者は設定そのもの、後者は
// typedoc と rustdoc が直接書き込む先である。
for (const item of ITEMS) {
  await rm(resolve(site, item), { recursive: true, force: true });
}
await mkdir(site, { recursive: true });

for (const item of ITEMS) {
  const from = resolve(docs, item);
  if (!existsSync(from)) {
    console.error(`${item} が無いので飛ばす（先に contract を実行したか確認すること）`);
    continue;
  }
  await cp(from, resolve(site, item), { recursive: true });
}

console.log(`組み立て先へ複製した: ${ITEMS.join(", ")} → ${site}`);
