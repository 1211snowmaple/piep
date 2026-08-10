# piep 大規模リファクタリング・総合監査レポート

実施日: 2026-08-09
対象: React / TypeScript / Mantine / TanStack Query・Virtual / Tauri 2 / Rust / SQLite / Tantivy / WebView2 / EPUB・バックアップ

## 1. 結論

今回の監査では、静的レビューだけでなく、ブラウザプレビュー、Tauri実機、3種類のウィンドウ幅、キーボード操作、大量データの実測、フロント・Rust双方の回帰テストを組み合わせた。

重大度の高かった次の問題は修正済みである。

- ライブラリのほぼ全ソート値がRust側で認識されず、保存日へサイレントにフォールバックしていた契約不一致
- 全文検索が1,000件前後で打ち切られ、後続結果へ到達できないページング欠落
- 5,000件smart検索が6.46〜6.48秒かかっていた検索パイプラインのCPU・メモリ負荷
- 大量作品を読み進めるほど全カードDOMが増え続ける一覧実装
- Tauri子WebViewのURL、bounds、User-Agent、認証callback、ライフサイクルの検証不足
- EPUBテンプレート名・ファイル名およびZIP/metadataパスからのディレクトリ脱出
- リモート子WebViewとDOMモーダルの奥行き競合
- HashRouterを壊すスキップリンク、選択モード中の隠れフォーカス、無名入力などのアクセシビリティ不具合
- Tauri外プレビューからTauri APIを呼び、生の例外を出す画面
- URL、localStorage、ブックマーク、作品ID、タブ、数値入力の破損値・境界値処理
- 高重要度のnpm依存脆弱性1件

通常回帰、警告ゼロlint、本番フロントビルド、Tauri debug production buildはすべて成功した。大量データ試験では20,000作品の一覧操作が145ms以下、5,000作品のsmart検索が708.164msとなった。

一方、「あらゆる環境・入力でバグが存在しない」ことを有限の監査で証明することはできない。今回確認できた残存リスクは第10節に明示した。特にZIP展開quota/原子的復元、単一の超巨大作品のIPC分割、子WebViewフォーカス中のショートカットは次段の優先項目である。

## 2. 監査範囲と方法

### コード・契約

- フロントのroute、query、mutation、永続化、入力検証、画像、空/loading/error状態
- Rust command公開面、URL/path検証、起動失敗、panic経路、SQLite query、cursor、sort契約
- Tauri capabilities、CSP、asset protocol、子WebView・認証windowの生成/破棄/遷移
- EPUB template、ZIP export/import、backup metadata
- Tantivy候補取得、検索後処理、読み・ローマ字・ngram fallback

### 実操作

次の11ルートを900×600、1200×800、1440×900の33組合せで再表示し、fatal error、main landmark、document横overflowを検査した。

- ホーム
- ライブラリ
- 作品詳細
- リーダー
- エディタ
- 作者詳細
- シリーズ詳細
- Webから保存
- EPUB
- 更新
- 設定

全33組合せでfatal errorなし、mainあり、document横overflow 0pxだった。ライブラリではgallery/list切替、検索中sort表示、複数選択、作者ページでは遅延ロード、保存画面では候補ペインとsplitterも操作した。

### Tauri実機

- Windows表示倍率150%
- 標準1200×800相当（物理1822×1256）
- 最小900×600相当（物理1372×956）
- 実際のpixivページを子WebView2で表示
- resize後のReact placeholderと子WebView bounds同期
- Spotlight表示時の子WebView非表示、モーダル前面描画、閉じた後のWebView復帰

標準・最小サイズとも、子WebView、アドレスバー、候補ペイン、splitter、下部操作へ到達できた。Spotlightは両サイズでWebViewより前に正しく表示された。

## 3. 主な修正 — Tauri / WebView / セキュリティ

### 認証と子WebView

- Pixiv/FANBOXごとの非blocking lockで認証windowの二重起動を防止
- window close/destroy時に待機中のoneshotを即座にcancel
- poisonされたmutexでpanicせずResultを返す構造へ変更
- staleな認証windowを破棄してから再作成
- callback origin/pathを厳密化
  - Pixiv: `pixiv://account/login` と許可したapp-api callbackのみ
  - FANBOX: `fanbox-auth://callback`のみ
