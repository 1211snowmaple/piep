# piep 包括リファクタリング・品質／性能監査レポート

- 実施日: 2026-08-12（継続実装・再検証: 2026-08-16）
- 対象: piep 0.4.0（React 19 / Tauri 2 / Rust / SQLite / Tantivy）
- 監査範囲: データ保全、セキュリティ、更新・復元、検索索引、Reader／Editor、UI／UX、アクセシビリティ、レスポンシブ、テスト基盤、大規模ライブラリ性能

## 1. 結論

今回の監査では、通常のテストでは見つかっていなかった複数の重大問題を再現し、データ損失・秘密情報漏えい・索引破損・操作競合を中心に修正した。

特に重要な修正は次のとおり。

1. 再スキャン時に、これから読み込む作品ディレクトリ自体を削除していた処理を修正した。
2. 全体バックアップが10,000作品でサイレントに打ち切られる問題をcursor走査へ変更し、不完全なまま成功しないようにした。
3. 再構築処理が作者・シリーズ情報と関係を全削除し、説明・アイコン・版情報を壊す問題を非破壊backfillへ変更した。
4. 保存先の`source`／`source_id`を使ったパストラバーサルとsymlink／junction逸脱を拒否した。
5. FANBOX Cookieが外部URLへ送られ得る経路を、公式HTTPSホストの完全一致だけに制限した。
6. EPUB検査時のZIP bomb／OOMを、entry数・単一entry・総展開量・圧縮率のquotaで防止した。
7. 作品削除後もTantivy／semantic索引へ文書が残る問題を修正し、一括削除を1 commit／1 reloadへ最適化した。
8. Readerの読書位置保存形式不一致、旧ページplaceholderの操作、保存ページ復元順序を修正した。
9. Editorで保存中に追加入力すると、その未保存入力まで「保存済み」扱いになる競合をsnapshot比較で修正した。
10. ブラウザの戻る／進むが未保存確認を迂回する問題、Tauriイベントの非同期登録cleanup race、保存元切替でWebViewを先に破壊する問題を修正した。
11. 停止中ジョブのキャンセルがworkerを再起動してしまう問題、resumeのlost wakeup、worker panic後に永久再開不能になる問題を修正した。
12. asset／プロフィール画像を最終pathへ直接書く処理を、quota付きstream→検証→fsync→atomic renameへ変更した。
13. backupを既存出力へ直接truncateせず、完成ZIPの全参照preflight後だけatomic replaceするよう変更した。
14. multipart backupで作品と無関係になった作者・シリーズが欠落する問題を、全entityのkeyset走査と専用catalog partで修正した。
15. Pixiv／FANBOXの全JSON応答をbounded stream化し、認証付きredirectを禁止した。
16. facet／候補検索とソート付き全文検索へ、DB・索引世代で無効化されるTTL／LRU／byte-bounded cacheを導入した。
17. 深いnumbered pageを最大OFFSET 5,000件に制限し、巨大・不正なURLを初回query前に正規化した。
18. 作者ページのシリーズ200件上限をopaque keyset cursor、server-side検索、exact totalへ置換した。
19. 同じlibraryを複数processが開く競合を、DB open前のOS排他lockで防止した。Windowsの起動失敗は日本語dialogで可視化した。
20. 更新ジョブIDを時刻／PID／counter依存から128-bit乱数へ変更した。

通常の一覧は1,000,000作品のmetadata-only fixtureで初回336.46ms、warm 1.51ms、深いcursor 1.38msをdebug buildで確認した。100,000作品の実Tantivy全件一致fixtureでは、relevance snapshotのcold構築12.09秒／2ページ目77.11ms、sort snapshotのcold構築7.61秒／2ページ目46.52ms、2 snapshot合計4MiBだった。semantic、更新ジョブ、バックアップ、全文検索、一括mutationも本追加ラウンドでメモリbounded／永続化／分割処理へ変更した。ただし1,000,000作品の本文・画像・Tantivy・semantic・multipart backupを組み合わせたend-to-end実データ受入試験まで完了したという意味ではない。

## 2. 実施方法

### 自動検証

- TypeScript型検査
- Vitest全体回帰
- Vite production build
- Rust全target test
- Rust Clippy（all targets / all features / warnings deny）
- rustfmt、`git diff --check`
- npm dependency audit
- 20,000／100,000作品fixtureの一覧・深いcursor・作者・facet性能試験
- 100,000作品の実Tantivy全件一致snapshot試験
- 1,000,000作品のmetadata-only一覧・集計・cache受入試験
- 実ライブラリ2,237作品／約4,347万文字／semantic 87,957 chunkのread-only計測
- Playwright visual／navigation matrix

