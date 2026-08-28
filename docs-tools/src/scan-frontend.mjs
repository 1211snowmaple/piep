// フロント側の走査。`invoke("名前")` とイベント購読の呼び出し位置を集める。
//
// 正規表現にしないのは、呼び出しが二通りあるからである。`collectionApi.ts` の
// ように `invoke` を直接呼ぶ場所と、`eventBus.ts` の `subscribeTauriEvent` の
// ように包んで呼ぶ場所がある。構文木なら包み関数の定義もその JSDoc も一緒に取れる。
//
// TypeScript 5.9 を使う。アプリがビルドに使う TypeScript 7（ネイティブ移植版）の
// npm パッケージには従来の Compiler API が入っておらず、`import ts from
// "typescript"` で得られるのは `{ version }` だけだからである。7.0 は 5.9 の
// 移植で新しい構文は無いので、5.9 のパーサで同じソースを読める。

import ts from "typescript";
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";

/** `invoke` として扱う呼び出し名。 */
const INVOKE_NAMES = new Set(["invoke"]);

/** イベント購読として扱う呼び出し名。`eventBus.ts` の包みを含む。 */
const LISTEN_NAMES = new Set(["listen", "onTauriEvent", "subscribeTauriEvent"]);

function parseArgs() {
  const args = process.argv.slice(2);
  let repo = ".";
  let out = null;
  for (let i = 0; i < args.length; i += 1) {
    if (args[i] === "--repo") repo = args[++i];
    else if (args[i] === "--out") out = args[++i];
    else throw new Error(`知らない引数: ${args[i]}`);
  }
  return { repo: resolve(repo), out };
}

/** tsconfig を読んでプログラムを組む。型検査はしない（走査だけが目的）。 */
function createProgram(repoRoot) {
  const configPath = resolve(import.meta.dirname, "..", "tsconfig.docs.json");
  const raw = ts.readConfigFile(configPath, ts.sys.readFile);
  if (raw.error) throw new Error(ts.flattenDiagnosticMessageText(raw.error.messageText, "\n"));
  const parsed = ts.parseJsonConfigFileContent(raw.config, ts.sys, dirname(configPath));
  // テストは走査対象に戻す。invoke をテストから呼んでいる場所も契約の一部である。
  const roots = ts.sys
    .readDirectory(resolve(repoRoot, "src"), [".ts", ".tsx"], undefined, undefined)
    .filter((f) => !f.includes("/node_modules/"));
  return ts.createProgram(roots, { ...parsed.options, noEmit: true });
}

/**
 * ノードが宣言なら、その名前と本体を返す。囲みでなければ null。
 *
 * 親を辿らないのは、`ts.createProgram` が型検査を要求されるまで親ポインタを
 * 張らないからである。`node.parent` は undefined のままで、辿る方式は
 * 一件も取れない。降りながら囲みを持ち回るほうが確実で、束縛も要らない。
 */
function declarationAt(node, sf) {
  if (ts.isFunctionDeclaration(node) || ts.isMethodDeclaration(node)) {
    return node.name ? { name: node.name.getText(sf), node } : null;
  }
  if (ts.isVariableStatement(node)) {
    const d = node.declarationList.declarations[0];
    return d?.name ? { name: d.name.getText(sf), node } : null;
  }
  return null;
}

/** 宣言に付いた JSDoc の本文。無ければ null。 */
function jsDocOf(node) {
  // 変数宣言では JSDoc は VariableStatement 側に付く場合と、その中の
  // VariableDeclaration 側に付く場合がある。両方見る。
  const targets = [node];
  if (node && ts.isVariableStatement(node)) targets.push(node.declarationList.declarations[0]);
  for (const t of targets) {
    const docs = t?.jsDoc;
    if (!docs?.length) continue;
    // 連続した `/** */` は全部この宣言に紐付く。宣言のものは最後の一つだけで、
    // 前のものは置き場所を間違えた別の説明である。連結すると嘘になる。
    const text = commentText(docs[docs.length - 1].comment).trim();
    if (text) return text;
  }
  return null;
}

/** JSDoc の本文は文字列か、`{@link}` を含む場合はノードの配列で届く。 */
function commentText(comment) {
  if (!comment) return "";
  if (typeof comment === "string") return comment;
  return comment.map((c) => c.text ?? "").join("");
}

/** 呼び出し式の関数名。`invoke<T>(...)` や `api.invoke(...)` にも当てる。 */
function calleeName(expr) {
  if (ts.isIdentifier(expr)) return expr.text;
  if (ts.isPropertyAccessExpression(expr)) return expr.name.text;
  return null;
}

function main() {
  const { repo, out } = parseArgs();
  const program = createProgram(repo);
  const invocations = [];
  const subscriptions = [];
  // 宣言そのものの名前は「参照」に数えない。数えてしまうと、誰も使っていない
  // 薄皮でも「使われている」ことになり、`uncalled` が永久に発火しない。
  const declarationNames = new Set();
  const identifierNodes = [];

  for (const sf of program.getSourceFiles()) {
    if (sf.isDeclarationFile) continue;
    const file = relative(repo, sf.fileName).replace(/\\/g, "/");
    if (!file.startsWith("src/")) continue;

    const visit = (node, enclosing) => {
      if (ts.isIdentifier(node)) identifierNodes.push(node);
      const declared = declarationAt(node, sf);
      if (declared) {
        const nameNode = ts.isVariableStatement(declared.node)
          ? declared.node.declarationList.declarations[0]?.name
          : declared.node.name;
        if (nameNode) declarationNames.add(nameNode);
      }
      const here = declared ?? enclosing;
      if (ts.isCallExpression(node)) {
        const name = calleeName(node.expression);
        const first = node.arguments[0];
        if (name && first && ts.isStringLiteralLike(first)) {
          const { line } = sf.getLineAndCharacterOfPosition(node.getStart(sf));
          const record = {
            name: first.text,
            file,
            line: line + 1,
            caller: here?.name ?? null,
            doc: here ? jsDocOf(here.node) : null,
            isTest: /\.test\.tsx?$/.test(file),
          };
          if (INVOKE_NAMES.has(name)) invocations.push(record);
          else if (LISTEN_NAMES.has(name)) subscriptions.push(record);
        }
      }
      ts.forEachChild(node, (child) => visit(child, here));
    };
    visit(sf, null);
  }

  // その包み関数の名前が、宣言以外の場所に一度でも現れるか。
  const referenced = new Set();
  for (const node of identifierNodes) {
    if (!declarationNames.has(node)) referenced.add(node.text);
  }
  for (const record of [...invocations, ...subscriptions]) {
    // 包みの外（画面のコード）から直に呼ばれているものは、常に生きている。
    record.callerUsed = record.caller === null || referenced.has(record.caller);
  }

  const sortKey = (a, b) => a.name.localeCompare(b.name) || a.file.localeCompare(b.file) || a.line - b.line;
  invocations.sort(sortKey);
  subscriptions.sort(sortKey);

  const result = { invocations, subscriptions };
  console.error(
    `invoke ${invocations.length} 箇所 (${new Set(invocations.map((i) => i.name)).size} 種) / ` +
      `購読 ${subscriptions.length} 箇所 (${new Set(subscriptions.map((i) => i.name)).size} 種)`,
  );

  const json = JSON.stringify(result, null, 2);
  if (out) {
    mkdirSync(dirname(resolve(out)), { recursive: true });
    writeFileSync(resolve(out), json + "\n");
    console.error(`書き出した: ${out}`);
  } else {
    process.stdout.write(json);
  }
}

main();
