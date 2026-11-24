# 進め方の取り決め

コミット・タグ・push の判断をその都度考え直さずに済むよう、決めたことを書き残す。
作業の流れの全体と、どの AI にも効く決まりは [AGENTS.md](AGENTS.md) にある。ここはその細目。

## コミットの粒度

**ひとつのコミットは、ひとつの機能または目的でまとまっていること。** 作業の順番や
時間で切らない。「午前中の作業」ではなく「コレクションを入れる」で切る。

判断の目安:

- そのコミットだけを読んで、何がどう変わるか説明できるか
- そのコミットの時点でビルドが通り、テストが緑か
- 説明に「そして」が3つ以上必要なら、たぶん分けるべき
- 逆に、1行の修正が単独で意味を持たないなら、関係する変更へ含める

試行錯誤は履歴に残さない。同じ箇所を何度も直したものは、最後にひとつへまとめる。

## コミットメッセージ

**日本語で書く。** 件名は体言止めにせず、何をしたかを言い切る。

```
コレクションを入れ、スキーマとリリース基盤を整える

作者・公式シリーズ・取得元をまたいで、読む単位を自分でまとめられるようにした。

- コレクション: ライブラリのタブとして一覧、作者詳細にも所属タブ
- 順序: 題名の話数や公式順といった明示的な手掛かりだけで読む順を決める
- スキーマ: 移行分岐を単一定義へ畳み、バージョンを 1 に振り直す
```

- 1行目: 何をしたか。50字前後。接頭辞（feat: など）は付けない
- 2行目: 空行
- 3行目以降: なぜそうしたか。読む人が後で判断を追えるだけの理由を書く
- 箇条書きは「領域: 内容」の形で揃える
- 直した不具合は、症状・原因・対処の順で書く。原因が推測なら推測と書く

書かないこと: 作業ログ、試した順番、AI が書いたという注記（道具が決まって付ける署名行 `Co-Authored-By:` は付けてよい）。

## いつコミットするか

- ひとつの機能または目的が**動く状態で**完結したとき
- ローカルのゲート（下記の `tools/check.ps1`）が緑になってから
- 途中で別の作業に移る必要が出たときは、中途半端な状態をコミットせず、まず
  まとまりのある単位まで進めるか、切り分けて先に片付ける

## ローカルのゲート

コミット前に通す。入口は一つで、CI も同じスクリプトを呼ぶ。

```powershell
pwsh tools/check.ps1
```

既定で流れる段は `frontend`（型検査・lint・画面の単体試験）、`rust`（nextest・clippy・rustfmt・
EPUBCheck）、`docs`（画面と Rust の境界の突き合わせ・方針書の数・文書サイト）。変えた物に応じて足す。

| 変えた物 | 足す段 |
|---|---|
| 画面 | `-Stage frontend,rust,docs,visual` |
| 携帯の画面・携帯の Rust | `android`（画面の組み立てと試験、携帯の Rust の試験・clippy・rustfmt） |
| 携帯の Rust・署名・Gradle | `apk` も（重い。鍵なしで組めることだけを見る） |

版は `rust-toolchain.toml`（Rust。携帯は `android/rust-toolchain.toml`）と Node.js 24.x にそろえる。`cargo-nextest` と、`docs-tools` の
依存（`npm --prefix docs-tools ci`）が要る。

`docs` の段はフロントと Rust の境界を突き合わせる。`invoke("名前")` に対応するコマンドが無い、
`generate_handler!` への登録を忘れた、といった**実行するまで気付かない食い違い**をここで捕まえる。

コマンドや画面を足したときは、方針書（`docs/policy/`）が古くなっていないかも見る。
**方針書はソースの変更と同じコミットで直す。** 別のコミットに回すと、だいたいそのまま忘れられる。

画面を変えたときは、加えて実際に動かして確認する。視覚の基準（`e2e/__screenshots__/`）は手元で
撮り直してよい（CI と画素まで一致する）。撮り直すときは spec を指定する。丸ごと走らせると
`readme-shots` も動いて `docs/screenshots/*` を書き換える。