- callback値の最大長と制御文字を検証
- embedded URLをhttp/httpsに限定し、credentials付きURLを拒否
- 子WebViewの全navigationをhttp/httpsへ制限
- boundsをfinite・正サイズ・妥当範囲で検証
- User-Agentの長さ・制御文字を検証
- FANBOX scriptからbrowserで禁止される`Origin`設定を削除
- host判定をsuffixの曖昧一致から正規hostname一致へ変更
- remote WebViewへTauri IPCを公開せず、URLは安全なpage-load通知とpollで同期
- 未使用の`notify_url_changed` commandを公開面から削除

### 奥行きとresize

子WebViewはmain WebViewとは別のnative surfaceなので、CSS `z-index`ではDOM modalより背面へ移せない。保存画面では、子WebView領域を複数点hit-testし、Spotlight、modal、drawerなどが重なる間だけnative WebViewを非表示にする。閉じた後は再表示する。

また、`ResizeObserver`、window resize、scroll、低頻度reconcileを併用し、候補ペインのdrag/collapse、DPI変更、window resize後もboundsを再同期する。drag中のIPCはanimation frame単位にcoalesceした。

### Tauri設定

- CSPから`script-src 'unsafe-inline'`を削除
- `base-uri 'none'`、`object-src 'none'`、`frame-src 'none'`を追加
- asset protocol公開範囲をダウンロード資産・プロフィール・シリーズ資産へ限定し、DBやtemplateを非公開化
- setup内の`expect`をcontext付きResultへ変更し、起動失敗時はmainが非zero終了
- EPUB exportのJSON parent `unwrap`をResultへ変更

### template / ZIP / metadata

- template名を英数・`_`・`-`へ限定
- template filenameを単一のNormal componentかつ許可suffixへ限定
- canonical pathがtemplate root直下か検証し、symlink脱出を拒否
- built-in templateを編集・削除不可に変更
- ZIP entryは標準`/`区切りの相対pathのみ許可
- ParentDir、RootDir、Windows Prefix、colon、backslash、制御文字、重複entryを拒否
- `create_dir_all`の前後でnearest existing ancestorをcanonicalizeし、期待root内か検証
- metadata内のicon、cover、JSON、original JSON、asset pathをDB mutation前に一括検証
- profiles、series、downloadsの期待root別resolverへ統一

## 4. 主な修正 — 大量ライブラリと検索

### 一覧

- gallery/compactを行単位で仮想化
- `.app-main`をscroll elementとして利用
- responsive列数、動的行高measure、overscanを実装
- viewport検出前も初期DOMを最大120カードへ制限
- 10,000件fixtureでDOMが有界であることを回帰テスト
- cover/avatar/entity画像へ`loading="lazy"`と`decoding="async"`を追加
- 作品詳細のassetを80件単位で表示
- 本文DOM parse、版preview、history/JSON取得を必要なtabまで遅延

### query / IPC

- 候補検索とfacet検索をlazy化・debounce
- search-as-you-type cacheのgc時間を短縮
- ライブラリ・entity infinite query cacheの保持時間を制限
- 全update targetをIPC転送して`.find()`していた処理を、Option返却の単件commandへ変更
- 一括favorite/watchを1件ずつinvokeせず、1transactionのbulk commandへ変更
- mutation後の全既読ページ再取得を避け、局所的なoptimistic updateへ変更
- 空bulk、空suggest、重複ID、無効ID、異常limit/offsetをフロント境界で抑止

### sortとcursor

フロントが送る次の値をRustの安全な許可リストへ正規化した。

- `downloaded_at`
- `source_created_at`
- `source_updated_at`
- `author_name`
- `text_length`
- `file_size_bytes`
- 既存の`title`、`date`、`published`、`author`、`size`、`series_order`

SQL列名は文字列を直接連結せず、固定mappingから選ぶ。不正sort値は既定値へ戻り、SQL injectionにならないことをテストした。

全文検索は固定1,000候補を撤廃した。TantivyからTopDocsと全hit数を取得し、cursor深度、filterの選択性、必要ページ数に応じて候補数を段階的に倍増する。1,025件を137件/pageで最後まで走査し、欠落・重複がないことを確認した。

全文検索中は関連度順が仕様であるため、効かない通常sort selectorを有効に見せず、disabled状態の「関連度順（検索中）」へ切り替える。