### 実ブラウザ操作

ブラウザプレビューを実際に操作し、次を確認した。

- 360 / 768 / 1,440px幅
- light / dark
- ホーム、ライブラリ、作品、Reader、Editor、Web保存、EPUB、テンプレート、更新、操作履歴、診断、設定
- フィルタードロワー開閉・適用領域
- 設定内セクション移動
- モバイルナビゲーション開閉と画面遷移
- 文書／main領域の横overflow
- 無名button、画像alt、見出し、致命的エラー表示

最終確認では、対象12ルートで致命的表示、横overflow、無名button、alt欠落を検出しなかった。フィルタードロワーの閉じるボタンは`閉じる`として公開され、文字数入力の不要な無名stepperも除去されている。

2026-08-16のin-app Browserによる追加操作では、設定画面を360px幅にするとセクション選択部だけが横スクロールする問題を発見した。選択部を3列（576px以下は2列）の折返しgridへ変更し、360px／768pxの双方で`scrollWidth == clientWidth`、ページ全体もviewport幅内、無名button 0件を再確認した。キーボード操作はBrowser automation側のTab送信が安定しなかったため、手動合格とはせずDOM統合テストのARIA検証を根拠としている。

最終ラウンドでは、同じBrowserで設定→診断とメンテナンス→計測を実操作した。9,633件のDB参照照合、健全性alert、再計測導線、性能表を確認した。360×800pxで`documentElement.scrollWidth == clientWidth == 360`、横overflowなし、無名button 0件、`h1`と`role=alert`の名称ありをDOMと画面の双方で再確認した。

診断結果の対処導線も追加確認した。警告から最大100件の対象を作品名・種別・path・期待／実size付きで確認でき、親folderを開く、安全な再取込、backup復元、元serviceからの再保存へ直接進める。自動削除は行わない。また、app data直下の正規な`profiles/`・`series/`を`downloads/`外という理由だけで「領域外」と誤判定していた問題を修正し、参照種別ごとに許可rootを検証するようにした。360×800pxの詳細dialogでも横overflowと無名buttonがないことを実操作で確認した。

## 3. 修正済みの問題

### P0: データ損失・バックアップ

| 問題 | 原因 | 修正 |
|---|---|---|
| 再スキャンで原本を削除 | 既存作品へ通常の`delete_download`を使い、走査中ディレクトリまで削除 | 再取込専用のDB record削除へ分離し、作品treeを保持 |
| 再取込後に作者／シリーズ詳細が消える | 関係・entity表を全DELETEして再構築 | `INSERT OR IGNORE`中心の非破壊backfillへ変更 |
| legacy作品1件の削除でsource配下全体を消し得る | `json_path`のparent回数から削除rootを推定 | DB上の安全な単一componentからrootを構成しcanonical containmentを検証 |
| 全体バックアップが10,000件まで | 固定limitの1ページだけをexport | cursorで全ページを収集し、重複・総件数変化・途中欠落を検知 |
| backup失敗で既存backupを破壊／欠落参照をsilent skip | 出力先を直接truncateし、参照file不在を成功扱い | 同一dirの一意tempへstream出力し、全必須参照・quota・pathをpreflight、sync後だけatomic replace。失敗時は旧backupとtemp cleanupを検証 |
| multipart backupで孤立した作者／シリーズが欠落 | 各作品partのrelationからentity scopeを作っていた | 全people／seriesを軽量keyset走査し、専用catalog partへ重複なく分割保存。作品0件・update targetのみのlibraryも保持 |
| 削除済み作品が検索に残る | SQLite rowだけ削除しsidecarを未削除 | SQLite transaction後、Tantivyとsemanticを一括削除 |

### P1: セキュリティ・DoS

