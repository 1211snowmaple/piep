// 散文の中に出てくる「数」を、抽出した事実から書き戻す。
//
// 数を手で書くと必ず古くなる。実際、方針書には「コマンド140個中、説明6個」と
// 書いてあったが、そのとき本当の数は159個中37個だった。index.md の「28の
// テーブル」も30が正しかった。三つとも、抽出器がすでに数えている値を手で
// 書き写したものである。
//
// そこで数の置き場を一つにする。文中では
//
//   コマンドは <!--stat:commands.total-->159<!--/stat--> 個ある
//
// と書く。HTML コメントは GitHub でも VitePress でも描画されないので、
// 読む人には「159」しか見えない。書き戻すのはこの道具で、`--check` は
// 食い違いを見つけて CI を落とす。
//
// 数を消すのではなく書き戻すのは、生の Markdown を GitHub で読む人がいる
// ため。プレースホルダのままでは、そこで意味が落ちる。

import { readFileSync, writeFileSync, existsSync, readdirSync } from "node:fs";
import { resolve, relative } from "node:path";

const here = import.meta.dirname;
const repo = resolve(here, "..", "..");
const build = resolve(repo, ".docs-build");

const check = process.argv.includes("--check");

// ------------------------------------------------------------------ 事実

const readJson = (p) => JSON.parse(readFileSync(p, "utf8"));

const contractPath = resolve(build, "contract.json");
const frontendPath = resolve(build, "frontend.json");
if (!existsSync(contractPath) || !existsSync(frontendPath)) {
  console.error("抽出結果が無い。先に `npm run extract` を実行すること。");
  process.exit(2);
}

const contract = readJson(contractPath);
const frontend = readJson(frontendPath);

const countMd = (dir) =>
  existsSync(resolve(repo, dir))
    ? readdirSync(resolve(repo, dir)).filter((f) => f.endsWith(".md")).length
    : 0;

/** 日本語の文字を一つも含まないか。コメントの言語を見分けるのに使う。 */
const NO_CJK = /^[^぀-ヿ㐀-鿿ｦ-ﾟ]*$/;

/**
 * まだ英語のまま残っている説明コメントの行数を数える。
 *
 * 目安であって厳密な数ではない。URL だけの行や `# Errors` のような見出しも
 * 混ざる。それでも移行がどこまで進んだかは見えるし、何より**手で書いた数が
 * 古くなるより正しい**。方針書に「約620行」と書いてあった数は、書いた時点の
 * 推計のまま更新されていなかった。
 */
function countEnglishDocComments(dir, exts, linePattern) {
  const root = resolve(repo, dir);
  if (!existsSync(root)) return 0;
  let count = 0;
  const walk = (d) => {
    for (const entry of readdirSync(d, { withFileTypes: true })) {
      const full = resolve(d, entry.name);
      if (entry.isDirectory()) {
        if (entry.name === "node_modules" || entry.name === "target") continue;
        walk(full);
        continue;
      }
      if (!exts.some((e) => entry.name.endsWith(e))) continue;
      for (const line of readFileSync(full, "utf8").split("\n")) {
        if (!linePattern.test(line)) continue;
        if (!/[A-Za-z]/.test(line)) continue;
        if (!NO_CJK.test(line)) continue;
        count += 1;
      }
    }
  };
  walk(root);
  return count;
}

/**
 * AI 補助の機能の数を `assist.rs` から読む。
 *
 * `current_prompt_version` の照合表が機能の一覧そのものなので、ここを数える。
 * 機能が増えたのに方針書が古いままなら `--check` が落ちる。
 */
function countAssistFeatures() {
  const file = resolve(repo, "src-tauri", "src", "assist.rs");
  if (!existsSync(file)) return 0;
  const text = readFileSync(file, "utf8");
  const table = text.match(/fn current_prompt_version[\s\S]*?\n}/);
  if (!table) return 0;
  return [...table[0].matchAll(/^\s*FEATURE_[A-Z_]+ =>/gm)].length;
}

/** e2e/readme-shots.spec.ts の `shots` 表から画面の数を読む。 */
function countScreens() {
  const spec = resolve(repo, "e2e", "readme-shots.spec.ts");
  if (!existsSync(spec)) return 0;
  const text = readFileSync(spec, "utf8");
  const table = text.match(/const shots:[\s\S]*?\n\];/);
  if (!table) return 0;
  return [...table[0].matchAll(/^\s*\["([a-z-]+)",/gm)].length;
}

/**
 * 使える名前の一覧。ここに無い名前を文中に書いたら落とす。
 * 黙って素通りさせると、綴り違いが「更新されない数」になって残る。
 */
const STATS = {
  "commands.total": contract.commands.length,
  "commands.documented": contract.commands.filter((c) => c.doc).length,
  "commands.undocumented": contract.commands.filter((c) => !c.doc).length,
  "commands.called": new Set(frontend.invocations.map((i) => i.name)).size,
  "events.total": new Set(contract.events.map((e) => e.name)).size,
  "tables.total": contract.tables.length,
  "policy.count": countMd("docs/policy"),
  "guide.count": countMd("docs/guide"),
  "screens.count": countScreens(),
  "assist.features": countAssistFeatures(),
  "comments.rust.english": countEnglishDocComments("src-tauri/src", [".rs"], /^\s*\/\/[/!]/),
  "comments.ts.english": countEnglishDocComments("src", [".ts", ".tsx"], /^\s*\*\s+\S/),
};

// ------------------------------------------------------------------ 書き戻し

/** 数を書き戻す対象。生成物 (`docs/reference/`) は元から数を持っているので含めない。 */
const TARGETS = [
  "README.md",
  "docs/index.md",
  ...["docs/guide", "docs/policy", "docs/plan"].flatMap((dir) =>
    existsSync(resolve(repo, dir))
      ? readdirSync(resolve(repo, dir))
          .filter((f) => f.endsWith(".md"))
          .map((f) => `${dir}/${f}`)
      : [],
  ),
];

const TOKEN = /<!--stat:([a-z.]+)-->(.*?)<!--\/stat-->/gs;

let stale = 0;
let written = 0;
let unknown = 0;

for (const rel of TARGETS) {
  const file = resolve(repo, rel);
  if (!existsSync(file)) continue;
  const before = readFileSync(file, "utf8");
  let changedHere = 0;

  const after = before.replace(TOKEN, (whole, name, current) => {
    if (!(name in STATS)) {
      unknown += 1;
      console.error(`${rel}: 知らない数の名前 "${name}"`);
      console.error(`  使えるのは: ${Object.keys(STATS).join(", ")}`);
      return whole;
    }
    const value = String(STATS[name]);
    if (current === value) return whole;
    changedHere += 1;
    if (check) {
      stale += 1;
      console.error(`${rel}: ${name} が "${current}" のまま。今は "${value}"`);
    }
    return `<!--stat:${name}-->${value}<!--/stat-->`;
  });

  if (!check && changedHere) {
    writeFileSync(file, after);
    written += changedHere;
    console.log(`${relative(repo, file)}: ${changedHere} 箇所を更新`);
  }
}

if (unknown) process.exit(1);

if (check) {
  if (stale) {
    console.error(
      `\n古い数が ${stale} 箇所ある。\`npm --prefix docs-tools run stats\` で書き戻すこと。`,
    );
    process.exit(1);
  }
  console.log(`数の記載は最新 (${Object.keys(STATS).length} 種を照合)`);
} else {
  console.log(written ? `${written} 箇所を書き戻した` : "数の記載は最新");
}
