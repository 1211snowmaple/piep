# 複数の作品を 1 冊の EPUB にまとめる — 設計提案レポート

実施日: 2026-08-16
状態: **提案のみ。コードは変更していない。2026-08-17 に「いったん保留」と決定した**（設計はそのまま使える状態で残す）。
対象: `src-tauri/src/epub/`（builder / template / intermediate）、`src-tauri/src/commands/epub.rs`、`src/features/epub/EpubPage.tsx`、`src/app/WorkspaceContext.tsx`

発展提案: [作品コレクションを中核にした読書・整理・EPUB統合 — 設計提案レポート](work-collections-reader-epub-design-proposal-2026-08-17.md)（本書の EPUB ビルダー設計を残しつつ、作品のくくりをライブラリとリーダーへ拡張）

---

## 1. 何を作るか

**選んだ作品を、選んだ順に、1 つの EPUB ファイルにする。**

- シリーズであるかどうかは条件にしない。pixiv と FANBOX をまたいでもよい。
- 順序は利用者が決めたもの（EPUB キューの並び）をそのまま使う。
- 「1 作品＝1 冊」の現行動作は残す。統合は選択肢として増える。

シリーズ単位・作者単位のまとめは、この一般形の上に「並べ方の初期値を作る機能」として乗る。

---

## 2. 現状の作りと、そのままでは足りないところ

### 2-1. いまの流れ

```
export_epub_batch(download_ids)
  └ 作品ごとに spawn_blocking（最大 MAX_CONCURRENT_EPUB_BUILDS 並列）
      ├ load_manifest      … JSON を EpubManifest ひとつに変換
      ├ resolve_template   … "__auto__" なら作品の取得元でテンプレートを選ぶ
      ├ EpubBuilder::build … 1 冊書く
      ├ validate           … 書いた EPUB を開き直して検証
      └ publish            … 一時ファイルを原子的に置換
```

### 2-2. 好都合な事実

| 事実 | 根拠 |
|---|---|
| **選んだ順はもう保持されている** | `WorkspaceContext` のキューは追加順の配列。`get_downloads` は「呼び出し側の順序を保つ」よう並べ直している（`queries.rs`） |
| ID とファイル名の一意化は既にビルダーの仕事 | `unique_id` / `unique_name` が `HashSet` で衝突を潰す。**同一ビルド内なら作品が増えても衝突しない** |
| 目次は既に 2 階層を表現できる | `NavEntry.children`、`toc.ncx` の `depth` 算出、`chapterToc` 設定 |
| 検証・原子的公開・失敗時の後始末は再利用できる | `build_validate_and_publish` / `StagedEpub` は「1 ファイルを作る」抽象で、統合でもそのまま使える |

### 2-3. 足りないもの

| # | 箇所 | 問題 |
|---|---|---|
| G1 | `EpubBuilder` | `manifest: EpubManifest` を 1 つしか持てない |
| G2 | `page_filename(order)` | 作品内でしか一意でない（`page_001.xhtml` が作品数だけ衝突する） |
| G3 | `package_images` | `is_cover` の画像が複数になる。`properties="cover-image"` は **1 冊に 1 つだけ** |
| G4 | `content.opf.j2` | `dc:title` / `dc:creator` / `dc:identifier` が `core`（＝単一作品）から来る |
| G5 | テンプレート解決 | 作品ごとに違うテンプレートを選べるが、**CSS は 1 枚**なので 1 冊に 1 テンプレートしか使えない |
| G6 | 画像の上限 | 合計 128 MiB / 10,000 枚（`builder.rs` の定数）。数百件を束ねると現実的に当たる |
| G7 | UI | キューに並べ替えがない（追加順のみ）。まとめる/分けるの選択肢もない |
| G8 | 進捗 | `ExportProgress` が「冊」単位。統合では「N 作品を 1 冊に」という進み方になる |

---

## 3. 設計の分岐点