| 問題 | 修正 |
|---|---|
| `source`／`source_id`の`..`等で保存root外へ書込可能 | provider allowlist、安全なASCII単一component、canonical containment検証 |
| FANBOX Cookieを任意asset／pagination URLへ付与 | APIは`https://api.fanbox.cc:443`、assetは許可した公式HTTPSホストだけ。redirectも禁止 |
| EPUB validatorが全entryを無制限展開 | 最大10,000 entries、単一256 MiB、総展開512 MiB、圧縮率250倍で事前拒否 |
| assetを最終pathへ直接truncateし、途中fileを次回「取得済み」と判定 | 一意`.part`へstream、Content-Length／累積256 MiB、画像header／最大1億pixel検査、flush／sync後rename。失敗・cancel・panic時はRAII cleanup |
| profile iconをtimeout／size上限なしで全bytes RAMへ読込 | connect 10秒／total 30秒、16 MiB、画像検査、同じatomic stream helper |
| asset走査がlink loop／root外／無制限再帰し得る | canonical containment、symlink／junction拒否、cycle検出、深さ16、20,000 files、`.part`除外 |
| 短寿命Pixiv tokenの`expires_in - 300` underflow | `saturating_sub`で短寿命tokenを即時refresh対象化 |
| `BadAccessToken`表示へtoken平文を含む | 表示・エラー経路からtokenを除去しsecret sentinel testを追加 |
| Pixiv／FANBOXの一般JSON応答が無制限 | JSON 32 MiB、Pixiv webview 64 MiB、token 2 MiB。Content-Length先行拒否とchunk累積検査、connect 10秒／total 30秒、credential redirect禁止 |

### P1: 更新ジョブ・非同期競合

| 問題 | 修正 |
|---|---|
| paused jobのcancelがcredentialなしworkerを起動 | workerを起動せず直ちにcanceledへ遷移 |
| canceling中のworkerがrunningへ戻す | 起動直後・各安全境界でstatusを再確認 |
| resume直後に旧workerが終了するとqueuedのままworkerなし | 最新credentialsをpending restartへ保持し、旧worker終了後に確実に再起動 |
| resume中の新credentialsが無視される | 旧workerを安全境界で退役させ、後続workerへ最新credentialsを渡す |
| worker panicでactive setにIDが永久残留 | worker本体とcleanup監督taskを分離し、panic時もactive IDを除去 |
| 重複candidateでもfound件数を加算 | insertが実際に行を追加した場合だけfoundを加算 |
| update job logが無制限に増加 | jobごとに最新2,000件へbounded化 |

### P1: Reader・Editor・画面遷移

| 問題 | 修正 |
|---|---|
| Readerは`sessionStorage`へobject、棚は`localStorage`から数値を読んでいた | `{page, top}`の共有schemaへ統一し、旧数値／quoted数値／session形式を移行 |
| 読みかけ件数がmount時から更新されない | membership変化イベントを購読してsidebar queryを即時更新 |
| 保存位置が2ページ目以降だと、本文到着前にtopを復元済みにする | 対象pageの実data到着後だけ復元 |
| page切替中に前pageのplaceholder本文を操作・保存できる | placeholderをLoading表示へ替え、しおりと位置書込を停止 |
| Editor保存中の追加入力がcleanになる | submit開始時deep snapshotと現在fingerprintが同一の場合だけdirty解除 |
| publish後のReader cache keyが不一致 | 実際の`reader-metadata`、`reader-content-page`等をinvalidate |
| popstate/hashchangeが未保存guardを迂回 | 外部popを元entryへ戻して確認し、承認時だけ遷移をcommit |
| 保存元切替で確認前にembedded browserを破壊 | guard済みroute変更がcommitした後のsource effectで破棄・再作成 |
| async event登録がunmount後に解決するとlistener leak | 同期cleanup wrapperを追加し、遅延解決したunlistenも直ちに実行 |
| 作品削除後に読書位置・EPUB queue・Reader／Editor cacheが残る | DB削除成功後だけ共通cleanupを実行し、失敗時はlocal stateを保持 |
| Libraryの並行optimistic mutationで古いrollbackが後続成功を消す | 失敗した作品・fieldだけrollbackし、全in-flight settleまでrefetchを遅延 |
| Work／Readerでrouteやversion切替中に旧本文を表示・操作 | query identityと`isPlaceholderData`を検証し、対象data到着までLoading表示 |

### P2: UI・アクセシビリティ・テスト基盤

- actionとして使うサイドバー／設定NavLinkを`button type="button"`へ変更した。
- 展開groupへ`aria-expanded`を付与した。
- Modal／Drawer共通close buttonへ`aria-label="閉じる"`を設定した。
- Libraryへ視覚非表示の`h1`を追加した。
- 最小／最大文字数入力の不要なNumberInput controlsを非表示にした。
- PlaywrightのbaseURLとVite/TauriのWindows上の`localhost`／`::1` bindingを一致させ、`127.0.0.1`へのchunk load失敗を解消した。
- 追加されたテンプレートスタジオのvisual baselineを6構成で作成した。
- README screenshot testの旧表示名`EPUB Studio`を現行の`EPUB書き出し`へ更新した。
- 狭幅設定ナビを横スクロールstripから折返しgridへ変更し、最後の項目が画面外へ隠れる問題を解消した。
- 作者ページの従来200件上限を撤去し、server検索・keyset追加読込・cursor失効時の再読込導線を追加した。
- numbered pageのOFFSET予算を5,000件に固定し、上限時は自動読込／絞り込みへ誘導した。
- 作者シリーズを60件単位のkeyset infinite queryへ変更し、200件以降もserver検索または追加読込で到達可能にした。

