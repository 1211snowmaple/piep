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

/**
 * 複製するもの。`public/` は生成器が直接ここへ書くので含めない。
 *
 * `screenshots` を運ぶのは、使い方の説明が `../screenshots/*.png` として
 * 貼っているからである。VitePress は Markdown からの相対パスを資産として
 * 束ねるので、同じ位置関係のまま置けばそれで解決する。
 */
const ITEMS = ["index.md", "guide", "policy", "plan", "reference", "screenshots"];

// .vitepress と public は消さない。前者は設定そのもの、後者は
// rustdoc が直接書き込む先である。
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

// アプリの印。ヘッダーの記章として使う。同じ絵を README も使っているので、
// 出どころはリポジトリ直下の一枚に保つ（複製を増やすと片方だけ古くなる）。
await mkdir(resolve(site, "public"), { recursive: true });
await cp(resolve(here, "..", "..", "piep_icon.svg"), resolve(site, "public", "piep-icon.svg"));

console.log(`組み立て先へ複製した: ${ITEMS.join(", ")}, piep-icon.svg → ${site}`);
