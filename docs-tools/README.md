# docs-tools

piep のドキュメントをソースから生成する道具一式。**アプリ本体とは依存を共有しない。**

## なぜ独立したパッケージなのか

アプリは TypeScript 7（ネイティブ移植版）でビルドしている。7 の npm パッケージには
従来の Compiler API が入っておらず、`lib/` にあるのは `tsc.js` のシムと
`version.cjs` だけである。`import ts from "typescript"` で得られるのは
`{ version: "7.0.2" }` に過ぎない。

TypeDoc はその API を必要とするため、ここに TypeScript 5.9 を隔離している。
7.0 は 5.9 の Go 移植で新しい構文は追加されていないので、5.9 のパーサで同じ
ソースを読める（`tsc --noEmit -p tsconfig.docs.json` が型エラーゼロで通ることを確認済み）。

npm workspaces にしていないのは、アプリの `npm ci`（CI の品質ジョブ3本）に
この依存を持ち込まないためである。docs のジョブでだけ `npm ci` する。

## 使い方

```bash
npm --prefix docs-tools ci        # 初回のみ
npm --prefix docs-tools run docs:build
```

| コマンド | すること |
| --- | --- |
| `docs:build` | 下の3つをまとめて実行する |
| `contract` | IPC・イベント・スキーマの契約書を生成する |
| `contract:check` | 同上。ただし食い違いがあれば終了コード 1 で落ちる（CI 用） |
| `typedoc` | `src/` の JSDoc から Markdown を生成する |
| `rustdoc` | `cargo doc` で Rust の HTML を生成する |

## 構成

```
src/scan-frontend.mjs    TypeScript 5.9 の API で src/ を読み、
                         invoke("名") と購読の呼び出し位置＋JSDoc を JSON にする
src/render-contract.mjs  抽出結果を Markdown へ整形し、両側の食い違いを検査する
tsconfig.docs.json       アプリの tsconfig を 5.9 が解釈できる形へ翻訳する
typedoc.json             TypeDoc の設定
coverage-baseline.json   コマンドの説明の付与率。下がったら CI が落ちる（ラチェット）
../tools/ipc-extract/    syn で src-tauri/src を読む Rust 側の抽出器
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

生成した Markdown と HTML は git で追跡しない。再生成できるものだからで、
読む場所は GitHub Pages である。