### smart検索高速化

- TopDocs取得済みstored documentを後段へ再利用し、N+1 document lookupを除去
- 候補文書を`Arc`共有し、大本文の複製を抑止
- direct/synonym一致を全fieldで先に検査
- Lindera、読み、ローマ字、ngramは直接一致がない語だけlazy fallback
- highlightもliteral一致を先行

かな・ローマ字一致とメタデータ優先順位を維持しつつ、5,000件smart検索を約9.1倍高速化した。

## 5. 主な修正 — UI / UX / アクセシビリティ

- HashRouterのrouteを破壊していた`href="#main-content"`をfocus buttonへ変更
- route lazy loadingへ`role="status"`、読み上げテキストを追加
- Loading/Error/RuntimeNoticeのlive region、alert、長文折返し、retryを統一
- ナビ開閉、戻る/進む、SegmentedControl、Slider、進捗へ日本語のaccessible nameを付与
- PageHeader、Settings navigation、toolbar、action groupを狭幅で折返し
- WorkCardの作者・シリーズ・タグ・version操作を24px以上のtargetへ拡大
- selection modeでは選択toggleだけをfocus可能にし、内部link/actionをinert・tabIndex -1・aria-hidden/disabled化
- Dashboardの見出し、空状態CTA、入力エラー関連付け、接続/索引error表示を追加
- Quick Save URLを文字列包含ではなく正規hostnameで判定し、偽装domainを拒否
- 保存候補splitterへArrow/Home/End操作とARIA valueを追加
- EPUBのqueue/template error、progress、slider名を追加
- UpdatesのNumberInput spinnerを除去し、整数範囲と実行前normalizationを追加
- update候補の保存対象を`candidate`/`failed`だけに限定
- 監視対象削除へ確認dialogを追加
- ブラウザpreviewではTauri Store、event購読、polling、認証/ファイル操作を実行しない
- 英語の生statusや`built-in`表記を日本語化
- 画面内の同種操作でbutton、focus ring、loading、error、empty stateの挙動を揃えた

## 6. 主な修正 — 状態・境界値・データ安全性

- URLをquery/tab/favorite/watch/sortの単一情報源へ整理
- URL、localStorage、saved search、filter、view modeをparse/normalize
- query長、tag数・長さ、saved search数を上限化
- min/max文字数の逆転を画面内で検証
- IME composition中の候補検索を停止
- entity source keyの二重`decodeURIComponent`を除去し、`%`を含む正常値の例外を修正
- 不正な作品ID、tab、route percent encodingを安全なNot Found/既定値へ処理
- bookmark localStorageから型不正entryを除去
- sessionStorage書込失敗をreader全体の例外へ波及させない
- editor linkをhttp/httpsだけに制限し、既存本文も表示前sanitize
- 保存中・未保存editがある状態のapp内navigationとTauri window closeへ確認を追加
- mutation pending中の二重操作をdisabled/optimistic表示で抑止

## 7. 性能測定結果

| ケース | 結果 | 判定 |
|---|---:|---|
| 20,000作品・先頭page | 144.7ms | 400ms基準内 |
| 20,000作品・深いkeyset page | 138.2ms | 400ms基準内 |
| 20,000作品・作者一覧 offset 300 | 42.6ms | 300ms基準内 |
| 20,000作品・軽量filter options | 17.8ms | 500ms基準内 |
| 5,000作品・smart全文検索 | 708.164ms | 1秒基準内 |
| 1,025件全文hitのcursor走査 | 全件到達・重複なし | 成功 |
| 10,000件virtual list | 初期DOM最大120カード | 成功 |

smart検索は修正前6.46〜6.48秒、修正後708.164msで、約9.1倍改善した。性能試験の時間にはfixture生成・索引構築を含めず、実際の検索区間だけを計測した。

## 8. 最終検証

| 検証 | 結果 |
|---|---|
| `npm test` | 17 files / 62 tests passed |
| `npm run build` | 成功 |
| `cargo fmt -- --check` | 成功 |
| `cargo test` | 73 passed / 0 failed / 2 ignored |
| ignored性能試験2件 | 個別実行で両方成功 |
| `cargo clippy --all-targets -- -D warnings` | 成功 |
| `npm audit --omit=dev` | 0 vulnerabilities |
| `tauri build --debug --no-bundle` | 成功 |
| `git diff --check` | 問題なし |
| browser route/viewport matrix | 33/33 fatalなし・横overflow 0px |
| Tauri native WebView | 標準/最小size、resize、overlay hide/restore成功 |

