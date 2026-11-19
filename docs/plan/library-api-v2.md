# 新ライブラリの認証付き資産 API

## 実装の境界

`server::v2::router` は接続情報、作品一覧、取得元の ID からの作品照合、読書 bundle、資産 manifest、
圧縮した部分の転送を提供する。
単独サーバーでは、設定で指定した棚を開いて既存の待ち受けにこの読取 API を載せる。
利用者状態の書き込みは、棚の作品のお気に入りの付け外しと読書位置を提供する（下の「お気に入りの付け外し」
「読書位置」）。対応能力（`identity` の capabilities）には含めず、応答の `x-piep-favorite-write: 1`・
`x-piep-reading: 1` で知らせる。
携帯の作者・シリーズ・コレクションの画面の口も同じく、`x-piep-groups: 1` で知らせる。
検索欄の候補の口と、作品の画面の版の一覧に版ごとの写しの ID を添えることは、`x-piep-suggest: 1` で知らせる。

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
設定では従来の待ち受けで動く。端末の登録 ID は既存の管理 API `GET /api/v1/admin/devices` の
`id` を使い、読む利用者は保存済みの利用者の UUID を指定する。

管理者は `GET /api/v1/admin/status` の `libraryV2` で、開いている棚の
`libraryId`、`syncEpoch`、`works`（十進文字列）を確認できる。設定を省いたサーバーは
従来の応答を返す。管理 API に入る権限と、この棚を読める端末の許可は別々に確かめる。

### サーバーに棚の写しを置く

`piep-library backup <新棚> <新規出力先>` で作った写しをサーバーへ運ぶ。サーバー上で、
初期化済みの置き場と運んだ写しを指定する。

```text
piep-server stage-library --data <サーバーの置き場> --shelf <運んだ棚の写し>
piep-server activate-library --data <サーバーの置き場> --generation <準備で表示された UUID>
```

準備は稼働中にも実行できる。棚の内容を検証した写しを `library-v2-generations/<準備の UUID>/`
に置く。続いてサーバーを停止して選んだ写しを有効にすると、再検証後に `server.toml` の
`library_v2.generation` が更新される。初回は `library_v2.readers` に端末の許可を設定してから起動する。
以後の切り替えは同じ棚の UUID の写しの間で行い、設定済みの端末の許可を引き継ぐ。

切り替えと復元のたびに同期 epoch を発行する。同じ写しを続けて指定した場合は現在の epoch を使う。
以前の写しへ戻すには、停止中にその準備の UUID を指定して有効にし、サーバーを起動する。
`generation` を省いて手動配置の `library-v2/` を開く構成も引き続き使える。

## 要求と応答

すべて `Authorization: Bearer …` によるネイティブ通信。Cookie、query の鍵、
複数の認証ヘッダー、Origin 付き要求は受け入れない。鍵や内部パスを応答へ含めない。