```powershell
npx playwright test e2e/visual-regression.spec.ts --update-snapshots
```

## いつ main へ push するか

作業ブランチから Pull Request を作り、`main` は **CI が緑であることを確認した状態だけ**を指す。
`main` へ直接 push しない。廃止済みの `develop` ブランチを中継しない。

1. 作業ブランチを push して Pull Request を作る。本文は `.github/pull_request_template.md` に沿う
2. Pull Request の CI が**全部**緑になるのを**実際に確認する**。いつも走るのは Quality の3つ
   （`checks` / `visual-regression` / `native-window-smoke`）と Docs の `contract`。触った場所に
   応じて Storage と Android も走る
3. 緑を確認し、利用者の了承を得てから `main` へマージする
4. `main` の同じジョブが、そのマージコミットで緑になったことを確認する

手元が緑でも CI が緑とは限らない。Node のバージョン差、並列度、実行環境で結果は変わる。
**手元の結果だけを根拠に「CI も通る」と言わない。**

緑を1回見ただけで「直った」と判断しない。断続的な失敗を疑うときは、同じ条件で
複数回確認してから結論を出す。

### CI の実行役

重いジョブ（Quality の `checks`・`visual-regression`、Android の APK、リリースの組み立て）は利用者の PC の
self-hosted runner（ラベル `snow`、2台）で動く。組み立ての途中経過が
残るので、キャッシュの復元なしで数分で終わる。

- よそのフォークからの PR は GitHub のランナーで動く。外部の人の PR のワークフローは承認が要る設定にしてある
- PC を長く止めるときは、リポジトリの変数 `CI_RUNNER` を `hosted` にすると全部 GitHub のランナーへ戻る
  （戻すときは `self`）。止まっている間に積まれたジョブは、PC が起きるまで待つ
- 窓を開く `native-window-smoke` は、使っている画面に割り込むのでいつも GitHub のランナー
- 実行役の上では利用者の rustup・npm・SDK がそのまま使われる。workflow から既定の版を変えない

## リリース

デスクトップ版・単独サーバー・Android 版は、**一つのタグ `vX.Y.Z`** に一緒に載る。

### 版

機能の節目ごとに minor を上げる。patch は不具合修正だけのリリースに使う。1.0.0 は、スキーマと
画面が数リリース跨いで安定してから。

- デスクトップ版は毎回5か所を揃える：`package.json` / `package-lock.json` / `src-tauri/Cargo.toml` /
  `src-tauri/tauri.conf.json` / `src/lib/version.ts`（`cargo check` で `Cargo.lock` も追随させる）
- 携帯は、**携帯に関わる物（`android/`・`shared/`・`crates/`。説明の `.md` は除く）が前に APK を出した版
  から変わったときだけ**、同じ版にする：`android/package.json` / `android/package-lock.json` /
  `android/src-tauri/Cargo.toml` / `android/src-tauri/Cargo.lock` / `android/src-tauri/tauri.conf.json`。
  変わっていない回に上げると、中身が同じでも携帯に更新の通知が出る。判断は Release が
  `tools/android-release-plan.ps1` で確かめ、上げ忘れていれば止まる
- 版を上げるコミットには、`npx playwright test readme-shots` で撮り直した `docs/screenshots/*` も入れる
  （版の表示が焼き込まれる）

### 手順

1. 版を上げる PR を出し、CI の緑と利用者の了承を見てから `main` へマージする
2. マージできたこと（PR の状態・`main` の先頭・そのコミットの版）を確かめてから、**マージコミット**に
   注釈付きタグを打って push する（`git tag -a vX.Y.Z <コミット> -m "piep vX.Y.Z"`）。前の段が失敗したまま
   次へ進まない