`npm audit`で検出した`nanoid 3.3.16`のhigh advisoryは3.3.18へ更新した。

## 9. 回帰テストとして追加・強化した領域

- routerの不正percent encodingとunsaved navigation guard
- bookmark破損localStorage
- editor link protocol
- Dashboard URL hostnameと状態表示
- Updatesの数値範囲・候補状態
- Library persist値、URL filter、検索中sort、表示形式ARIA
- Entity `%` keyと単件update target
- WorkCard selection modeのfocus/interaction
- VirtualizedWorkList 10,000件
- dbApi/searchApiのID、limit、空IPC
- sort alias/SQL mapping
- template traversal/built-in read-only
- ZIP entry・metadata traversal
- auth callback、URL、bounds、UA
- 1,025件全文cursor
- 5,000/20,000件performance smoke
- かな・ローマ字検索と検索順位

## 10. 残存リスク

### P1: backup importの資源制限と原子性

ZIP path traversalは防いだが、entry数、展開総量、圧縮率のquotaは未実装である。非常に大きい正規backupを許容する要件と両立する、設定可能quotaと空き容量preflightが必要である。また現状はlive storageへ展開してからmetadataを処理するため、後段失敗時に一部ファイルが残り得る。staging directoryへ展開・全検証し、DB transactionとatomic promote/rollbackを組み合わせるべきである。canonical checkから`File::create`までのsymlink/TOCTOUは、OS handle-relative openでさらに強化できる。

### P1: 単一の超巨大作品

一覧DOMとasset DOMは有界化したが、`ReaderDocument`はHTML、plain text、assets、versionsを1回のIPCで返す。数百MB級の単一作品ではRust/JSON/WebViewのpeak memoryが増える。本文page、asset、versionを別commandへ分割し、本文はchunk/streamingで渡す必要がある。

### P2: 長時間閲覧したinfinite-query cache

virtualizationによりDOMと画像decodeは有界だが、現在sessionで取得したpage data自体は閲覧件数に比例する。完全な固定memoryにはpage eviction、戻りcursor履歴、scroll anchor復元が必要である。

### P2: 極端に深い全文検索

深いcursorと非常に選択的なSQLite filterの組合せは、正確性を保つため最終的に全Tantivy hitへ候補を広げ得る。数十万hit向けにはTantivyのsearch-afterとsecondary fast-field cursorへ移行すべきである。

### P2: 子WebViewフォーカス中のshortcut

remote子WebViewがkeyboard focusを持つ間、main WebView内の`Ctrl+K`などは受信できない。headerの検索buttonは常に使え、overlayの奥行き自体は正しい。keyboardを完全に統一する場合は、appがactiveな間だけ有効なnative acceleratorをTauri側で実装する必要がある。

### P2: OAuth state

Pixiv独自login endpointが標準OAuth `state`をechoする公開仕様を確認できず、互換性を壊さないため未追加である。現状は毎回32-byte PKCE verifier、exact callback、provider別login serializationを用い、別flowのcodeはtoken exchangeで失敗する。仕様が公開されたらstateのconstant-time照合を追加する。

### P3: fuzzy facetの希少値

直接LIKE substringなら希少tag/authorも検索できる。純粋なかな・ローマ字・fuzzy fallbackは人気上位最大4,000候補へ限定しているため、候補外の希少値には届かない場合がある。facet専用の正規化列または小さな検索indexが次の解決策である。

### 実機網羅性

Windows 150% DPIの単一monitorで標準・最小sizeを確認した。100/125/175/200% DPI、monitor間移動、macOS WKWebView、Linux WebKitGTK、OS shutdown、実際の認証失敗/通信断は今回の自動試験範囲外である。pure validationとlifecycleはunit test済みだが、release前の実機matrixを推奨する。

## 11. 次期機能・UI/UX提言

優先順は次のとおり。

1. **原子的バックアップ復元**
   staging、quota、容量見積り、検証結果preview、rollbackを一つの復元wizardにまとめる。