| GET | 用途 |
| --- | --- |
| `/api/v2/identity` | 棚・利用者・同期 epoch・形式・上限・対応能力 |
| `/api/v2/libraries/{library}/works` | 保存日の新しい順に、棚にある作品と現在の読書用の版を一ページ返す |
| `/api/v2/libraries/{library}/works/lookup?provider={provider}&key={key}` | 取得元の作品 ID から、棚の作品と現在の読書用の版を照合 |
| `/api/v2/libraries/{library}/works/{work}` | 携帯の作品の画面：今の版のカード・説明・作者とシリーズ・版の一覧（版ごとの写しの ID つき。以前の版はその写しの bundle で読む） |
| `/api/v2/libraries/{library}/works/{work}/cover?size={card\|large}` | 今の版の表紙を携帯向けの WebP（320×480 / 640×960 に収める）で |
| `/api/v2/libraries/{library}/works/suggest?text&limit` | 検索欄の候補（旧 API の `search/suggest` と同じ形、既定 8・上限 12）。題の候補の `sourceKey` は棚の作品の ID |
| `POST /api/v2/libraries/{library}/works/search` | 携帯の一覧：旧 API と同じ条件の形で、絞り込み・並べ替え・語の検索・ページ送り |
| `PUT /api/v2/libraries/{library}/works/{work}/favorite` | 認証した利用者のお気に入りを付け外しする（下の「お気に入りの付け外し」） |
| `PUT /api/v2/libraries/{library}/works/{work}/reading` | 認証した利用者の読書位置を書く（下の「読書位置」） |
| `/api/v2/libraries/{library}/reading?cursor&limit` | 認証した利用者の読書位置。初めは全部、そのあとは変わった分（既定 200・上限 500） |
| `/api/v2/libraries/{library}/works/{work}/collections` | 作品の入ったコレクションと、その中の位置 |
| `/api/v2/libraries/{library}/{people\|series}?text&offset&limit&sort&order` | 作者・シリーズの一覧。並べる鍵と向きは旧 API と同じ語彙、続きは `nextOffset` |
| `/api/v2/libraries/{library}/{people\|series}/detail?provider&key` | 作者・シリーズの画面の頭：説明・リンク・タグ・シリーズの数・コレクション |
| `/api/v2/libraries/{library}/people/series?provider&key&text&sort&order&cursor&limit` | 作者の作品が属するシリーズ。続きの位置は何件目からかの十進 |
| `/api/v2/libraries/{library}/{people\|series}/cover?provider&key[&variant=banner]` | 作者のアイコン（`banner` はヘッダー、320×480 / 1280×480）、シリーズの表紙 |
| `/api/v2/libraries/{library}/collections` | コレクションの一覧（更新の新しい順）。`id` は棚の UUID |
| `/api/v2/libraries/{library}/collections/{collection}` | コレクションの画面。回の作品は一覧と同じ形。旧棚から移したものは旧棚の ID でも開ける |
| `/api/v2/libraries/{library}/collections/{collection}/cover/{single\|file\|tile-0〜3}` | 表紙に選んだ作品の表紙・選んだ画像・並びの先頭からの回の表紙 |
| `/api/v2/libraries/{library}/entities/{entity}/views/{snapshot\|edit}/{view}/bundle` | 読書 bundle：読書用文書（`piep-reader-v1`）と、渡せる全資産の file 記述を一回で |
| `/api/v2/libraries/{library}/entities/{entity}/views/{snapshot\|edit}/{view}/resources/{body\|media\|file}/manifest` | 所属と表示版を固定した一ファイルの descriptor |
| 同じ資産パスの `/files/{file}/chunks/{ordinal}` | 指定した元ファイルの一部分を保存形式のまま転送 |

media と file は `slot` query が必須。body は query 不要。
取得時の raw 応答 original は、内部情報を含み得るため読取 API では提供しない。

作品照合には取得元の名前（例 `pixiv`、128 バイトまで）と、
URL エンコードした取得元の作品 ID（デコード後 4096 バイトまで）を渡す。
応答の `entityId` と `snapshotId` で保存版の bundle を取れる。`editId` がある作品の反映中の
編集を読む場合は `views/edit/{editId}/bundle` を使う。棚に現在ある作品だけが照合できる。
file は利用者向けの attachment / document に限る。管理用原本の出力はローカル保管層の
別機能として保持する。一覧と照合の `editId` は認証した利用者が反映中の版を指す。
編集の bundle・manifest・部分実体では、その利用者が反映中か以前の編集版に残しているかを確かめる。

一覧の対応は `identity` の応答ヘッダー `X-Piep-Works-List: 1` で分かる。
`limit` は 1〜50 件、省略時は 30 件。一覧の `items` の各行には `view`（作品照合と同じ項目）、
`provider`、`key`、`title`、`authorName`、`contentType` が入る。同じ保存日の作品は作品 UUID の降順。
`next` があれば変更せず次の要求の `after` に渡し、`null` なら終わり。
同期 epoch または利用者が変わった場合は 409（`stale_cursor`）を返すので、最初のページから読み直す。