3. Release ワークフローが走る。組み立ては品質の確認を待たずに始まり、同じコミットの `main` の Quality と
   Docs の `contract` が緑になってから下書きを作る。Docs は `cancel-in-progress` なので、直後に別の PR が
   入ると取り消されてリリースが止まる。そのときは `contract` のジョブだけ再実行する
4. 下書きを確かめる。載る物は8つ：`piep_X.Y.Z_x64-setup.exe` とその `.sig`、`latest-v2.json`、
   `piep-server-X.Y.Z-windows-x64.zip` とその `.sig`、`latest-server.json`、
   `piep-android-vA.B.C-arm64.apk`、`latest-android.json`。単独サーバーの zip の中身の一覧は前の版と
   比べる（増えると前の版の自己更新が受け取らない）
5. リリースノートを書く（下の「リリースノート」）。本文は `gh release edit vX.Y.Z --notes-file <ファイル>` で
   入れ、そのあと `gh release view vX.Y.Z --json tagName,isDraft` でタグ名が `vX.Y.Z` のままか確かめる
6. 利用者の了承を得てから公開する（`gh release edit vX.Y.Z --draft=false`）。公開後、
   `releases/latest` が新しい版を指し、インストーラーの URL が 200 を返すことを確かめる

公開済みの版は作り直さない。アプリ内更新は版の番号で比べるので、同じ番号で中身を替えても届かない。
足したい物は patch で出す。過去の節目へ後からタグを足さない。

## 署名鍵

配布物の署名鍵は二つある。どちらも GitHub の `release` Environment の Secrets にあり、`v*` のタグからしか
読めない。秘密鍵の所在や保護方法は公開資料に記載しない。

| Secret | 用途 |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | デスクトップ版のアプリ内更新と単独サーバーの自己更新の署名。公開鍵は `src-tauri/tauri.conf.json` の `plugins.updater.pubkey` |
| `ANDROID_KEYSTORE_BASE64` / `ANDROID_KEYSTORE_PASSWORD` / `ANDROID_KEY_ALIAS` / `ANDROID_KEY_PASSWORD` | Android の APK の署名（[android/RELEASING.md](android/RELEASING.md)） |

- 登録と入れ替えは利用者がする。`gh secret set <名前> --env release --repo 1211snowmaple/piep` は値を
  端末で尋ねるので、パスワードを引数や履歴に残さずに済む
- Secrets は書き込み専用で、あとから取り出せない。鍵とパスワードの控えはリポジトリとも GitHub とも
  別の場所に置く
- **鍵を作り直すと、すでに配ったインストールは新しい鍵で署名した更新を受け取れない。** パスワードを
  付け替えるだけなら鍵の対は変わらないので、作り直さない
- 作り直すときは `npx tauri signer generate -f -w <ファイル>`。`npm run tauri signer generate` は `-w` を
  npm の `--workspace` として食べてしまう。公開鍵の差し替えと Secrets の登録し直しが要る

## リリースノート

**読む人は、使う人であって作った人ではない。** 何が使えるようになったかと、操作を
誤ると困ることだけを書く。

書くこと:

- 主な機能。見出しひとつに箇条書き数行まで
- 動作が変わって困りうること（データが消える、設定が引き継がれない等）

書かないこと:

- 内部の変更（モジュール分割、SQL の書き換え、テストの実行方法）
- 検証の内訳や件数
- 調整の経緯や、そこへ至った理由

目安は 20 行以内。コミット本文には理由を尽くして書き、リリースノートには結果だけを
残す。読む相手が違う。

## 履歴を書き換えるとき

`main` を force-push する前に、必ず退避を取ってリモートへも押しておく。

```bash
git branch backup/pre-rewrite-$(date +%Y-%m-%d) HEAD
git push origin backup/pre-rewrite-$(date +%Y-%m-%d)
```

書き換え後は、最終ツリーが書き換え前と一致することを確認する。

```bash
git rev-parse <new-head>^{tree} <old-head>^{tree}
```