2. **超巨大作品のstreaming reader**
   章単位取得、先読み、asset paging、reader内検索indexを導入し、単一作品のpeak memoryを固定化する。

3. **Tantivy native cursor**
   relevance scoreとfast fieldによるsearch-afterを実装し、数十万hitの深いpageでも候補再走査を避ける。

4. **大量ライブラリ診断画面**
   DB件数、索引件数、cache、保存容量、orphan asset、検索時間、再構築履歴を可視化し、保守操作を一か所へ集約する。

5. **native accelerator層**
   子WebView focus中もCommand Palette、戻る、保存候補取得などの限定shortcutを扱う。system-wide登録ではなくapp active時だけに限定する。

6. **実機visual regression CI**
   900×600、1200×800、1440×900、light/dark、100/150/200% DPIをscreenshot比較し、WebView overlayはWindows runnerのsmoke testで確認する。

7. **操作履歴と再実行**
   保存、更新、EPUB、backupを共通job modelへ寄せ、失敗理由、再試行、cancel、通知、ログ表示を同じinteractionに統一する。

8. **検索説明UI**
   relevance、exact、semanticの違い、索引完成度、filter後の概算件数を検索bar近くで短く説明し、深い検索の挙動を予測しやすくする。

## 12. 総評

今回の変更で、piepは「少量のpreview dataでは動く」状態から、Tauriのnative surface、破損永続値、1,000件超検索、10,000件DOM、20,000件DBを明示的に扱う構造へ進んだ。特にWebViewの奥行き問題はCSSではなくnative visibilityで解決し、検索は正確性を維持したまま実測で9倍以上高速化した。

次のボトルネックは一覧ではなく、単一巨大documentのIPCと、backup復元の資源・transaction境界である。次期作業はこの2点を最優先にすると、保存数・作品サイズの両方向でスケールする。

## 13. v0.3.0 追補 — 提言の実装完了

この節は、上記の「残存リスク」「次期機能・UI/UX提言」を実装した後の最終状態であり、該当する旧記述を更新する。

### 実ライブラリの再計測

ユーザーの実データを変更せず、診断APIと実アプリ画面で測定した。

| 項目 | 実測 |
|---|---:|
| 作品 | 919件 |
| 本文 | 21,711,943文字 |
| 版 | 919件 |
| アセット | 2,541件 |
| 検索索引完成度 | 914 / 919作品（99.5%） |
| SQLite物理サイズ | 約1.887GB |
| SQLite論理使用量 | 約4.8〜5.0MB |
| SQLite再利用待ち領域 | 99.73〜99.74% |
| 全文索引 | 約20.6〜22.1GB、5,920ファイル、986セグメント |
| 一覧先頭80件 | 今回10.0ms、p50 8.1ms、p95 10.1ms |
| 作者の完全一致 | 今回/p50 9.8ms、p95 14.4ms |
| 断片化索引での最多作者全文検索 | p50 1.40秒、cold/p95 39.81秒 |

919作品という件数自体は十分軽い。遅延原因は作品数ではなく、SQLiteの空きpageとTantivyの986セグメントである。DB圧縮と検索索引統合は別操作として診断画面に表示し、どちらも空き容量preflight、明示確認、進捗、操作履歴を備える。実データへは自動実行せず、利用者が診断値を見て選択できる。

### 検索意図と添付事例の修正

- 候補は種別を保持し、作品は作品詳細、作者は作者詳細、シリーズはシリーズ詳細へ直接遷移する。
- タグ候補は文字列全文検索ではなく構造化tag filterへ変換する。
- 完全一致の作者名・シリーズ名はSQLite relationで厳密に限定し、本文に同名が出る別作者作品を混ぜない。
- legacy query `pixiv:<series id>`もシリーズrelationとして解釈する。
- 検索bar直下に、SQLite厳密絞り込み、全文関連度、semanticなど実際に使った検索経路を表示する。
- 各作品cardに一致field/reasonを表示し、なぜ出たかを説明する。

添付事例と同じ`pixiv:12552619`を実ライブラリで操作し、修正前の0件ではなく該当シリーズ85作品を表示した。検索説明は「保存済みメタデータをSQLiteで厳密に絞り込みました」となり、Tantivyの曖昧検索を経由しない。

### 8提言の実装結果

