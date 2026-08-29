# 品質の担保

## テストの層

| 層 | 道具 | 何を守るか |
|---|---|---|
| フロント静的検査 | tsc + oxlint | 型、hooks の依存、取りこぼした Promise |
| フロント単体 | Vitest + Testing Library | 純粋な道具（`lib/`）、画面の状態遷移、部品の振る舞い |
| Rust 単体・結合 | `cargo test` | スキーマ移行、検索、EPUB の組み立てと検証、解析 |
| 視覚回帰 | Playwright | 3つのウィンドウ幅 × ライト／ダーク × 3段階の DPI |
| ネイティブ | `e2e/native-window-smoke.ps1` | 実際にビルドしたアプリが起動して窓が出るか |
| 境界のドリフト | 自作の抽出器 | フロントと Rust の食い違い |

## 手元で通すもの

コミット前に、CI と同じ内容を先に確認する。**一覧の正本は
[CONTRIBUTING.md](https://github.com/1211snowmaple/piep/blob/main/CONTRIBUTING.md)
に一つだけ置く。** 二箇所に書いていたころ、こちらには `cargo fmt --check` と
境界のドリフト検査が入っておらず、方針書を信じた人だけが CI で落ちた。

画面を変えたときは、加えて実際に動かして確認する。視覚回帰は CI と同条件で:

```bash
npm run build && CI=1 npx playwright test
```

### linter は、意味のある規則だけを門にする

`oxlint` を入れているが、既定の全部を門にはしていない。React Compiler 由来の
助言（`set-state-in-effect` / `refs` / `immutability` / `purity` ほか）は、この
コードベースが意図して選んでいる書き方にも当たるので切ってある。**常に警告が
出る門は、誰も読まなくなる。**

残してあるのは、外すと実際に壊れるものだけ:

- `react-hooks/exhaustive-deps` — 依存の漏れは stale closure になる。入れた
  当日に7件見つかった
- `react-hooks/rules-of-hooks`
- `typescript/no-floating-promises` — `await` 忘れは失敗が消える

`unicorn/no-useless-spread` は切ってある。反復しながら同じコレクションを変更する
箇所（ライブ NodeList、Map のイテレータ）で複製は必須で、外すと壊れる。

### `--test-threads=1` の理由

Tantivy は一つのディレクトリに `IndexWriter` を一つしか許さない。アプリは
起動時にライブラリを一つ開いて排他ロックを取る。索引を使うテストを並列に走らせると
一つのプロセスに何十もの writer ができ、Windows では "Access is denied" で
コミットが失敗する。

**直列で走らせるのが実際の使われ方と一致する。** 20分のコンパイルに対して
20秒の追加で済む。

## 境界のドリフト検査

フロントと Rust は文字列で繋がっているだけで、型検査が効かない。

```bash
npm --prefix docs-tools run docs:check
```

| 検査 | 水準 | 意味 |
|---|---|---|
| `invoke-undefined` | エラー | フロントが呼ぶ名前の Rust 側コマンドが無い。**実行時に必ず失敗する** |
| `unregistered` | エラー | 定義済みだが `generate_handler!` に無い。呼んでも届かない |
| `registered-undefined` | エラー | 登録されているが定義が見つからない |
| `doc-coverage-regressed` | エラー | 説明の付いたコマンドが基準より減った |
| `uncalled` | 警告 | 誰も呼んでいないコマンド。意図的なこともある |
| `emit-unlistened` / `listen-unemitted` | 警告 | イベントの送出と購読が片側だけ |

**説明の付与率はラチェットにしてある。** すべてに一度で書くのは現実的でないので、
現在値を `docs-tools/coverage-baseline.json` に残し「下がらないこと」だけを強制する。
触ったコマンドに書き足していけば自然に上がる。**上がったら基準も締め直す** —
生成（`npm --prefix docs-tools run contract`）が現在値を書き戻す。書き戻さないと、
増やした説明をあとから全部消しても緑のままで、ラチェットが名前だけのものになる。

### 薄皮を「呼び出し」に数えない

`uncalled` は長らく一度も発火しなかった。`services/` の包み関数は必ず
`invoke("名前")` を1回含むので、**その包みを誰も使っていなくても呼び出しは1件ある**
ことになっていたからである。実際、16個のコマンドが死んだ包みに守られて残っていた。

いまは走査が「その包みの名前が、宣言以外の場所に現れるか」も見る。現れなければ
その `invoke` は数えない。警告に出ているものは、**使うか消すかを決めていない
入口**であって、放っておいてよい印ではない。

→ [ドキュメントの作り](10-documentation.md)

## CI

`push` と `pull_request` のたびに GitHub Actions で走る。**4つある。**

| ジョブ | ワークフロー | 内容 |
|---|---|---|
| `checks` | Quality（Windows） | 型検査、フロント単体テスト、`cargo test`、警告ゼロの `clippy`、`cargo fmt --check`、EPUBCheck |
| `visual-regression` | Quality（Windows） | 3幅 × 明暗 × 3 DPI の視覚回帰 |
| `native-window-smoke` | Quality（Windows） | 実際にビルドしたアプリを起動する |
| `contract` | Docs（Linux） | 境界のドリフト検査。フロントと Rust の食い違いを落とす |

Windows で走らせるのは、意味検索の GPU 実行（DirectML）、内蔵ブラウザと認証
（WebView2）、実ウィンドウを起動するテストがいずれも Windows 前提だからである。

## 手元が緑でも、CI が緑とは限らない

Node のバージョン差、並列度、ランナーのコア数で結果は変わる。

- **手元の結果だけを根拠に「CI も通る」と言わない**
- 緑を1回見ただけで「直った」と判断しない。断続的な失敗を疑うときは、同じ条件で
  複数回確認してから結論を出す
- `main` は **CI が緑であることを実際に確認した状態だけ**を指す
  （`gh run watch` で見る）。見るのは3つではなく**4つ**である

## 性能の主張は実測に基づく

「速い」と書くときは、何をどう測ったかを言えるようにする。診断画面が実データでの
検索速度と索引の完成度を測れるようになっているのはそのためである。

1万件を超えるライブラリが前提であり、性能の確認もその規模で行う。

## スクリーンショット

README のスクリーンショットは**デモデータ**（`src/mocks/demoData.ts`）で撮る。
実在の作品や利用者のライブラリは使わない。

```bash
npx playwright test readme-shots --project=1440x900-light-200dpi --project=1440x900-dark-200dpi
```