## 4. 性能計測

### 4.1 fixture／実データ

| 計測 | 結果 |
|---|---:|
| 20,000作品・先頭ページ（最終実装） | 14.9 ms |
| 20,000作品・深いcursorページ（最終実装） | 1.41 ms |
| 20,000作品・作者集計／filter | 49.3 ms／18.2 ms |
| 100,000作品・先頭ページ（最終実装） | 25.59 ms |
| 100,000作品・深いcursorページ（最終実装） | 0.98 ms |
| 100,000作品・作者集計 | 136.68 ms |
| 100,000作品・facet cold／warm | 56.35 ms／0.034 ms |
| 100,000作品・suggestion cold／warm | 34.47 ms／0.036 ms |
| 100,000作品・Tantivy relevance snapshot cold／2ページ目 | 12.092秒／77.11 ms |
| 100,000作品・Tantivy sort snapshot cold／2ページ目 | 7.613秒／46.52 ms |
| 100,000作品・Tantivy snapshot一時容量 | 2件合計4 MiB |
| 1,000,000作品・metadata fixture作成／DB容量 | 66.18秒／1,130 MiB |
| 1,000,000作品・一覧 cold／warm | 336.46 ms／1.51 ms |
| 1,000,000作品・深いcursor | 1.38 ms |
| 1,000,000作品・作者集計 cold／warm | 1.641秒／0.078 ms |
| 1,000,000作品・facet cold／warm | 719.15 ms／0.043 ms |
| 1,000,000作品・suggestion cold／warm | 0.869 ms／0.019 ms |
| 1作者20,000シリーズ・初回／50ページ中最遅 | 117.43 ms／63.91 ms |
| 本文前処理 2,000文字 | 9.3 ms |
| 本文前処理 16,000文字 | 73.1 ms |
| 全文索引再構築 300作品×12,000文字 | 6.65秒、45.1作品/秒 |
| 実ライブラリ2,237作品・日付一覧 | 0.13 ms |
| 実ライブラリ・タグfacet | 3.37 ms |
| semantic fingerprint 87,957 chunks | 165.8 ms |
| semantic全payload走査 | 1.02秒 |

45.1作品/秒を単純外挿すると、索引再構築は1万件約3.7分、10万件約37分、100万件約6.2時間になる。debug build・現在の端末・本文分布に依存する参考値であり、保証値ではない。

### 4.2 容量外挿

実ライブラリは2,237作品、本文約4,347万文字、平均19,433文字だった。

| 構成 | 実測 | 10万作品の単純外挿 | 100万作品の単純外挿 |
|---|---:|---:|---:|
| Tantivy | 224.8 MB | 約10 GB | 約100 GB |
| semantic SQLite | 368.1 MB | 約16 GB | 約164 GB |
| semantic vector生データ | 約135 MB | 約6 GB | 約60 GB |

## 5. 規模別評価

### 1万～2万作品

一覧、cursor page、通常の語彙検索は実用範囲。通常回帰に加え、20,000シリーズの作者ページkeysetも直接試験している。

### 10万作品

一覧・集計・facetはfixtureで、relevance／sortは本文を持つ実Tantivy fixtureで直接試験した。facet／suggestionの世代付きbounded cache、update snapshotのdisk backing、progress throttle、frontendの`maxPages`とvirtualize、multipart backup、全文検索のdisk snapshotを実装済みである。全件一致のsnapshot初回構築はO(N)であり、debug buildではrelevance 12.09秒、sort 7.61秒かかった一方、2ページ目以降は77.11ms／46.52msだった。

### 100万作品

従来実装は非対応だった。特にsemantic ANNは全chunkの本文と384次元vectorをメモリへ読み、HNSWを再構築していた。今回、SQLite payloadと永続sharded HNSWを分離し、検索時は1 shardずつload、変更shardだけを再構築する方式へ変更した。共通メモリ予算も導入したため、以前の「全vectorをRAMへ展開」という決定的な障害は除去した。

