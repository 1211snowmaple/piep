# docs-tools

piep のドキュメントをソースから生成する道具一式。**アプリ本体とは依存を共有しない。**

## なぜ独立したパッケージなのか

アプリは TypeScript 7（ネイティブ移植版）でビルドしている。7 の npm パッケージには
従来の Compiler API が入っておらず、`lib/` にあるのは `tsc.js` のシムと
`version.cjs` だけである。`import ts from "typescript"` で得られるのは
`{ version: "7.0.2" }` に過ぎない。

フロント側の抽出器（`scan-frontend.mjs`）はその API を必要とするため、ここに
TypeScript 5.9 を隔離している。7.0 は 5.9 の Go 移植で新しい構文は追加されて
いないので、5.9 のパーサで同じソースを読める（`tsc --noEmit -p tsconfig.docs.json`
が型エラーゼロで通ることを確認済み）。

npm workspaces にしていないのは、アプリの `npm ci`（CI の品質ジョブ3本）に
この依存を持ち込まないためである。docs のジョブでだけ `npm ci` する。

## 使い方

```bash
npm --prefix docs-tools ci        # 初回のみ
npm --prefix docs-tools run docs:build
```

| コマンド | すること |
| --- | --- |
| `docs:build` | 生成をまとめて実行し、サイトを組む |
| `docs:check` | 生成せずに検査だけする（CI 用）。下の4つを順に走らせる |
| `contract` / `contract:check` | IPC・イベント・スキーマの契約書を生成／検査する |
| `stats` / `stats:check` | 散文に埋めた数を書き戻す／古くないか検査する |
| `check:screens` | 使い方の説明と、実際に撮っている画面のずれを見る |
| `check:emphasis` | 日本語で閉じない `**` を探す |
| `rustdoc` | `cargo doc` で Rust の HTML を生成する |

TypeDoc は外した。読む人がいないうえ、rustdoc と合わせて**サイトの中に別の見た目
と別の言語の島を二つ作っていた**。境界の契約は `reference/` に日本語で出ている。

## 構成

```
src/scan-frontend.mjs        TypeScript 5.9 の API で src/ を読み、
                             invoke("名") と購読の呼び出し位置＋JSDoc を JSON にする
src/render-contract.mjs      抽出結果を Markdown へ整形し、両側の食い違いを検査する
src/sync-stats.mjs           散文の <!--stat:名--> を抽出結果から書き戻す／検査する
src/check-guide-screens.mjs  使い方の説明と readme-shots.spec.ts の画面を突き合わせる
src/check-japanese-emphasis.mjs  日本語で閉じない強調記法を探す
src/assemble-site.mjs        docs/ をサイトの組み立て先へ運ぶ
src/copy-rustdoc.mjs         cargo doc の HTML を public/backend/ へ運ぶ
tsconfig.docs.json           アプリの tsconfig を 5.9 が解釈できる形へ翻訳する
coverage-baseline.json       コマンドの説明の付与率。下がったら CI が落ちる（ラチェット）
../tools/ipc-extract/        syn で src-tauri/src を読む Rust 側の抽出器
```

抽出（`scan-frontend` と `ipc-extract`）は事実を集めるだけで判断をしない。
判断は `render-contract` に集めてある。そうしておくと「事実は合っているが
検査が厳しすぎる」といった調整を、抽出器に触らずにできる。

## 検査していること

| 検査 | 水準 | 意味 |
| --- | --- | --- |
| `invoke-undefined` | エラー | フロントが呼ぶ名前の Rust 側コマンドが無い。実行時に必ず失敗する |
| `unregistered` | エラー | 定義済みだが `generate_handler!` に無い。呼んでも届かない |
| `registered-undefined` | エラー | 登録されているが定義が見つからない |
| `doc-coverage-regressed` | エラー | 説明の付いたコマンドが基準より減った |
| `uncalled` | 警告 | 誰も呼んでいないコマンド |
| `emit-unlistened` / `listen-unemitted` | 警告 | イベントの送出と購読が片側だけ |

手で書いた文書についても、二つ見ている。

| 検査 | 意味 |
| --- | --- |
| `stats:check` | 散文に書いた数がソースと食い違う。知らない名前を書いた場合も落ちる |
| `check:screens` | 撮っている画面に説明が無い／説明にある画面を撮っていない |

この二つがあるのは、**生成物だけを検査していると、人が書いた側が黙って古くなる**
からである。方針書は「コマンド140個中6個」と書いたまま実物から離れていた。

生成した Markdown と HTML は git で追跡しない。再生成できるものだからで、
読む場所は GitHub Pages である。
