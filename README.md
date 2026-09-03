<div align="center">

<img src="piep_lockup.svg" alt="piep" width="248">

**pixiv と FANBOX の小説・記事を、自分のPCに丸ごと残して読むためのデスクトップアプリ。**

保存・検索・読書・書き出しまでを、ネットにつながっていなくても完結させます。

[![Release](https://img.shields.io/github/v/release/1211snowmaple/piep?include_prereleases&sort=semver)](https://github.com/1211snowmaple/piep/releases)
[![Quality](https://github.com/1211snowmaple/piep/actions/workflows/quality.yml/badge.svg)](https://github.com/1211snowmaple/piep/actions/workflows/quality.yml)
![Platform](https://img.shields.io/badge/platform-Windows-lightgrey)

<img src="docs/screenshots/library-light.png" alt="piepのライブラリ画面" width="880">

</div>

---

## piepでできること

- **保存する** — アプリに内蔵したブラウザでpixiv・FANBOXを開き、そのページから作品・シリーズ・作者をまとめて取り込みます。本文、画像、タグ、公開日などのメタデータをローカルに保存します。
- **探す** — 1万件を超えても待たされない全文検索。表記ゆれ（かな・カナ・ローマ字・部分一致）を吸収します。覚えている内容を言葉で書いて探す「言葉で探す」は、意味の近さで別に探します（設定で意味索引を作ってから使えます）。
- **読む** — 明朝／ゴシック、文字サイズ、行間、背景色を選べるリーダー。読みかけの位置は自動で覚えます。
- **直す** — 取り込んだ本文をブロック単位で編集。元データはそのまま残り、編集は別リビジョンとして保存されます。
- **追う** — 監視している作品の改稿、作者の新作、シリーズの続編を一つのジョブで確認し、選んだものだけ保存します。
- **まとめる** — 前後編、非公式の連載、pixivとFANBOXに分かれた作品をコレクションにまとめます。棚から確かなまとまりを探し、すでにあるコレクションへあとから合う作品も探せます。
- **持ち出す** — 選んだ作品をEPUB 3として書き出し。テンプレートと画像最適化を指定できます。
- **守る** — ライブラリ全体のバックアップ（マニフェスト＋分割ZIP）と検証付き復元、保存フォルダーからの再取り込み、容量と索引の診断。

保存と検索は自分のPCの中だけで完結します。外部通信は、pixiv・FANBOXからの取得、
GitHub Releasesでの更新確認、利用者が明示的に設定したAI補助の宛先に限ります。
AI補助は既定で無効で、外部宛先はHTTPSと宛先ごとの同意が必要です。

---

## 画面

### ホーム

保存件数、使用容量、保存の推移、検索インデックスの状態がひと目で分かります。URLを貼れば、そのまま保存候補へ進めます。

<img src="docs/screenshots/home-light.png" alt="ホーム画面" width="880">

### ライブラリ

作品・作者・シリーズ・コレクションの4つの見方を切り替えられます。「すべて」「お気に入り」「読みかけ」「更新監視」の棚、絞り込み、並べ替え、保存した検索、ギャラリー／リスト表示、複数選択の一括操作に対応しています。

<img src="docs/screenshots/library-people-light.png" alt="作者・クリエイター一覧" width="880">

### 作品とリーダー

作品ページでは概要・本文・アセット・履歴・JSONを確認でき、そのまま読む、編集する、EPUBキューに入れる、アーカイブとして書き出すことができます。

<table>
<tr>
<td width="50%"><img src="docs/screenshots/work-light.png" alt="作品詳細"></td>
<td width="50%"><img src="docs/screenshots/reader-light.png" alt="リーダー"></td>
</tr>
</table>

ライトとダークの両方に対応しています。

<img src="docs/screenshots/reader-dark.png" alt="ダークテーマのリーダー" width="880">

### エディタ

見出しや段落をブロックとして並べ替え・書き換えでき、右側で仕上がりを確認しながら下書きを保存できます。

<img src="docs/screenshots/editor-light.png" alt="ブロックエディタ" width="880">

### 更新センター

「確認のみ」と「自動保存」を選び、同時実行数を決めて実行します。進行中のジョブは一時停止・再開・中止でき、失敗した分だけを後から再試行できます。

<img src="docs/screenshots/updates-light.png" alt="更新センター" width="880">

### EPUB Studio

キューに入れた作品を、テンプレートと画像最適化の設定でまとめて書き出します。

<img src="docs/screenshots/epub-light.png" alt="EPUB Studio" width="880">

### ライブラリ診断

容量、索引の完成度、実データでの検索速度、孤立したアセットを測ります。SQLiteの最適化と検索索引の最適化もここから実行できます。

<img src="docs/screenshots/diagnostics-light.png" alt="ライブラリ診断" width="880">

> [!NOTE]
> 掲載しているスクリーンショットはすべてデモデータです。作品名・作者名・本文・統計はこのリポジトリに含まれる架空のサンプル（`src/mocks/demoData.ts`）で、実在の作品や利用者のライブラリではありません。

---

## インストール

[Releases](https://github.com/1211snowmaple/piep/releases) から、Windows向けのインストーラー（`.msi` または `.exe`）をダウンロードしてください。

配布しているのはWindows版だけです。意味検索のGPU実行（DirectML）、内蔵ブラウザと認証（WebView2）、実際のウィンドウを起動するテストがいずれもWindows前提で、動作を確認できている環境がWindowsのみのためです。ソースからのビルドはTauriが対応する他のOSでも試せますが、動作は保証していません。

コード署名は行っていないため、初回起動時にWindowsの警告が出ることがあります。

### 動作環境

- Windows 10（64bit）以降
- WebView2ランタイム — Windows 11には標準で入っています。無い場合はインストーラーが導入します。
- 保存した作品のぶんの空き容量。意味検索を使う場合はモデル用に数百MB。

### 使いはじめ

1. **設定 → サービス接続** から、pixivとFANBOXにログインします。
2. **保存** を開き、内蔵ブラウザで保存したいページを表示して「候補を取得」を押します。
3. 保存する項目を選んで実行すると、**ライブラリ** に並びます。あとは読むだけです。

### データの置き場所

すべて `%APPDATA%\com.hiron.piep` の下にあります（実際のパスは **設定 → ローカルライブラリ** に表示されます）。

| 場所 | 中身 |
| --- | --- |
| `piep.db` | 作品のメタデータ、タグ、履歴、更新監視の設定 |
| `downloads/` | 本文のJSONと画像。取得元・作品ごとのフォルダー |
| `collection-covers/` | 選んだコレクション表紙の管理コピー。バックアップ対象 |
| `templates/` | EPUBテンプレート（テンプレートスタジオで編集したもの） |
| `delete-staging/` | 削除をクラッシュ安全に完了するための一時隔離。次回起動時に復旧または完了 |

同じ画面から、マニフェストと分割ZIPとしてバックアップを書き出せます。復元は取り込む前に中身を検査します。

---

## 開発

### 必要なもの

- Node.js 24.x
- Rust 1.97.1（CI が固定している版。`clippy` を `-D warnings` で通すので、
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

READMEのスクリーンショットは次のコマンドで撮り直せます。

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

### ドキュメント

設計の方針は `docs/policy/` にあります。「いつ調べたか」ではなく「今どうなって
いるか」を書いた文書で、更新はソースの変更と同じコミットで行います。

- [piepとは何か](docs/policy/01-what-piep-is.md) — 何をして、何をしないか
- [設計原則](docs/policy/02-principles.md) — 迷ったときに立ち返る規則
- [アーキテクチャ](docs/policy/03-architecture.md) — 層と責務、依存の向き
- [データの持ち方](docs/policy/04-data.md) — 原本不変、スキーマ、コレクション
- [画面の方針](docs/policy/05-frontend.md) — 状態の置き場、スクロール、配色
- [Rust側の方針](docs/policy/06-backend.md) — コマンド設計、検索、更新監視
- [取得の方針](docs/policy/07-acquisition.md) — 認証、レート、規約
- [EPUBの方針](docs/policy/08-epub.md) — 何に準拠し、何を保証しないか
- [品質の担保](docs/policy/09-quality.md) — テストの層、CI、ドリフト検査
- [ドキュメントの作り](docs/policy/10-documentation.md) — コメント規約と生成の仕組み

API リファレンス（フロント・Rust・IPC契約・スキーマ）はソースから生成しています。

```bash
npm --prefix docs-tools ci
npm --prefix docs-tools run docs:build
```

---

## 注意事項

- piepは、自分が閲覧できる作品を自分用に手元へ残すための道具です。保存したデータの権利は各作者にあります。再配布や公開はしないでください。
- 利用にあたっては、pixivおよびFANBOXの利用規約に従ってください。短時間に大量の取得を行わないよう、同時実行数は控えめな既定値にしています。
- 本ソフトウェアは無保証で提供されます。大切なライブラリは、**設定 → ローカルライブラリ** のバックアップ機能で定期的に保存してください。

## リンク

- [ダウンロード（Releases）](https://github.com/1211snowmaple/piep/releases)
- [不具合の報告・要望（Issues）](https://github.com/1211snowmaple/piep/issues)