今回実装した設計変更:

1. persistent sharded ANNと変更shard単位の再構築
2. ANN graphと本文payloadの分離
3. 作品単位のmodel/version/content hash/coverage管理
4. manifest + SHA-256付きmultipart ZIP（2,000作品単位、作品／entity／update targetをquota時に自動二分）
5. frontend bounded query cache、entity virtualize、large JSON表示上限
6. update candidate/log cursor、operation永続化のbatch化

1,000,000作品のmetadata-only fixtureは実行済みで、一覧・keyset・作者・facet・suggestionの受入値を取得した。未実施なのは、長編の階層要約、および本文・画像・Tantivy・semantic・multipart backupを同時に含む1,000,000作品end-to-end受入試験である。

## 6. 未解決リスクと推奨順序

「バグが一切存在しない」ことは有限のテストでは証明できない。前版で列挙した項目は推奨順に実装した。以下は完了状況と、なお残る境界を区別した一覧である。

### 次のP0/P1

1. **完了 — 同一作品の同時保存**: `(source,id)` lock、同一filesystem staging、atomic rename、単一SQLite transaction、durable save journal、startup recoveryを実装。競合・DB失敗・crash回帰を追加。
2. **完了 — 保存のFS／DB非原子性**: assetも`.part`へbounded stream後に検証・fsync・rename。途中失敗、切断、過大応答、破損既存fileを回帰化。
3. **完了 — restoreのcrash consistency**: filesystem journalとSQLite commit markerを組み合わせ、derived indexはDB commit後へ移動。commit前後の失敗注入に対応。
4. **完了 — backupの時点整合性**: process内library read/write gateで全backup対象mutationとexport/importを同期し、別processはDB open前のOS lockで拒否。
5. **完了 — 大規模backup**: JSON manifest、複数ZIP、SHA-256、全part事前検査、重複作品検査、容量検査、自動part二分、再開可能な復元を実装。従来単一ZIP restoreも維持。
6. **完了 — backup catalogの全件性**: 作品に紐づかないpeople／seriesと大量update targetを各5,000件のkeysetで走査し、作品／entity／targetを別partへquota時自動二分。空libraryも保持。

### スケール上のP1

1. **完了** semantic coverageを`download_id/current_version/content_hash/model_id`単位で管理し、全current作品のpendingが0のときだけcompleteにした。
2. **完了** update candidateは200件cursor、logは300件reverse cursorへ変更。UIはID mergeで追加読込できる。
3. **完了** sort付き全文検索は256件固定batchでdisk-backed SQLite snapshotへstreamし、巨大Vec／JSONと旧100万ID上限を廃止。索引・library世代に固定し、TTL 90秒・LRU・共有disk quotaで再利用する。
4. **完了** relevanceも全scoreを256件固定batchでdisk-backed snapshotへ固定し、2ページ目以降の全posting再採点を除去。exact filtered total、stable download ID順、segment merge／library mutation時の世代契約を回帰化した。
5. **完了** 主要sort索引へ`id` tie-break列を追加し、query-plan testでTEMP B-tree不使用を確認。
6. **緩和済み** 任意ページへ直接移動するnumbered modeのOFFSETは残るが、5,000件を上限として巨大deep linkをquery前に拒否・正規化する。通常のauto scroll／深い一覧はkeyset cursorを使う。
7. **完了（bounded cache）** suggestionをprefix/indexed化し、facet／suggestion／filter-facetsへdata-version付きTTL／LRU／byte上限cacheを追加。100,000件でfacet 56.35ms→0.034ms、suggestion 34.47ms→0.036ms。完全な差分materializeは不要になったが、cold queryは全体集計を行う。
8. **完了** infinite queryを最大25 pageに制限し、entity gridをvirtualize、上限到達時にUIで絞り込みを案内。
9. **完了** 利用可能RAMからSQLite cache/mmap、Tantivy、semantic ANNへ共通予算を割当。
10. **一部完了** 通常Tantivy writerを再利用し、bulk writerとは排他調停。保存ごとのdurable commit/reloadは正確性のため維持。
11. **完了** 作者シリーズは`latestWorkAt/count/name/source/key`の安定keysetへ移行。20,000シリーズでgap／duplicateなし、初回117.43ms、深部最遅63.91ms。cursorは人物・query・SQLite世代に固定し、途中更新時はUIが既読を保って先頭再読込を案内。

### その他のP1/P2

