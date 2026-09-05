<div align="center">

<img src="piep_lockup.svg" alt="piep" width="248">

**pixiv と FANBOX の小説・記事を、自分のPCに丸ごと残して読むためのデスクトップアプリ。**

保存・検索・読書・書き出しまでを、ネットにつながっていなくても完結させます。

[![Release](https://img.shields.io/github/v/release/1211snowmaple/piep?include_prereleases&sort=semver)](https://github.com/1211snowmaple/piep/releases)
[![Quality](https://github.com/1211snowmaple/piep/actions/workflows/quality.yml/badge.svg)](https://github.com/1211snowmaple/piep/actions/workflows/quality.yml)
![Platform](https://img.shields.io/badge/platform-Windows-lightgrey)

**[ドキュメント](https://1211snowmaple.github.io/piep/)** ・
[使いはじめる](https://1211snowmaple.github.io/piep/guide/02-getting-started) ・
[画面の説明](https://1211snowmaple.github.io/piep/guide/03-screens) ・
[ダウンロード](https://github.com/1211snowmaple/piep/releases)

<img src="docs/screenshots/library-light.png" alt="piepのライブラリ画面" width="880">

</div>

---

## できること

**保存する** — 内蔵ブラウザで開いたページから、作品・シリーズ・作者をまとめて取り込む。
**探す** — 1万件を超えても待たされない全文検索と、意味で探す「言葉で探す」。
**読む** — 縦書き／横書き、目次、しおり、本文内検索。読みかけは行単位で覚える。
**直す** — 本文をブロック単位で編集。元データは残り、編集は別リビジョンになる。
**追う** — 改稿・新作・続編を一つのジョブで確認し、選んだものだけ保存する。
**まとめる** — 前後編や、サービスに分かれた続き物をコレクションにまとめる。
**持ち出す** — EPUB 3 として書き出す。
**守る** — バックアップと検証付き復元、容量と索引の診断。

保存と検索は自分のPCの中だけで完結します。外部通信は、pixiv・FANBOXからの取得、
GitHub Releasesでの更新確認、利用者が明示的に設定したAI補助の宛先に限ります。

→ [できることの詳しい説明](https://1211snowmaple.github.io/piep/guide/01-what-it-does)

## インストール

[Releases](https://github.com/1211snowmaple/piep/releases) から、Windows向けの
インストーラー（`.msi` または `.exe`）をダウンロードしてください。Windows 10
（64bit）以降が必要です。

配布しているのはWindows版だけです。意味検索のGPU実行（DirectML）、内蔵ブラウザと
認証（WebView2）、実際のウィンドウを起動するテストがいずれもWindows前提で、動作を
確認できている環境がWindowsのみのためです。

→ [はじめる](https://1211snowmaple.github.io/piep/guide/02-getting-started) ・
[データの置き場所とバックアップ](https://1211snowmaple.github.io/piep/guide/04-your-data)

---

## 開発

### 必要なもの

- Node.js 24.x
- Rust 1.98.1（CI が固定している版。`clippy` を `-D warnings` で通すので、
  新しい stable では新規の lint に当たって落ちることがある）
- OSごとのTauri前提環境（Windows: WebView2 / Linux: `libwebkit2gtk-4.1-dev` ほか）— [Tauri 2 の前提条件](https://v2.tauri.app/start/prerequisites/)

### コマンド

```bash
npm install            # 依存関係のインストール
npm run tauri dev      # デスクトップアプリを開発モードで起動
npm run dev            # フロントのみをブラウザで起動（デモデータで表示）
npm run check          # TypeScriptの型検査
npm test               # フロントの単体テスト
npm run test:visual    # 画面のビジュアル回帰テスト
npx --no-install tauri build    # 配布用ビルド
```

コミット前に通すものは [CONTRIBUTING.md](CONTRIBUTING.md#ローカルのゲート) にまとめてあります。

ドキュメントに載せるスクリーンショットは次のコマンドで撮り直します。CI でも同じ
撮影が走るので、説明した画面が開かなくなれば落ちます（絵そのものの比較はしていません）。

```bash
npx playwright test readme-shots --project=1440x900-light-200dpi --project=1440x900-dark-200dpi
```

### 技術構成

| 層 | 使っているもの |
| --- | --- |
| 画面 | React 19 / TypeScript / Mantine 9 / TanStack Query・Virtual |
| アプリ基盤 | Tauri 2（Rust）、内蔵WebViewによる取得と認証 |
| 保存 | SQLite（rusqlite + r2d2）、ローカルファイル |
| 検索 | Tantivy（全文・n-gram・読み正規化）、fastembed + ONNX Runtime（意味検索、DirectML対応） |
| 書き出し | minijinja テンプレート + ZIP、oxipng / zenjpeg / webp による画像最適化 |

### 品質チェック

`push` と `pull_request` のたびに、GitHub Actions で次を実行しています。

- 型検査、フロント単体テスト、`cargo test`、警告ゼロの `clippy`、`cargo fmt --check`
- EPUB の適合性を EPUBCheck（外部の権威）で確認
- 3つのウィンドウ幅 × ライト／ダーク × 3段階のDPIスケールでのビジュアル回帰
- 実際にビルドしたアプリを起動するネイティブウィンドウのスモークテスト
- フロントと Rust の境界のドリフト検査（`invoke` の名前・イベント・スキーマ）
- ドキュメントの検査（数の鮮度、画面の説明と実物のずれ、スクリーンショットの鮮度）

### ドキュメント

読む場所は [GitHub Pages](https://1211snowmaple.github.io/piep/) です。
読む人で三つに分かれています。

| | 誰のためか | 場所 |
| --- | --- | --- |
| [使う](https://1211snowmaple.github.io/piep/guide/01-what-it-does) | piep を入れた人 | `docs/guide/` |
| [つくり](https://1211snowmaple.github.io/piep/policy/01-what-piep-is) | 次に手を入れる人 | `docs/policy/` |
| [契約](https://1211snowmaple.github.io/piep/reference/ipc) | 境界を触る人 | `docs/reference/`（生成物） |

書き方の規則は [ドキュメントの作り](docs/policy/10-documentation.md) にあります。

```bash
npm --prefix docs-tools ci
npm --prefix docs-tools run docs:build   # 生成しなおす
npm --prefix docs-tools run docs:check   # 検査だけ
```

---

## 注意事項

- piepは、自分が閲覧できる作品を自分用に手元へ残すための道具です。保存したデータの権利は各作者にあります。再配布や公開はしないでください。
- 利用にあたっては、pixivおよびFANBOXの利用規約に従ってください。短時間に大量の取得を行わないよう、同時実行数は控えめな既定値にしています。
- 本ソフトウェアは無保証で提供されます。大切なライブラリは、**設定 → ローカルライブラリ** のバックアップ機能で定期的に保存してください。
- 掲載しているスクリーンショットはすべてデモデータです（`src/mocks/demoData.ts`）。実在の作品や利用者のライブラリではありません。

## リンク

- [ドキュメント](https://1211snowmaple.github.io/piep/)
- [ダウンロード（Releases）](https://github.com/1211snowmaple/piep/releases)
- [不具合の報告・要望（Issues）](https://github.com/1211snowmaple/piep/issues)