| 論点 | 選択肢 | 提案する決定 |
|---|---|---|
| **A. どこで束ねるか** | ①中間表現（`EpubManifest` の集合）を 1 冊として組む ②書き出した EPUB 同士を後から結合する | **①**。②は OPF・目次・ID・CSS をすべて解析し直すことになり、実質もう一度ビルダーを書くのと同じ。①なら `EpubBuilder` の拡張で済む |
| **B. 作品の境界の表し方** | ①作品ごとに扉ページ ②見出しだけ ③何も置かない | **①を既定、②を設定で選べる**（`perWorkTitlePage`） |
| **C. 目次の階層** | ①作品のみ（1 階層） ②作品＞ページ ③作品＞ページ＞章 | **②を既定**。③は `tocDepth: 3` で選択制（Kindle は深い目次を苦手とする） |
| **D. 表紙** | ①先頭作品の表紙 ②生成した合本表紙 ③表紙なし | **①を既定、②を選択制**（[表紙プレースホルダーの提案](cover-placeholder-design-proposals-2026-08-16.md) の生成扉を流用） |
| **E. テンプレート** | ①1 冊 1 テンプレート ②作品ごとに切り替え | **①**。ただし `body` に `data-source="pixiv|fanbox"` を出し、CSS 側で取得元ごとの差を付けられるようにする |
| **F. 識別子の安定性** | ①毎回 UUID ②選択内容から決定論的に導く | **②**。`sha256("piep:collection:" + source:id を順に連結)` の先頭 32 桁。同じ本を書き出し直しても書架で重複しない |
| **G. 大きさの上限** | ①上限で失敗させる ②自動で分冊する | **②を選択制**（`splitAt`）。既定は「1 冊」。Send to Kindle は 1 ファイル 200 MB、Kindle の品質ガイドは HTML 300 ファイル未満を推奨 |
| **H. 途中の失敗** | ①1 作品でも失敗したら中止 ②失敗した作品を飛ばして続行 | **②を既定**。飛ばした作品は結果に列挙し、巻末の出典一覧にも「収録できなかった作品」として残す |

---

## 4. 統合の方式 — 7案

### M1. Flat Concat（単純連結）

- 各作品の本文ページを、選んだ順に並べるだけ。作品ごとの情報ページは扉として先頭に挟む。
- 目次は「作品名」の 1 階層。
- **最小の実装で「選んだ順に 1 冊」という要求を満たす。**
- 弱点: 数百ページの本で、作品内のページに直接飛べない。

### M2. Two-Level（部・章の二層）

- 作品＝部、作品内のページ＝章。目次は 2 階層（`NavEntry.children` にページを入れる）。
- 章見出し（`chapterToc`）まで含めると 3 階層になるため、`tocDepth` で切る。
- **既定として推奨。** 既存のデータ構造でそのまま表現できる。

### M3. Anthology（合本の体裁）

- M2 に加えて、巻頭に**総合情報ページ**（収録作品の一覧・総文字数・期間・作者一覧）、巻末に**出典一覧**（各作品の URL・公開日・取得日）を置く。
- 「あとで何が入っているか分かる本」になる。テンプレートに `collection_info.xhtml.j2` と `colophon.xhtml.j2` が増える。
- [テンプレート提案 D-3（奥付）](epub-template-design-proposals-2026-08-16.md) と同じ部品。

### M4. Auto Grouping（自動でまとめ方を決める）

- キューの中身を `source + seriesId` で束ね、シリーズ内は話数順、シリーズ外は選択順。
- あくまで**キューの並べ替えの初期値を作る機能**として実装し、最終的な順序は利用者が触れる（要求どおり「選んだ順」が最終権限を持つ）。
- 「シリーズごとに 1 冊ずつ書き出す」（＝N 冊の統合出力）もこの延長。

### M5. Reading Mode（読み物モード）

- 作品ごとの情報ページを省き、扉は「表題＋作者」だけの 1 行見出しにする。連載を一気に読むための体裁。
- `perWorkTitlePage: "minimal"` として M2 と両立。

### M6. Per-Work Style Scope（取得元混在の体裁）

- `body` に `data-source` と `data-work-index` を出し、CSS で pixiv 由来と FANBOX 由来を書き分ける。
- 横断統合（pixiv の小説と FANBOX の記事を 1 冊に）でも、どちらの世界の文章かが読者に分かる。

### M7. Auto Split（分冊）

- 「ページ数」「収録画像の合計サイズ」「作品数」のいずれかがしきい値を超えたら、`_1` `_2` … と分けて出す。
- G6（128 MiB / 10,000 枚）と Kindle の実務上限に対する現実解。

### 比較

| 案 | 実装量 | 読みやすさ | 前提 | 位置づけ |
|---|---|---|---|---|
| M1 | 小 | △ | なし | v1 の土台 |
| M2 | 小〜中 | ◎ | M1 | **v1 で入れる** |
| M3 | 中 | ◎ | M2 | v2 |
| M4 | 中 | ○ | UI 側 | v2 |
| M5 | 小 | ○ | M2 | v1 の設定として |
| M6 | 小 | ○ | M2 | v1 の設定として |
| M7 | 中 | — | M2 | v3 |

---

## 5. 推奨する実装

### 5-1. 中間表現に「冊」を足す

```rust
// intermediate.rs
/// 1 冊にまとめる単位。1 作品だけの場合も表現できる。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpubCollection {
    /// 収録順。ここに入った順序がそのまま読み進み順になる。
    pub works: Vec<EpubManifest>,
    pub meta: CollectionMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionMeta {
    pub id_: String,           // urn:piep:collection:<hash>
    pub name: String,          // 利用者が入力、または自動生成
    pub creator: String,       // 単一作者ならその名前、複数なら「複数の作者」
    pub contributors: Vec<String>,
    pub language: String,      // 収録作品の最頻値
    pub date_published: Option<String>, // 最も古い公開日
    pub description: Option<String>,
    pub series: Option<EpubSeries>,     // 全作品が同一シリーズのときだけ
}
```

