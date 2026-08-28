// 抽出した事実を Markdown へ整形し、同時に両側の食い違いを検査する。
//
// 抽出（ipc-extract と scan-frontend）は事実を集めるだけで判断をしない。
// 判断はここに集めてある。そうしておくと「事実は合っているが検査が厳しすぎる」
// といった調整を、抽出器に触らずにできる。

import { readFileSync, writeFileSync, mkdirSync, existsSync } from "node:fs";
import { dirname, resolve } from "node:path";

const REPO_URL = "https://github.com/1211snowmaple/piep/blob/main";

function parseArgs() {
  const args = process.argv.slice(2);
  const opt = { build: ".docs-build", out: "docs/reference", repo: ".", check: false };
  for (let i = 0; i < args.length; i += 1) {
    if (args[i] === "--build") opt.build = args[++i];
    else if (args[i] === "--out") opt.out = args[++i];
    else if (args[i] === "--repo") opt.repo = args[++i];
    else if (args[i] === "--check") opt.check = true;
    else throw new Error(`知らない引数: ${args[i]}`);
  }
  return opt;
}

const read = (p) => JSON.parse(readFileSync(p, "utf8"));
const src = (file, line) => `[${file}:${line}](${REPO_URL}/${file}#L${line})`;
/**
 * 表のセルに入れる。改行と縦棒を潰し、山括弧を実体参照へ逃がす。
 *
 * 山括弧を逃がすのは、この Markdown を VitePress が Vue のテンプレートとして
 * 通すからである。`///` に `<pre>` や `Vec<T>` と書かれていると、閉じタグの
 * 無い要素と見なされてビルドが落ちる。
 */
const cell = (s) =>
  (s ?? "")
    .replace(/\|/g, "\\|")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/\n+/g, " ")
    .trim();
const code = (s) => (s ? `\`${s}\`` : "");

// ---------------------------------------------------------------- 検査

/**
 * 両側を突き合わせる。返すのは所見の配列で、`level` が `error` のものが
 * 一つでもあれば CI を落とす。
 */
function inspect(contract, frontend) {
  const findings = [];
  const defined = new Map(contract.commands.map((c) => [c.name, c]));
  const registered = new Set(contract.registered);
  const invokedBy = new Map();
  for (const i of frontend.invocations) {
    if (!invokedBy.has(i.name)) invokedBy.set(i.name, []);
    invokedBy.get(i.name).push(i);
  }

  // 1. フロントが呼んでいるのに、そんなコマンドが無い。実行時に必ず失敗する。
  for (const [name, sites] of invokedBy) {
    if (!defined.has(name)) {
      findings.push({
        level: "error",
        code: "invoke-undefined",
        message: `フロントが invoke("${name}") を呼んでいるが、その名前のコマンドが Rust 側に無い`,
        detail: sites.map((s) => `${s.file}:${s.line}`).join(", "),
      });
    }
  }

  // 2. 定義されているのに登録されていない。呼んでも届かない。
  for (const c of contract.commands) {
    if (!registered.has(c.name)) {
      findings.push({
        level: "error",
        code: "unregistered",
        message: `${c.name} は定義されているが generate_handler! に登録されていない`,
        detail: `${c.file}:${c.line}`,
      });
    }
  }

  // 3. 登録されているのに定義が見つからない。綴り違いか、別の場所にある。
  for (const name of registered) {
    if (!defined.has(name)) {
      findings.push({
        level: "error",
        code: "registered-undefined",
        message: `generate_handler! に ${name} があるが、commands/ に定義が見つからない`,
        detail: "src-tauri/src/lib.rs",
      });
    }
  }

  // 4. 誰も呼んでいないコマンド。意図的な場合もあるので警告に留める。
  //
  // **薄皮そのものを「呼び出し」に数えない。** `services/` の包み関数は
  // 必ず `invoke("名前")` を1回含むので、包みが誰からも使われていなくても
  // 呼び出しは1件あることになる。それではこの規則は永久に発火しない。
  // 実際、16 個のコマンドが死んだ包みに守られて残っていた。
  const liveInvokedBy = new Map();
  for (const [name, sites] of invokedBy) {
    const live = sites.filter((s) => s.callerUsed !== false);
    if (live.length) liveInvokedBy.set(name, live);
  }
  for (const c of contract.commands) {
    if (!liveInvokedBy.has(c.name)) {
      findings.push({
        level: "warn",
        code: "uncalled",
        message: `${c.name} を呼ぶフロントのコードが無い`,
        detail: invokedBy.has(c.name)
          ? `${c.file}:${c.line}（包みはあるが、その包みを誰も使っていない: ${invokedBy
              .get(c.name)
              .map((s) => `${s.file}:${s.line}`)
              .join(", ")}）`
          : `${c.file}:${c.line}`,
      });
    }
  }

  // 5. イベントの送出と購読の対応。
  // テストの中の購読は除く。`eventBus.test.ts` は購読の仕組みそのものを試す
  // ために架空の名前を使っており、送出側が無いのは正しい。
  const emitted = new Set(contract.events.map((e) => e.name));
  const realSubs = frontend.subscriptions.filter((s) => !s.isTest);
  const subscribed = new Set(realSubs.map((s) => s.name));
  for (const name of subscribed) {
    if (!emitted.has(name)) {
      findings.push({
        level: "warn",
        code: "listen-unemitted",
        message: `"${name}" を購読しているが、Rust 側に送出箇所が無い`,
        detail: realSubs.filter((s) => s.name === name).map((s) => `${s.file}:${s.line}`).join(", "),
      });
    }
  }
  for (const name of emitted) {
    if (!subscribed.has(name)) {
      findings.push({
        level: "warn",
        code: "emit-unlistened",
        message: `"${name}" を送出しているが、購読するフロントのコードが無い`,
        detail: contract.events.filter((e) => e.name === name).map((e) => `${e.file}:${e.line}`).join(", "),
      });
    }
  }

  return findings;
}

