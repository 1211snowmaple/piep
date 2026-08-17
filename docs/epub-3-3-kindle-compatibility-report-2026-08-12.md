# EPUB 3.3・Send to Kindle 互換性 再設計／検証レポート

- 対象: piep 0.4.0 開発版
- 調査・実装・検証日: 2026-08-12
- 対象機能: pixiv / pixivFANBOX 保存データからの EPUB 生成、EPUB 検証、テンプレート管理
- 結論: 保存済みの Pixiv / FANBOX 実データから生成した 2 冊は、公式 EPUBCheck 5.3.0 で fatal 0 / error 0 / warning 0 / usage 0 になった。構造上は EPUB 3.3 適合を確認できた。Send to Kindle についても公開ガイドラインに合わせているが、Amazon の変換処理は非公開かつ将来変更されるため、実サービスへの送信成功までを仕様だけで永久保証するものではない。

## 1. 基準にした仕様とガイドライン

2026-08-12 時点で、次を基準とした。

| 基準 | 採用内容 |
|---|---|
| [EPUB 3.3 W3C Recommendation](https://www.w3.org/TR/epub-33/) | OCF/ZIP、Package Document、Manifest、Spine、Navigation Document、XHTML、CSS の適合条件。現行 Recommendation は 2026-01-13 版。 |
| [EPUB Accessibility 1.2](https://www.w3.org/TR/epub-a11y-12/) | `accessMode`、`accessModeSufficient`、`accessibilityFeature`、`accessibilityHazard`、要約の付与。現時点では WCAG 適合を宣言せず、生成物について真実な範囲だけを記述する。 |
| [EPUBCheck 5.3.0](https://github.com/w3c/epubcheck/releases/tag/v5.3.0) | EPUB 3.3 の公式リファレンス検証器。内部検証とは別に、実生成物の最終判定に利用。 |
| [Send to Kindle 対応形式](https://www.amazon.com/sendtokindle) / [Amazon ヘルプ](https://digprjsurvey.amazon.com/csad/help/node/TCUBEdEkbIhK07ysFu) | EPUB を受け付け、1 ファイルの上限は 200 MB。 |
| [Amazon Kindle Publishing Guidelines](https://kdp.amazon.com/en_US/help/topic/GU72M65VRFPH43L6) | Kindle 変換で問題になりやすい目次、表紙、画像、HTML/CSS の実装判断。 |
| [Kindle の目次ガイド](https://kdp.amazon.com/en_US/help/topic/GY3AD8C6C6GAG42N) | 論理目次を完全にする。HTML 目次を本文から開けるようにし、EPUB 3 Navigation Document と旧 NCX の両方を用意する。 |
| [Kindle の画像ガイド](https://kdp.amazon.com/en_US/help/topic/G75V4YX5X8GRGXWV) | 内部表紙は全ページ表示、幅または高さ 1200 px 以上を推奨。画像には代替テキストを持たせる。表紙で SVG 依存を避ける。 |
| [Kindle の HTML/CSS ガイド](https://kdp.amazon.com/en_US/help/topic/G200673220) | 通常本文でフォント、文字色、背景色、行高などを過度に固定せず、読者設定を尊重する。 |
| [Kindle の品質確認ガイド](https://kdp.amazon.com/en_US/help/topic/GGRXLC5USU4H67YM) | 1 HTML 30 MB 未満、HTML ファイル 300 未満、Kindle Previewer 等での確認を推奨。 |

## 2. EPUB 3.3 適合の実装状況

### 2.1 OCF / ZIP コンテナ

| 要件 | 実装 |
|---|---|
| `mimetype` が ZIP の先頭 | ビルダーが必ず最初に書き込む。 |
| 内容が正確に `application/epub+zip` | BOM・改行・空白なしで 20 byte を書く。 |
| `mimetype` は無圧縮 | `Stored` で格納する。 |
| `META-INF/container.xml` | ルートファイル `OEBPS/content.opf` を宣言する。 |
| 参照の一意性 | ZIP 内の重複パス、Manifest の重複 href / id、XML ID の衝突を生成時・検証時に防ぐ。 |

### 2.2 Package Document / 書誌

- Package version は EPUB 3.3。
- `unique-identifier` と `dc:identifier` の参照を一致させ、作品ごとに安定した `urn:pixiv:novel:{id}` / `urn:fanbox:post:{id}` を使う。
- 必須の `dc:title`、`dc:identifier`、`dc:language`、`dcterms:modified` を必ず 1 つ以上持たせる。
- `dcterms:modified` は投稿の更新日ではなく、その EPUB パッケージを生成した時刻を UTC の `YYYY-MM-DDThh:mm:ssZ` で記録する。
- 作者、作者 role、publisher、公開日、source URL、タグ、概要、シリーズ名・巻数を利用可能な範囲で付与する。
- 表紙画像には EPUB 3 の `properties="cover-image"` と、旧 Kindle 系取り込み用の `<meta name="cover">` を併記する。
- 画像のある本について `textual` だけで十分とは宣言せず、挿絵がある場合は `textual,visual` とする。挿絵の内容を安全に推測できないため、アクセシビリティ要約には「説明的な代替テキストがない」ことを明示する。

### 2.3 Manifest / Spine / Navigation

- すべての XHTML、CSS、画像、NCX を Manifest に一意の ID・正しい media type で登録する。
- EPUB 3 の `nav.xhtml` は `properties="nav"` を持つ項目をちょうど 1 つにする。
- `nav.xhtml` 内の `epub:type="toc"` も 1 つだけにし、Spine の本文順と同じ順序で並べる。
- Kindle で HTML 目次としても開けるよう、`nav.xhtml` 自体を Spine に入れる。
- 表紙、作品情報、目次、本文を読み進む順に並べ、設定で表紙を `linear="no"` にできる。
- 旧取り込み経路向けに NCX を標準で併記する。親ページと子章で `playOrder` が重複せず、1 から欠番なしで増えるよう修正した。
- landmarks は存在する Spine 項目だけを指し、表紙・作品情報・本文を正しく分類する。

### 2.4 XHTML / CSS / リソース

- 配信元の HTML 断片をそのまま埋め込まず、許可タグ・許可属性だけの整形式 XHTML に再構築する。
- 閉じ忘れ、誤った入れ子、重複 ID、XML 禁止制御文字、未知属性、script / iframe / embed を除去または修復する。
- テキスト、属性、URL を用途別に XML エスケープする。テンプレートの auto-escape に依存しない。
- Pixiv 記法の `[newpage]`、`[chapter:]`、`[pixivimage:]`、`[jumpuri:]`、`[[rb:...]]` をページ、章、挿絵、リンク、ruby に変換する。
- `pixiv://users/...`、`pixiv://illusts/...`、`pixiv://novels/...` を公開 HTTPS URL へ変換し、非対応カスタムスキームは外す。
- 画像参照は XHTML からの相対 URL として percent-encode し、Manifest にない画像参照は残さない。
- 表紙ページは直接 `<img>` を使い、既定状態で SVG に依存しない。カスタム表紙テンプレートが実際に SVG を含む場合だけ Manifest に `svg` property を付ける。
- 既定 CSS は通常本文のフォント、文字色、背景色、行高を強制しない。利用者が明示的にカスタマイズした項目だけを追加 CSS として保存する。

## 3. 以前の生成物で確認した主な問題と修正

| 問題 | 影響 | 修正 |
|---|---|---|
| OPF / XHTML の escaping が不完全 | `&` や `<` を含む題名・作者名で XML が壊れる | XML 用 escape と XHTML sanitizer を分離して全面適用。 |
| 必須時刻の形式と意味が不正確 | `dcterms:modified` で検証エラー、または投稿更新日と EPUB 更新日の混同 | ビルド時刻を秒精度 UTC で 1 つ出力。 |
| Navigation Document の landmark が Spine 外を参照 | EPUBCheck `RSC-011` | nav を Spine に含め、landmark の対象を実在項目に限定。 |
| NCX の `playOrder` が親子で重複・欠番 | FANBOX の章付き投稿で EPUBCheck エラー | 親の順番を子の採番前に確定し、深さにかかわらず連番化。 |
| `pixiv://` URL が残る | EPUBCheck warning、Kindle で開けないリンク | 公開 HTTPS URL へ正規化。 |
| 保存済み標準テンプレートが古いまま | コード更新後も利用者環境では古い OPF/nav を使い続ける | 起動時に全組み込みテンプレートファイルを現行版で同期。カスタムテンプレートは保持。 |
| 同名作品が同じ出力名になる | 一括書き出しで前の EPUB を無言上書き | `{タイトル} [pixiv-{id}].epub` / `{タイトル} [fanbox-{id}].epub` に変更。 |
| 生成後の検証結果を成功扱い | 壊れた本がキューから消える | 無効な本は `invalidIds` として返し、EPUB キューに残す。単冊書き出しは検証エラーとして失敗させる。 |

## 4. Pixiv / FANBOX から抽出できる情報

変換層は配信元固有 JSON を共通の `EpubManifest` に正規化する。テンプレートスタジオの「差し込める項目」はこの中間形式から自動生成されるので、手書きの項目一覧と実装がずれない。

### 4.1 共通フィールド

| グループ | 主な値 | 配置例 |
|---|---|---|
| `core` | 識別子、題名、作者名・ID・アカウント・URL・アイコン、紹介文 XHTML / plain text、タグ、翻訳タグ、公開・更新日時、元ページ URL、シリーズ、言語、publisher | OPF 書誌、表紙、作品情報、ヘッダー、奥付相当 |
| `provider` | source、Pixiv 小説 ID、FANBOX 投稿 ID、シリーズ ID、FANBOX 投稿 type | 出典表示、条件分岐、プロバイダー別装飾 |
| `stats` | 文字数、ページ数、章数、画像数、添付数、いいね / ブックマーク、コメント数、必要支援額、成人向けフラグ | 作品情報、バッジ、条件付き行 |
| `content` | 表紙画像、本文ページ、章、挿絵、添付ファイル、文字数 | 表紙、本文、目次、添付一覧 |

コード編集では `{{ core.name }}`、`{{ core.author.name }}`、`{{ stats.likeCount }}` のように参照でき、配列は MiniJinja の loop / if で配置できる。

### 4.2 Pixiv

- `detail.id/title/caption/create_date/update_date/text_length`
- `detail.user.id/name/account/profile_image_url`
- `tags[].name/translated_name`
- 実際の保存形式に存在する `series_id/series_title/series_order`、`seriesNavigation`、`isPartOf` の各形状
- `total_bookmarks`、`total_comments`、成人向け判定
- 本文のページ区切り・章・ruby・外部リンク・Pixiv 画像参照
- 保存済み cover と illustrations。画像 ID とページ番号を正規化し、本文内の参照とローカルファイルを結び付ける

### 4.3 pixivFANBOX

- `id/title/excerpt/publishedDatetime/updatedDatetime/type/tags`
- `creatorId`、`user.userId/name/iconUrl`
- `likeCount/commentCount/feeRequired/hasAdultContent`
- article の `blocks`: paragraph、header、image、file、URL embed
- text / image / file 投稿の `body.text`、`body.images`、`body.files`
- `imageMap/fileMap/urlEmbedMap`
- `styles` と `links` の UTF-16 offset/length。JavaScript の文字位置と同じ UTF-16 単位で bold / italic / link 等を正しい文字範囲へ適用する
- 添付ファイルと URL embed は、危険な iframe やバイナリを本文へ埋め込まず、名称と安全なリンクとして表示する

## 5. テンプレートスタジオ

右サイドバーの付属機能ではなく、`/epub/templates` の独立画面に分離した。左ナビゲーションには「EPUB キュー」と「テンプレートスタジオ」を別項目として置き、双方の画面にも往復ボタンを用意した。

### 5.1 管理

- 標準 / Pixiv / FANBOX の 3 組み込みテンプレート
- 組み込みテンプレートは読み取り専用。複製してカスタム版を作る
- 作成、複製元選択、名前変更、削除、ファイル単位の既定値復元
- 自動選択対象を Pixiv / FANBOX ごとに指定
- テンプレート名とファイル名は許可文字と既知ファイルに限定し、path traversal を拒否

### 5.2 見た目編集

- 書体、本文サイズ、余白、行間、段落間隔、字下げ、両端揃え、縦書き、ruby サイズ
- 本文色、背景色、リンク / タグ色、補助文字色
- タイトル・章見出しの位置 / 大きさ / 罫線
- 表紙幅・角丸、挿絵幅
- 視覚設定は `style.css.j2` の管理ブロックだけを書き換え、それ以外の手書き CSS を保持

### 5.3 構成編集

- 表紙ページ、表紙の linear、作品情報、章目次、NCX の有無
- 左綴じ / 右綴じ、BCP 47 言語タグ
- 作品情報ページの 18 項目を有効化、並べ替え、見出し変更
- 目次、表紙、作品情報、本文、ページ等の表示文言

### 5.4 コード編集とプレビュー

次の 8 ファイルを細部まで直接編集できる。

1. `style.css.j2`
2. `_base_style.css.j2`
3. `cover_page.xhtml.j2`
4. `info_page.xhtml.j2`
5. `page_wrapper.xhtml.j2`
6. `nav.xhtml.j2`
7. `toc.ncx.j2`
8. `content.opf.j2`

保存時にテンプレートを構文検査し、壊れた MiniJinja を拒否する。作品情報、表紙、本文、目次は iframe、OPF はソース表示で確認できる。アプリ内ではライブラリの実作品を選び、その作品から取れた値とプレビューを同時に確認できる。

## 6. アプリ内 EPUB 検証

公式 EPUBCheck をアプリへ同梱せず、書き出し直後に軽量な内部検証を必ず実行する。検査対象は次のとおり。

- ZIP 先頭 entry、mimetype の内容・圧縮方式、重複 entry
- container.xml、rootfile、OPF、全 XML/XHTML の整形式性
- 必須 metadata、時刻書式、unique identifier
- Manifest の ID / href / media type /実ファイル、Spine idref
- nav property / toc nav の個数、nav の Spine 登録、目次リンクと fragment
- NCX `playOrder` の重複・欠番
- cover-image の個数、表紙実体、画像寸法
- XHTML 内の画像・リンク参照、危険 URL
- Send to Kindle 200 MB 上限
- Kindle 推奨の HTML 300 未満、単一 HTML 30 MiB 未満、内部表紙 1200 px 以上

内部検証の error は書き出し失敗として扱い、warning は結果画面に表示する。warning のある EPUB は構造上 valid でも、利用者が品質上の注意を把握できる。

## 7. 実データでの検証結果

### 7.1 公式 EPUBCheck 5.3.0

| 入力 | 生成物 | 内容 | Fatal | Error | Warning | Usage |
|---|---:|---:|---:|---:|---:|---:|
| 保存済み Pixiv 小説 JSON + assets | 6,850,858 bytes | 本文 2 ページ、挿絵 2、表紙 1 | 0 | 0 | 0 | 0 |
| 保存済み FANBOX article JSON + assets | 47,847,955 bytes | 本文 1 ページ、挿絵 19、表紙 1 | 0 | 0 | 0 | 0 |

いずれも EPUB 3.3、reflowable、script なし、暗号化なしとして認識された。Pixiv は Spine 5 項目、FANBOX は Spine 4 項目で、nav と NCX の参照解決も完了した。

### 7.2 内部検証

- FANBOX: valid、指摘 0。
- Pixiv: valid。保存済み元表紙が実寸 240 × 180 px だったため `KINDLE-COVER-SMALL` warning 1 件。ファイル名に `master1200` が含まれていても、実画素を読んで判定する。
- 低解像度画像を自動拡大しても情報量は増えず、ファイルサイズとぼやけだけが増すため、生成側では引き伸ばさない。可能なら取得時に高解像度表紙を保存するのが正しい対応。

### 7.3 自動テストと UI 確認

- Rust library: 150 tests 実行、145 passed / 0 failed / 5 ignored。EPUB の converter / builder / sanitizer / template / validator を含む。
- Frontend: 33 test files、185 tests passed。
- TypeScript production build: 成功。
- Rust example `epub_probe`: build 成功。保存 JSON → 中間形式 → EPUB → 内部検証をアプリ本体と同じコード経路で再現可能。
- ブラウザ実動確認: テンプレートスタジオの構成・見た目・コード・差し込み項目、プレビュー、EPUB キューとの往復を確認。console warning / error 0。
- `git diff --check`: whitespace error 0。

## 8. 残る制約と運用上の確認

1. Send to Kindle への実送信は Amazon アカウントへの外部操作になるため、この検証では実施していない。仕様適合、EPUBCheck、Amazon 公開ガイドラインの 3 層で受け入れ可能性を高めている。
2. リリース前は代表的な横書き・縦書き・長文・大量画像の EPUB を [Kindle Previewer](https://kdp.amazon.com/en_US/help/topic/G202131170) でも目視することを推奨する。
3. 画像内容から正確な代替テキストを自動生成することはできない。現在はファイル由来の短い alt と、アクセシビリティ要約で限界を明示している。WCAG / EPUB Accessibility 適合を名乗るには、人による説明文の編集 UI と監査が別途必要。
4. FANBOX の添付バイナリは EPUB 内に複製せず、一覧情報と利用可能なリンクだけを残す。大容量化・実行ファイル混入・権限付き URL の漏えいを避けるための意図的な制限。
5. EPUBCheck が 0 件でも、配信先固有の変換・規約・コンテンツ審査まで保証するものではない。EPUBCheck の版を更新したときは、同じ実データ probe を再実行する。

## 9. 再検証手順

開発用 probe は `src-tauri/examples/epub_probe.rs` にある。

```powershell
cargo run --manifest-path src-tauri/Cargo.toml --example epub_probe -- `
  <original.json> pixiv <data_assets> pixiv <output.epub>
```

公式 EPUBCheck は次の形で実行する。

```powershell
java -jar epubcheck-5.3.0/epubcheck.jar --json result.json output.epub
```

合格条件は process exit code 0 かつ `nFatal = 0`、`nError = 0`、`nWarning = 0`。内部品質 warning は EPUBCheck と別に確認する。

## 10. 判定

今回の実装は「ZIP に HTML を詰めたファイル」から、EPUB 3.3 のコンテナ・書誌・目次・読み順・XHTML・アクセシビリティ metadata を一貫して生成し、出力後に検証結果まで利用者へ返す構成へ更新された。実データ 2 系統で公式検証を通過しており、以前の Send to Kindle 非認識につながっていた具体的な構造不備は解消済みと判断する。

今後の互換性維持では、EPUBCheck の更新追随、Kindle Previewer での代表冊子確認、低解像度表紙の取得改善を継続項目とする。
