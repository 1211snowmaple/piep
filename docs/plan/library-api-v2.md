# 新ライブラリの認証付き資産 API

## 実装の境界

`server::v2::router` は接続情報、読書 bundle、資産 manifest、圧縮した部分の転送を提供する。
単独サーバーでは、設定で指定した棚を開いて既存の待ち受けにこの読取 API を載せる。
一覧と利用者状態の書き込みは未提供で、対応能力にも含めない。

管理側は新ライブラリを開き、永続化した同期 epoch と認可器を `Context` に渡す。
epoch は棚の復元・再作成時に変更し、要求や router の起動ごとには生成しない。
`DeviceAuthority` は既存の端末登録の解除を要求ごとに確認し、別途明示した
`(端末登録, 棚, 利用者)` の許可だけを使う。端末 UUID を利用者 UUID と解釈しない。
単独サーバーは以下を `server.toml` から読む。棚はサーバーの置き場の `library-v2/` に置き、
`library_id` にはその棚の `library.json` にある UUID を指定する。`sync_epoch` は同期の世代を
表す UUID で、棚を復元したときは新しい値に更新する。

```toml
[library_v2]
library_id = "<棚の UUID>"
sync_epoch = "<同期の世代 UUID>"

[library_v2.readers]
"<端末の登録 ID>" = "<その端末が読む利用者の UUID>"
```

設定の許可を起動時に読み、端末登録が有効かを要求ごとに確認する。`library_v2` を省いた
設定では従来の待ち受けで動く。新しい棚の配置と許可の変更は、管理者が設定ファイルで行う。
`piep-library backup <新棚> <新規出力先>` で検証済みの写しを作れる。端末の登録 ID は既存の
管理 API `GET /api/v1/admin/devices` の `id` を使い、読む利用者は保存済みの利用者の UUID を指定する。

管理者は `GET /api/v1/admin/status` の `libraryV2` で、開いている棚の
`libraryId`、`syncEpoch`、`works`（十進文字列）を確認できる。設定を省いたサーバーは
従来の応答を返す。管理 API に入る権限と、この棚を読める端末の許可は別々に確かめる。

## 要求と応答

すべて `Authorization: Bearer …` によるネイティブ通信。Cookie、query の鍵、
複数の認証ヘッダー、Origin 付き要求は受け入れない。鍵や内部パスを応答へ含めない。

| GET | 用途 |
| --- | --- |
| `/api/v2/identity` | 棚・利用者・同期 epoch・形式・上限・対応能力 |
| `/api/v2/libraries/{library}/entities/{entity}/views/{snapshot\|edit}/{view}/bundle` | 読書 bundle：読書用文書（`piep-reader-v1`）と、渡せる全資産の file 記述を一回で |
| `/api/v2/libraries/{library}/entities/{entity}/views/{snapshot\|edit}/{view}/resources/{body\|media\|file}/manifest` | 所属と表示版を固定した一ファイルの descriptor |
| 同じ資産パスの `/files/{file}/chunks/{ordinal}` | 指定した元ファイルの一部分を保存形式のまま転送 |

media と file は `slot` query が必須。body は query 不要。
取得時の raw 応答 original は、内部情報を含み得るため読取 API では提供しない。
file は利用者向けの attachment / document に限る。管理用原本の出力はローカル保管層の
別機能として保持する。利用者の編集版は認証した利用者との所属も照合する。

`identity` は `piep-library-api-v2` 形式を厳密に検査する。旧 `/api/about` に
`apiVersion: 2` があっても新ライブラリ対応とは解釈しない。
`readerBundle` は true、`mutations` は false。資産転送の成功を一作品の保存完了としない。

読書 bundle（`piep-reader-bundle-v1`）は、本文・画像・読書用の添付の file 記述（ハッシュ・長さ・
部分の一覧）と、それを取る view（編集の基準の保存版の資産は、その保存版の view）を持つ。
未保存の資産と原応答は載せない。部分は上の chunk の入口で一つずつ取る。画像 100 枚の作品も、
記録の取得は一回で済む（前は資産ごとに manifest を取った）。

認証後の応答には `x-piep-library-id`、`x-piep-subject-id` と `x-piep-sync-epoch` が付く。
同時に読めるのは端末ごとに 4 件、全端末あわせて 8 件。空きがなければ最大 2 秒待ち、
それでもなければ 429 と `Retry-After: 1` を返す。一台が全体を占めない。無制限の待機列を作らない。
ハッシュだけを指定する入口は設けず、manifest / chunk の両方で所属を再確認する。

## キャッシュと破損検査

部分は中身のハッシュで指定して取るので、持っていれば要求しない。ETag と 304 は持たない
（条件付き要求も毎回認証・所属・実体を確かめて中身を返す）。部分には
`private, max-age=31536000, immutable, no-transform`、記録（bundle・manifest）には
`private, no-cache, no-transform`、どちらにも `Vary: Authorization` を付ける。
圧縮済み object を再圧縮する共通 middleware は組み込まない。

合成した棚と loopback HTTP による 7 試験で、認証、bundle からの全部分の復元、解除、
別作品・別 file・別の棚、破損、端末ごとの同時数と待ち、棚・epoch のヘッダー、raw 原本の
非公開を確認する。
実サービスへの接続や実際の端末登録・鍵発行は必要ない。