`EpubBuilder` は `EpubCollection` を受け取る形に変え、1 作品の書き出しは `EpubCollection { works: vec![manifest], .. }` として通す。**分岐を 2 本持たないことが、検証済みの品質を保つ最短路**である。

### 5-2. 名前空間の付け方

`w{index:02}` を接頭辞にする（index は 1 始まり）。

| 対象 | いま | 統合後 |
|---|---|---|
| 本文 | `text/page_001.xhtml` | `text/w01_page_001.xhtml` |
| 作品扉 | `text/info.xhtml` | `text/w01_title.xhtml` |
| 画像 | `images/cover.jpg` | `images/w01_cover.jpg` |
| manifest id | `page-001` | `w01-page-001` |
| 画像 id | `img-123_p0` | `w01-img-123_p0` |
| 合本の情報ページ | — | `text/collection.xhtml` |
| 巻末の出典 | — | `text/colophon.xhtml` |

`unique_id` / `unique_name` はそのまま最後の砦として残す（接頭辞を付けても、同じ作品を 2 回キューに入れれば衝突しうる）。

**表紙は 1 枚だけ。** `is_cover` は先頭作品（または利用者が選んだ作品）の 1 枚にのみ立て、他作品の表紙は通常画像として扱い、その作品の扉に載せる。`<meta name="cover">` も 1 つ。

### 5-3. spine と目次

```
cover.xhtml            （任意）
collection.xhtml       合本の情報ページ（M3）
nav.xhtml              目次
w01_title.xhtml        作品1の扉
w01_page_001.xhtml …   作品1の本文
w02_title.xhtml        作品2の扉
w02_page_001.xhtml …
…
colophon.xhtml         出典一覧（M3）
```

目次は `NavEntry` を 2 階層で組む。

```
1. 星を編む人 第十二話          → w01_title.xhtml
     1.1 ページ 1               → w01_page_001.xhtml
     1.2 ページ 2
2. 制作ノート #25               → w02_title.xhtml
```

`tocDepth: 3` のときだけ、ページの下に章見出しを足す。NCX の `playOrder` は既存の採番ロジック（親→子の順で単調増加）をそのまま使う。

### 5-4. テンプレート側

追加するファイル（既定テンプレートに置き、pixiv / FANBOX は継承）:

| ファイル | 役割 |
|---|---|
| `collection_info.xhtml.j2` | 合本の情報ページ。収録一覧、総文字数、期間 |
| `work_title.xhtml.j2` | 作品ごとの扉。`info_page.xhtml.j2` の縮約版で、`work` と `work_index` を受け取る |
| `colophon.xhtml.j2` | 出典一覧。作品ごとに URL・公開日・取得日 |

レンダリング文脈には `collection`（`CollectionMeta`）と `works`（一覧）を足す。既存テンプレートの `core` / `content` は**その作品のもの**を指したままにする（互換性のため）。

`template.json` に足す設定:

```jsonc
{
  "merge": {
    "perWorkTitlePage": "full",   // "full" | "minimal" | "none"
    "tocDepth": 2,                // 1 | 2 | 3
    "collectionInfoPage": true,
    "colophon": true,
    "coverFrom": "first",         // "first" | "generated" | "none"
    "splitAt": { "works": 0, "pages": 0, "megabytes": 0 }  // 0 は無効
  }
}
```

### 5-5. コマンドと UI

```rust
#[tauri::command]
pub async fn export_epub_merged(
    app: tauri::AppHandle,
    download_ids: Vec<i64>,      // 並びがそのまま収録順
    template_name: String,
    output_dir: String,
    title: Option<String>,       // 未指定なら自動生成
    compress_options: Option<ImageCompressOptions>,
) -> Result<ExportBatchResult, String>
```

- 変換（`load_manifest`）は今と同じく並列でよい。**ビルドは 1 本**なので、`spawn_blocking` の中で `EpubCollection` を組んでから 1 回だけ `build` する。
- 進捗は `phase` を再利用する: `converting`（i/N の作品変換）→ `building`（1 冊の組み立て）→ `compressing`（画像）→ `completed`。
- 失敗した作品は飛ばし、`failed_ids` に積む（H の決定）。**1 冊も収録できなければ失敗**とする。

UI（`EpubPage.tsx`）:

1. 「1 冊にまとめる」トグル。オンにするとタイトル入力欄と、まとめ方（連結／シリーズごと／作者ごと）が出る。
2. キューの各行に**並べ替え**（上下ボタン＋ドラッグ）。キーボードだけで動かせること。
3. 「シリーズ順に並べる」「保存順に並べる」「タイトル順に並べる」のワンタッチ整列。
4. 見積り表示: 収録作品数・総文字数・画像の合計サイズ・推定ページ数、および上限に近いときの警告。

`WorkspaceContext` は既に順序付き配列なので、`reorderEpubQueue(from, to)` を足すだけで済む。

---

## 6. メタデータの決め方

| 項目 | 規則 |
|---|---|
| `dc:title` | 利用者の入力。未入力なら「＜先頭作品の題＞ 他 N 編」。全作品が同一シリーズなら「＜シリーズ名＞」 |
| `dc:identifier` | `urn:piep:collection:{sha256(source:id を収録順に連結)[0..32]}` — 同じ組み合わせ・同じ順序なら常に同じ |
| `dc:creator` | 作者が 1 人ならその人。複数なら先頭作者 + `dc:contributor` に残りを最大 20 名 |
| `dc:language` | 収録作品の最頻値。同数なら先頭作品 |
| `dc:date` | 収録作品のうち最も古い公開日 |
| `dcterms:modified` | ビルド時刻（UTC・秒精度、1 つだけ） |
| `dc:source` | 出さない（作品ごとに URL が異なるため、扉と巻末に置く） |
| `belongs-to-collection` | 全作品が同一シリーズのときだけ、`collection-type="series"` |
| `dc:subject` | 収録作品のタグの上位 20 件（出現数順） |
| `schema:accessibilitySummary` | 収録作品のいずれかに画像があれば visual を含める（既存ロジックの集約） |

ファイル名は既存の `sanitize_epub_filename` を通し、`＜タイトル＞ [piep-N冊].epub` のように**冊であることが分かる接尾辞**を付ける。分冊時は `_1` `_2`。

---

## 7. 上限と分冊

| 制約 | 値 | 出所 |
|---|---|---|
| 画像の合計 | 128 MiB | `MAX_TOTAL_PACKAGED_IMAGE_BYTES` |
| 画像の枚数 | 10,000 | `MAX_PACKAGED_IMAGE_COUNT` |
| 1 画像 | 64 MiB / 40 MP | `MAX_SOURCE_IMAGE_BYTES` / `MAX_IMAGE_PIXELS` |
| Send to Kindle | 1 ファイル 200 MB | Amazon |
| Kindle 品質ガイド | HTML 300 ファイル未満、1 HTML 30 MB 未満 | KDP |

**統合すると、これらは「まれに当たる上限」から「ふつうに当たる上限」に変わる。** 100 件の FANBOX 記事を束ねれば画像 128 MiB は容易に超える。したがって:

- 書き出し前に見積りを出し、超える見込みなら**分冊を提案する**（黙って失敗させない）。
- 分冊は「作品の途中では割らない」。作品単位でしきい値を跨いだところで切る。
- 分冊した各冊は `belongs-to-collection` で同じ合本名を共有し、`group-position` に巻数を入れる。

---

## 8. 検証とテスト

| 段階 | 内容 |
|---|---|
| 単体（Rust） | `EpubCollection` の ID 決定論性、名前空間の衝突（同じ作品を 2 回入れる）、表紙が 1 つだけになること、目次の `playOrder` が単調増加であること |
| 通し（Rust） | `builder.rs` の既存テストを「3 作品の合本」に拡張し、内部検証（`validate_epub`）を通ること。うち 1 作品は画像なし、1 作品は敵対的メタデータ |
| 実データ | pixiv のシリーズ 10 話、FANBOX の記事 30 本、両者混在の 5 件で EPUBCheck 5.3.0 が error 0 |
| 端末 | Kindle Previewer（目次の階層と表紙）、Thorium（`nav` の 2 階層）、Apple Books（`belongs-to-collection` の見え方） |
| フロント | キューの並べ替え（キーボード操作を含む）、見積り表示、統合と個別の切り替えが結果表示に正しく反映されること |

---

## 9. 段階リリース

| 版 | 内容 | 概算 |
|---|---|---|
| **v1** | `EpubCollection` 導入、M1+M2（連結と 2 階層目次）、M5/M6 の設定、`export_epub_merged`、キューの並べ替え UI、タイトル入力 | 3〜4 日 |
| **v2** | M3（合本の情報ページと出典一覧）、M4（シリーズ／作者でのまとめ方）、生成表紙 | 2〜3 日 |
| **v3** | M7（分冊）、見積りと上限警告、Kindle 向けプリセット | 2 日 |

v1 の時点で要求（「選んだ小説を選んだ順番で 1 つのファイルに」）は満たされる。v2 以降は合本としての読み心地と、大量統合の現実対応にあたる。