/**
 * コメントの付いたコマンドの割合が、記録した基準より下がっていないか。
 *
 * 140 個すべてに一度で書くのは現実的でないので、現在値を基準として残し
 * 「下がらないこと」だけを強制する。触ったコマンドに書き足していけば上がる。
 */
function ratchet(contract, baselinePath) {
  const total = contract.commands.length;
  const documented = contract.commands.filter((c) => c.doc).length;
  const current = { documented, total };

  if (!existsSync(baselinePath)) {
    mkdirSync(dirname(baselinePath), { recursive: true });
    writeFileSync(baselinePath, JSON.stringify(current, null, 2) + "\n");
    return { current, findings: [], created: true };
  }

  const base = read(baselinePath);
  const findings = [];
  if (documented < base.documented) {
    findings.push({
      level: "error",
      code: "doc-coverage-regressed",
      message: `コマンドの説明が ${base.documented} 件から ${documented} 件へ減った`,
      detail: `基準: ${baselinePath}。意図した削除なら基準を更新すること`,
    });
  }
  return { current, base, findings, improved: documented > base.documented };
}

// ---------------------------------------------------------------- 整形

function renderIpc(contract, frontend) {
  const invokedBy = new Map();
  for (const i of frontend.invocations) {
    if (!invokedBy.has(i.name)) invokedBy.set(i.name, []);
    invokedBy.get(i.name).push(i);
  }
  const documented = contract.commands.filter((c) => c.doc).length;
  const modules = [...new Set(contract.commands.map((c) => c.module))].sort();

  const out = [];
  out.push("# IPC コマンド契約");
  out.push("");
  out.push("> この頁はソースから生成している。直接編集しても次の生成で消える。");
  out.push("> 内容を変えるには、Rust 側のコマンドか、その `///` を直す。");
  out.push("");
  out.push(
    `フロントは \`invoke("名前")\` で Rust の関数を呼ぶ。名前はただの文字列で、` +
      `型検査は効かない。この頁はその名前の一覧であり、両側の食い違いを検査した結果でもある。`,
  );
  out.push("");
  out.push("| | 件数 |");
  out.push("| --- | ---: |");
  out.push(`| 定義されたコマンド | ${contract.commands.length} |`);
  out.push(`| \`generate_handler!\` に登録 | ${contract.registered.length} |`);
  out.push(`| フロントから呼ばれている | ${invokedBy.size} |`);
  out.push(`| 説明 (\`///\`) がある | ${documented} |`);
  out.push("");
  out.push("引数のうち `AppHandle` や `State` は Tauri が差し込むので、フロントからは渡さない。");
  out.push("渡す名前は Tauri 2 が camelCase へ変換したものである（`source_id` → `sourceId`）。");
  out.push("");

  for (const mod of modules) {
    const cmds = contract.commands.filter((c) => c.module === mod);
    out.push(`## ${mod}`);
    out.push("");
    out.push(`\`src-tauri/src/commands/${mod}.rs\` — ${cmds.length} 件`);
    out.push("");
    out.push("| コマンド | 渡す引数 | 返り値 | 呼び出し元 | 説明 |");
    out.push("| --- | --- | --- | --- | --- |");
    for (const c of cmds) {
      const args = c.args.filter((a) => !a.injected);
      const argText = args.length
        ? args.map((a) => `${code(a.js_name)}: ${code(a.ty)}`).join("<br>")
        : "—";
      const ret = c.returns.ok ? code(c.returns.ok) : code(c.returns.raw);
      const callers = invokedBy.get(c.name) ?? [];
      const callerText = callers.length
        ? callers.map((s) => `${s.caller ? code(s.caller) : ""} ${src(s.file, s.line)}`).join("<br>")
        : "**なし**";
      out.push(
        `| **${c.name}**<br>${src(c.file, c.line)} | ${argText} | ${ret} | ${callerText} | ${cell(c.doc) || "—"} |`,
      );
    }
    out.push("");
  }
  return out.join("\n") + "\n";
}