1. **原子的バックアップ復元**
   復元wizardで、format、entry、圧縮率、総展開量、空き容量、作品/作者/シリーズ/版/asset件数をpreviewする。traversal、symlink、重複・case衝突、Windows予約名、参照欠落をDB mutation前に拒否する。専用staging、`BEGIN IMMEDIATE`、rollback backup付きfile promotionにより、失敗試験でDB・既存file・上書きfile・stagingが元へ戻ることを確認した。

2. **超巨大作品のstreaming reader**
   metadataと本文IPCを分離し、FANBOX/editorは128KiB block、pixivは原稿page単位で取得する。64MiB/8文書LRU、非active page eviction、全page本文検索（Ctrl+F、page/snippet/count、結果から移動）を追加した。

3. **Tantivy native cursor**
   relevance scoreとDocAddressをcursorへ含めたsearch-afterへ変更した。1,000件超とSQLite filter併用を、欠落・重複なしで最後まで走査する。

4. **大量ライブラリ診断画面**
   実データ7回のp50/p95、DB/WAL/page/free/cache、orphan DB row/物理file、process memory、storage/index容量、index file/segment/completionを表示する。SQLite optimize/VACUUMとTantivy mergeは独立した明示操作である。

5. **native accelerator層**
   WebView2 `AcceleratorKeyPressed`で、子/単独WebView focus中もAlt+←/→、F5/Ctrl+R、Ctrl+S、Ctrl+Wを処理する。Ctrl+SはURLだけをtrusted mainへ渡し、既存の候補確認を迂回しない。

6. **実機visual regression CI**
   900×600、1200×800、1440×900、light/dark、100/150/200% DPIの66基準画像を追加した。Windows runnerではTauri appを起動し、window寸法、WebView2 child surface、保存workspace shortcutをsmoke testする。

7. **操作履歴と再実行**
   保存、更新、EPUB、backup、restore、検索再構築、DB/index保守を操作履歴へ統合した。状態、進捗、cancel、retry、失敗理由、時系列logを同じinteractionで表示し、異常終了中のjobは次回起動時に`interrupted`として残す。

8. **検索説明UI**
   検索解釈、一致理由、索引完成度、検索中の関連度順固定をUIへ追加した。作者/シリーズのexact intentは厳密検索となり、明示名で無関係作品を混ぜない。

### 大きい単独Webウィンドウ

保存workspaceの「外部ブラウザで開く」を「大きいウィンドウで開く」へ変更した。providerごとに1枚を再利用し、現在monitor/DPIの88%×86%、最小800×560、最大1600×1000で生成する。HTTP(S)・host必須、credentials付きURLと無管理popupを拒否し、remote capability/IPCは与えない。Windows実機でmain 1822×1256に対し単独window 2062×1406、連打時の重複なし、focus再利用、Ctrl+W終了を確認した。

### v0.3.0最終検証

| 検証 | 結果 |
|---|---|
| TypeScript | 成功 |
| frontend unit/integration | 23 files / 75 tests passed |
| production build | 成功 |
| Rust unit/integration | 86 passed / 0 failed / 2 ignored |
| 20,000作品性能 | first 97.4ms / deepest 94.5ms / authors 29.0ms / facets 9.8ms |
| 5,000作品smart検索 | 528.4ms、1秒基準内 |
| Clippy `-D warnings` | 成功 |
| visual regression | 24 passed / 12意図的skip、66基準画像 |
| Windows native smoke | 1215×837、WebView2 child surface検出 |
| `npm audit --omit=dev` | 0 vulnerabilities |
| Tauri debug no-bundle build | 成功 |
| `git diff --check` | errorなし（Windows改行noticeのみ） |

### 物理的な境界

- 数百MB級の単一作品は、初回parse時だけraw JSONとHTML 1文書分のbackend peak memoryを必要とする。以後のIPC、再parse、WebView DOMはpage/LRUで有界化した。
- SQLiteとfilesystemを同時にcommitするOS横断の単一primitiveはないため、復元はtransactionとrollback journalを組み合わせる。攻撃・途中失敗試験で復元性を確認済みである。
- 実データの22GB全文索引と1.8GB DBはユーザーデータを勝手に書き換えない方針で未圧縮である。診断画面の明示操作は容量検査・確認後に原子的に実行される。