携帯の一覧は `identity` の応答ヘッダー `X-Piep-Works-Search: 1` で分かる。本文は旧 API の
`MobileSearchRequest` と同じ JSON で、同じ検査を通す（知らない項目・範囲外・重なる指定は 400）。
作品は棚の UUID（`id`）と今の版の UUID（`snapshotId`）で返す。続きの位置は棚・同期 epoch・利用者に
結び付け、違えば 409（`stale_cursor`）。ページ番号で開いたページには続きの位置を付けない。
件数は最初のページだけ数える。語の検索は、配信する写しの索引が棚に追いつくまで 503
（`search_indexing`）で断り、絞り込みだけの一覧は先に答える。表紙は作品のカードから引くので、
ハッシュだけでは取れない。棚から外した作品は、作品の画面も表紙も 404。

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
`private, max-age=31536000, immutable, no-transform`、記録（作品一覧・作品照合・bundle・manifest）には
`private, no-cache, no-transform`、どちらにも `Vary: Authorization` を付ける。
圧縮済み object を再圧縮する共通 middleware は組み込まない。

合成した棚と loopback HTTP の試験で、認証、bundle からの全部分の復元、解除、
別作品・別 file・別の棚、破損、端末ごとの同時数と待ち、棚・epoch のヘッダー、raw 原本の
非公開を確認する。
実サービスへの接続や実際の端末登録・鍵発行は必要ない。

## お気に入りの付け外し

本文は `{"requestId": "<UUID>", "expected": "<十進の更新世代>", "favorite": true|false}` で、
知らない項目は 400。`expected` は一覧や作品の画面の `favoriteGeneration`（利用者ごとの値、
行が無ければ `"0"`）か、前の付け外しで返った `generation`。符号・空白・先頭の 0 の重ねは 400。

- 成功すると `{"favorite": …, "generation": "<十進>"}` を返す。同じ `requestId` を送り直すと、
  棚に残した受領をそのまま返し、世代は進まない。
- 世代が違う（別の端末やデスクトップで先に変わった）か、同じ `requestId` を別の変更に使ったときは
  409 `{"error": "favorite_conflict", "current": {"favorite": …, "generation": …}}`。端末の時刻で
  勝者を決めない。識別のヘッダーは 409 でも付くので、棚の入れ替わり（ヘッダーが変わる 409）と見分けられる。
- 棚に無い作品・外した作品は 404、棚が書き込み中なら 503 `busy`。
- 単独サーバーは同じ変更を旧 API の変更の記録（`favorite_changes`）へも写し、デスクトップ版は
  今までどおり取り込む。旧 API で変えた値は新しい棚の利用者にも当て、写しを入れ替えたときは、
  デスクトップ版がまだ取り込んでいない変更を当て直す。

## 読書位置

位置は読む画面のページ（本文の区切りで数えた 0 からの番号）とその中の割合（百万分率）で、読んでいた
版の写しか編集に結ぶ：`{"viewKind": "snapshot"|"edit", "viewId": "<UUID>", "page": n, "ratioPpm": n}`。
デスクトップの読む画面と同じ形（`ReadingAnchor::Page`）なので、同じ利用者の位置はデスクトップと携帯で
共通。

- 書き込みの本文は `{"requestId", "expected", "location", "finished"}`。受領・世代・409 の扱いは
  お気に入りと同じで、409 は `{"error": "reading_conflict", "current": <位置>}`。本文に無いページ・
  別の作品の版・利用者が読めない編集は 400、棚に無い作品は 404。
- 位置は `{"work", "location", "finished", "mine", "updatedAt", "generation"}`。`location` が空なのは
  消した位置か、ページで表せない位置。`mine` は要求した端末が書いた位置か。端末の識別は登録の ID
  から決まった UUID（旧 API の位置を移すときと同じ）。
- 一覧は `cursor` を空で始めると利用者の位置を作品の順に全部返し、返し終えると変わった分の印になる。
  印は棚の変更の記録（outbox）の番号で、返し始める前に控えるので途中で変わった位置も次で届く。印には
  同期の世代を入れ、別の写しの印は 400 `reading_cursor_expired`（端末は空から取り直す）。
- 携帯は書き込みを端末の SQLite に置いてから送る。409 の今の位置がこの端末のもの（応答の届かなかった
  前の書き込み）なら今の世代で書き直し、ほかの端末のものなら捨てて、読む画面で続きの位置を案内する。
  棚・利用者・同期の世代が変われば、端末の位置と未送信の書き込みを捨てる。
- 単独サーバーの棚は配る写しなので、携帯が書いた位置は写しを入れ替えると消える。デスクトップの棚へ
  取り込むのは、写しを送る仕組み（遠隔転送）と合わせて作る。