function renderEvents(contract, frontend) {
  const out = [];
  out.push("# イベント契約");
  out.push("");
  out.push("> この頁はソースから生成している。");
  out.push("");
  out.push("Rust が `.emit(\"名前\", 値)` で送り、フロントが `subscribeTauriEvent` で受ける。");
  out.push("コマンドと違い、送る側と受ける側のどちらにも相手を指す型が無い。");
  out.push("");
  const names = [...new Set([...contract.events.map((e) => e.name), ...frontend.subscriptions.map((s) => s.name)])].sort();
  out.push("| イベント | 送出 (Rust) | 購読 (フロント) |");
  out.push("| --- | --- | --- |");
  for (const name of names) {
    const emits = contract.events.filter((e) => e.name === name);
    const subs = frontend.subscriptions.filter((s) => s.name === name);
    out.push(
      `| \`${name}\` | ${emits.length ? emits.map((e) => src(e.file, e.line)).join("<br>") : "**なし**"} | ` +
        `${subs.length ? subs.map((s) => src(s.file, s.line)).join("<br>") : "**なし**"} |`,
    );
  }
  out.push("");
  return out.join("\n") + "\n";
}

function renderSchema(contract) {
  const out = [];
  out.push("# データベーススキーマ");
  out.push("");
  out.push("> この頁はソースから生成している。");
  out.push("");
  out.push(
    "`src-tauri/src/database/schema.rs` が作るテーブル。**今のコードが作る姿**であって、" +
      "利用者の手元にあるデータベースの実際の姿ではない。試験用に作る旧世代のテーブルは載せていない。",
  );
  out.push("");
  out.push(`テーブル ${contract.tables.length} 件。`);
  out.push("");
  for (const t of contract.tables) {
    out.push(`## ${t.name}`);
    out.push("");
    out.push(src(t.file, t.line));
    out.push("");
    out.push("| 列 | 定義 |");
    out.push("| --- | --- |");
    for (const c of t.columns) out.push(`| \`${c.name}\` | ${code(c.definition)} |`);
    out.push("");
  }
  return out.join("\n") + "\n";
}

// ---------------------------------------------------------------- 実行

function main() {
  const opt = parseArgs();
  const contract = read(resolve(opt.build, "contract.json"));
  const frontend = read(resolve(opt.build, "frontend.json"));

  const findings = inspect(contract, frontend);
  const baselinePath = resolve(import.meta.dirname, "..", "coverage-baseline.json");
  const cov = ratchet(contract, baselinePath);
  findings.push(...cov.findings);
  // 改善したら基準を締め直す。書き戻さないと、増やした説明をあとから全部
  // 消しても検査は緑のまま - ラチェットが名前だけのものになる。
  // `--check`（CI）は生成物を書き換えないので、そこでは締め直さない。
  if (cov.improved && !opt.check) {
    writeFileSync(baselinePath, `${JSON.stringify(cov.current, null, 2)}
`);
  }

  const files = {
    "ipc.md": renderIpc(contract, frontend),
    "events.md": renderEvents(contract, frontend),
    "schema.md": renderSchema(contract),
  };
  mkdirSync(resolve(opt.out), { recursive: true });
  for (const [name, body] of Object.entries(files)) {
    writeFileSync(resolve(opt.out, name), body);
  }

  const errors = findings.filter((f) => f.level === "error");
  const warns = findings.filter((f) => f.level === "warn");

  console.log(`生成: ${Object.keys(files).join(", ")} → ${opt.out}`);
  console.log(
    `説明のあるコマンド: ${cov.current.documented}/${cov.current.total}` +
      (cov.created
        ? "（基準を新規作成した）"
        : cov.improved
          ? opt.check
            ? `（基準 ${cov.base.documented}/${cov.base.total} より改善。生成を1回走らせて基準を締め直すこと）`
            : `（基準を ${cov.base.documented}/${cov.base.total} から締め直した）`
          : ""),
  );
  console.log("");

  const show = (list, label) => {
    if (!list.length) return;
    console.log(`${label} ${list.length} 件`);
    const byCode = new Map();
    for (const f of list) {
      if (!byCode.has(f.code)) byCode.set(f.code, []);
      byCode.get(f.code).push(f);
    }
    for (const [code, items] of byCode) {
      console.log(`  [${code}] ${items.length} 件`);
      for (const f of items.slice(0, 10)) console.log(`    - ${f.message}  (${f.detail})`);
      if (items.length > 10) console.log(`    ... ほか ${items.length - 10} 件`);
    }
    console.log("");
  };
  show(errors, "エラー");
  show(warns, "警告");

  if (!errors.length && !warns.length) console.log("食い違いなし。");

  if (opt.check && errors.length) {
    console.error(`検査に失敗した。エラー ${errors.length} 件。`);
    process.exit(1);
  }
}

main();
