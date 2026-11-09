# 新ライブラリの認証付き資産 API

## 実装の境界

`server::v2::router` は独立して組み込める読取 router。現在の本番サーバーの棚や
旧 API は切り替えない。接続情報、資産 manifest、圧縮した部分の転送を実装した。
一覧、読書 bundle、利用者状態の書き込みは未提供で、対応能力にも含めない。

管理側は新ライブラリを開き、永続化した同期 epoch と認可器を `Context` に渡す。
epoch は棚の復元・再作成時に変更し、要求や router の起動ごとには生成しない。
`DeviceAuthority` は既存の端末登録の解除を要求ごとに確認し、別途明示した
`(端末登録, 棚, 利用者)` の許可だけを使う。端末 UUID を利用者 UUID と解釈しない。
このアダプターの許可はメモリー上のみで、再起動後は未許可になる。
永続許可の管理と本番サーバーへの組み込みは、棚の切り替え処理とともに実装する。

## 要求と応答

すべて `Authorization: Bearer …` によるネイティブ通信。Cookie、query の鍵、
複数の認証ヘッダー、Origin 付き要求は受け入れない。鍵や内部パスを応答へ含めない。

| GET | 用途 |
| --- | --- |
| `/api/v2/identity` | 棚・利用者・同期 epoch・形式・上限・対応能力 |
| `/api/v2/libraries/{library}/entities/{entity}/views/{snapshot\|edit}/{view}/resources/{body\|media\|file}/manifest` | 所属と表示版を固定した一ファイルの descriptor |
| 同じ資産パスの `/files/{file}/chunks/{ordinal}` | 指定した元ファイルの一部分を保存形式のまま転送 |

media と file は `slot` query が必須。body は query 不要。
取得時の raw 応答 original は、内部情報を含み得るため読取 API では提供しない。
file は利用者向けの attachment / document に限る。管理用原本の出力はローカル保管層の
別機能として保持する。利用者の編集版は認証した利用者との所属も照合する。

`identity` は `piep-library-api-v2` 形式を厳密に検査する。旧 `/api/about` に
`apiVersion: 2` があっても新ライブラリ対応とは解釈しない。
`readerBundle` と `mutations` は false。資産転送の成功を一作品の保存完了としない。

認証後の応答には `x-piep-library-id` と `x-piep-sync-epoch` が付く。
並列読取は4件までで、超過は429。無制限の待機列を作らない。
ハッシュだけを指定する入口は設けず、manifest / chunk の両方で所属を再確認する。

## キャッシュと破損検査

ETag は転送する実際のバイト列の SHA-256。元ファイルのハッシュとは用途を分ける。
認証、所属、実体の検証を終えてから条件付き要求を判定する。304 で認可解除や破損を
隠さない。弱い比較、複数候補、`*` は標準のヘッダー parser で扱う。
`private, no-cache, no-transform` と `Vary: Authorization` を付ける。
圧縮済み object を再圧縮する共通 middleware は組み込まない。

合成した棚と loopback HTTP による6試験で、認証、条件付き転送、解除、別作品・別 file、
破損、並列上限、棚・epoch のヘッダー、raw 原本の非公開を確認する。
実サービスへの接続や実際の端末登録・鍵発行は必要ない。