- **完了** Pixiv／FANBOX pagingはlast-seen停止、page/item上限、cursor重複検出、truncation明示errorを追加。
- **完了** API 401/403、invalid grant/token等を`auth_required`へ分類。
- **完了** EPUB batchをbounded `spawn_blocking`化し、temp build→validate→fsync→atomic replaceへ変更。invalidはfailure分類。
- **完了（quota方式）** EPUB画像は64MiB/枚、40MP/枚、128MiB/冊、10,000枚を上限とし、二重pre-readを廃止。完全な逐次spoolではないため同時2冊分のpeakは残る。
- **完了** diagnosticsを単一iterative bounded walkに統合し、symlink/junction、cycle、depth64、500万entryを防御。
- **完了** Entity raw JSONはlazyかつ最大30万文字DOM、operation localStorageは250ms batch＋terminal flushへ変更。
- **完了** 一括watch／deleteの200,000件silent capと全ID `Vec + HashSet`を廃止。検索結果をdisk snapshotへ固定batchでstreamし、SQLiteはset-based mutation、Tantivyは1 commit、semanticは1,000件chunkで処理する。
- **完了** Pixiv／FANBOXの一般JSON endpointをbounded response readerへ統一し、timeout／redirect policyも回帰化した。
- **完了** app dataをDBより先にOS exclusive lockし、同時processを拒否。drop／process abort後の再取得と、Windows releaseの日本語起動error dialogを回帰化した。
- **完了** 更新job IDを128-bit乱数へ変更し、再起動／PID再利用／時計粒度への依存を除去した。
- **完了** DB参照ファイルの手動削除・領域外link・0 byte・asset容量差・一時`.part`／`.stage`をbounded stream診断し、設定画面へ件数・最大100件の対象詳細・用途別の対処を表示。正規な`profiles/`・`series/`を領域外扱いする誤判定も修正した。DBに存在しない完全な作品folderだけをcanonical／quota検証後、作品単位の単一DB transactionで安全に再取込する。既存作品は上書きしない。
- **完了** version directory publishは全regular fileと子directoryを同期し、Windowsは`MoveFileExW(MOVEFILE_WRITE_THROUGH)`、Unixはrename後のparent fsyncを行う。rollback後もparent同期完了時だけjournalを削除する。
- **残る受入境界** 1,000,000作品metadata-onlyと100,000作品実Tantivyまでは実測済み。1,000,000作品の本文・画像・Tantivy・semantic・multipart backupを同時に使う全量試験、実機電源断、HDD／8GB RAMのrelease build受入は未実施。snapshot cold構築は一致件数に対してO(N)で、一時disk quota超過時は明示errorになる。外部filesystem変更は自動で推測修復せず、診断と安全な再取込の対象に限定する。

## 7. 最終回帰試験結果

| 検証 | 結果 |
|---|---|
| `npm run check` | 成功 |
| `npm test -- --run` | 43 files / 234 tests passed |
| `npm run build` | 成功 |
| `npm audit --audit-level=moderate` | 0 vulnerabilities |
| `cargo test --all-targets --all-features -- --test-threads=1` | lib 226 passed / 0 failed / 7 ignored、main 1 passed |
| `cargo clippy --all-targets --all-features -- -D warnings` | 成功 |
| `cargo fmt --all -- --check` | 成功 |
| `git diff --check` | 成功（WindowsのLF→CRLF予告のみ） |
| `npm run test:visual` | layout 35 passed / 198 matrix skips。README screenshot 1件はWindows一時file lockで失敗後、同projectを1 workerで再実行しpassed |

Playwrightのlibrary shellは900×600、1,200×800、1,440×900、light／dark、100／150／200dpiで横clippingを検査した。手動ブラウザ横断では360／768／1,440pxの12主要routeでも横overflow、無名button、alt欠落、致命的表示を検出しなかった。

追加の1,000,000作品metadata-only fixtureでは、初回一覧336.46ms、warm 1.51ms、深いcursor 1.38ms、作者cold／warm 1.641秒／0.078ms、facet cold／warm 719.15ms／0.043msで合格した。100,000作品の実Tantivy fixtureでは全件一致をdisk snapshot化し、2ページ目をrelevance 77.11ms、sort 46.52msで取得した。同時保存、保存DB失敗、restore commit前後失敗、破損asset、ZIP bomb、multipart復元、孤立entity catalog、Tantivy segment merge中のcursorも自動試験へ追加した。本レポートの数値は同一端末のdebug build実測であり、release build、8 GB RAM、HDD、実画像容量を含む1,000,000作品multipart全量試験は今後の受入試験として残る。
