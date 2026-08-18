# piep 包括バグ・仕様・コードベース健全性 調査レポート

調査日: 2026-08-18
対象: `develop` ブランチ + 未コミットの作業ツリー（作品コレクション機能一式）
状態: **調査のみ。コードは一切変更していない。**

前提資料: [作品コレクションを中核にした読書・整理・EPUB統合 — 設計提案レポート](work-collections-reader-epub-design-proposal-2026-08-17.md)（以下「設計提案」）

---

## 0. 結論

自動検証は**ほぼ緑だが、CI は現状の作業ツリーで確実に落ちる**。

| 検証 | 結果 |
|---|---|
| `tsc --noEmit` | ✅ 0 件 |
| `vitest run src` | ✅ 46 ファイル / 256 テスト passed |
| `cargo test` | ⚠️ 245 passed（ただし 1 件が並列実行時のみ間欠失敗） |
| `cargo clippy --all-targets -- -D warnings` | ❌ **error 2 件 — CI の quality ジョブが落ちる** |
| `cargo fmt --check` | ❌ 差分あり（新規コード含む） |
| ESLint | ❌ **設定自体が存在しない** |

過去の監査（[2026-08-09](comprehensive-refactor-audit-2026-08-09.md) / [2026-08-12](comprehensive-refactoring-audit-2026-08-12.md)）で挙がった P0/P1 は概ね「完了」になっており、再掲していない。本レポートは **それ以降に入った未コミットのコレクション機能** と、**過去監査が扱っていない領域**に絞る。

発見は重大度順に P0〜P3 で示す。

---

## 1. P0 — 今すぐ直すべきもの

### 1-1. CI が確実に落ちる（clippy）

`.github/workflows/quality.yml` は `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` を実行する。現在の作業ツリーはこれで **error 2 件**になる。

```
error: use of `extend` instead of `append` for adding the full range of a second vector
  --> src\commands\epub.rs:263:9
  help: try: `illustrations.append(&mut manifest.content.illustrations)`

error: use of `extend` instead of `append` for adding the full range of a second vector
  --> src\commands\epub.rs:264:9
  help: try: `attachments.append(&mut manifest.content.attachments)`
```

該当は今回追加された [`merge_collection_manifests`](../src-tauri/src/commands/epub.rs:263) の 2 行。`clippy --fix` かサジェスト通りの手修正で解消する。**この 1 件だけは push 前に必須。**

### 1-2. 却下したコレクション候補を元に戻す手段がない

設計提案 §15 は「却下した候補が繰り返される」対策として *作品ペアと規則版を含む* 否定フィードバックを求めている。実装は `collection_pair_feedback` に `rule_version` を**保存はするが、判定時に一切参照しない**。

[`suggestion_pair_is_rejected`](../src-tauri/src/database/queries.rs:9188) は `decision = 'reject'` を見るだけで `rule_version` を条件に入れていない。結果として:

- 一度却下したペアは**規則を改善しても永久に再提案されない**（設計提案 §5「規則を更新しても…却下した提案も再表示しない」の意図を超えて、更新しても復活しない）。
- 却下を解除する API・UI が存在しない。`accept` で上書きは可能だが、**そのペアは提案に出てこないので accept する経路自体が作れない**。データモデル上の袋小路になっている。
- [`reject_collection_suggestion`](../src-tauri/src/database/queries.rs:2380) は **(全 seed × 全 member) の総当たりペア**を reject 登録する。30 件の候補のうち 3 件だけが誤りでも、ボタン一つで 30 件すべてのペアが恒久ブロックされる。
- しかも UI 側 [`CollectionPage.tsx:125`](../src/features/collections/CollectionPage.tsx:125) の「今後この組合せを提案しない」に**確認ダイアログがない**。削除操作には `modals.openConfirmModal` があるのに、より不可逆なこちらには無い。

既存テスト [`collection_suggestions_use_title_parts_and_learn_rejection`](../src-tauri/src/database/queries.rs:14044) はこの「永久ブロック」を正常系として固定してしまっている。

**推奨**: ①判定時に `rule_version` を突き合わせる、②却下は「その提案に実際に含まれ、かつ利用者がチェックを外したペア」に限定する、③設定画面に却下履歴のクリアを置く、④却下ボタンに確認を入れる。

---

## 2. P1 — 設計提案の受け入れ条件に反しているもの

### 2-1. UI 名称が設計決定と正反対（「グループ」対「コレクション」）

設計提案は 2 箇所で明示的に決定している。

- §2-3: 「内部名称は `work_collection`、**利用者向け表示は「コレクション」を推奨する**」。「グループ」を退けた理由まで書いてある — *作者サークル、共同制作グループ、権限グループとも読める*。
- §20 の決定表: 「UI 名 | **コレクション** | 人物グループとの混同を避ける」。

実装の実測値:

```
src/ 配下の「グループ」    : 27 箇所
src/ 配下の「コレクション」:  0 箇所
```

内部側はすべて `collection` で統一されている（ルート `/collections`、型 `WorkCollection`、`collectionApi`、テーブル `work_collections`）のに、**利用者に見える文字列だけが全部「グループ」**。ドキュメントと画面が食い違い、かつ設計提案が避けようとした語をそのまま採用している。

さらに [`WorkspaceNav.tsx`](../src/app/WorkspaceNav.tsx) では語の衝突が実際に起きている。同ファイルの `type GroupId` / `NavGroup` は元々「サイドバーの折りたたみ区画」の意味で、コメントも `/** Which groups are unfolded. */`。そこへ作品コレクションを指す `groups: true` が追加され、1 ファイル内で "group" が 2 つの異なる概念を指すようになった。設計提案が警告した曖昧さがそのまま発生している。

**推奨**: 表示文字列を「コレクション」に統一する。実施しないなら、設計提案 §2-3/§20 を「グループ」に改訂して決定を一本化する。どちらでもよいが、**両方が併存している現状が最も悪い**。

### 2-2. 順序なしコレクションを「前の作品 / 次の作品」として見せている

設計提案 §10-4 は「**「次の作品」とは呼ばず**」と指定し、§16 Phase 2 の受け入れ条件にも「**順序なしコレクションを「続編」と表示しない**」と明記されている。

[`ReaderPage.tsx`](../src/features/reader/ReaderPage.tsx) の `addCollection` は `collection.collectionKind` を**一度も参照していない**。`unordered` のコレクションでも `ordered` と全く同じ「前の作品」「次の作品」ボタンが出る。型 `WorkCollection` には `collectionKind` が来ているので、分岐は追加するだけで足りる。

### 2-3. EPUB 書き出しに「除外して続行」がない

設計提案 §13-3: 「EPUB 書き出し時は欠落作品を明示し、**「除外して続行」または「中止」を選べる**。」

[`export_collection_epub`](../src-tauri/src/commands/epub.rs) は欠落があると無条件でエラーを返す。

```rust
if !missing.is_empty() {
    return Err(format!("未保存の作品が{}件あるため、内容を欠かさず一冊にできません: {}", ...));
}
```

選択肢は「中止」しかない。作品を 1 件消しただけでそのコレクションは**永久に EPUB 化できなくなる**。設計提案が「欠落参照を残す」を既定にした（§20）のは再保存と再現性のためで、書き出しを塞ぐためではない。

加えて操作順が逆。[`CollectionPage.tsx:235`](../src/features/collections/CollectionPage.tsx:235) は `openSingleDialog` で出力先フォルダーを選ばせた**後**に上記エラーが出る。欠落チェックはダイアログより前に出すべき。

### 2-4. 話数の表記ゆれ吸収が、設計提案の例をほとんど通さない

設計提案 §6-3 は `第12話 / 12話 / #12 / その12 / 十二 / XII` を挙げ、§3 の例 C では `航路 01` / `航路 #2` / `航路 第三夜` を代表例にしている。

[`title_stem_and_order`](../src-tauri/src/database/queries.rs:9237) の正規表現は接頭辞が**必須**。

```
(?:第\s*|[#＃]\s*)(\d+)\s*(?:話|章|回|編)?
```

実測（同一パターンで検証）:

| 入力 | 結果 |
|---|---|
| `第12話` | ✅ 12 |
| `#12` | ✅ 12 |
| `12話` | ❌ 非該当 |
| `その12` | ❌ 非該当 |
| `十二` | ❌ 非該当 |
| `XII` | ❌ 非該当 |
| `航路 01` | ❌ 非該当 |
| `航路 #2` | ✅ 2 |
| `航路 第三夜` | ❌ 非該当（`第` の後が漢数字） |

**設計提案自身の例 C は 3 件中 1 件しか認識できない。** 前後編マーカー側も `上巻/中巻/下巻` はあるが**素の `上` / `中` / `下` がない**（§6-3 は素の形を挙げている）。`最終話` `番外編` `プロローグ` `エピローグ` の特別位置（§6-3）は未実装。

副作用として、マーカー正規表現の `final` は大小無視で**単語境界を見ない**ため、`Final Fantasy` や `finalize` を含む題名が「完結編」と誤判定され語幹からも削られる。

既存テストは `前編` / `後編` のみを覆っており、この差分は検出できていない。

### 2-5. 候補が 1 件も見つからなかった提案が「確度 100%」と表示される

[`generate_collection_suggestion`](../src-tauri/src/database/queries.rs:2244):

```rust
let overall_score = ranked.iter()
    .filter(|value| !seed_id_set.contains(&value.work.id))
    .map(|value| value.member_score)
    .reduce(f64::max)
    .unwrap_or(1.0);   // ← seed 以外が 0 件のとき 1.0
```

seed は必ず `ranked` に残る（`is_seed` はスコア閾値 0.44 を通過しない）ため `ranked.is_empty()` のエラーには落ちず、**関連作品ゼロの提案が生成され、UI は `確度 100%` のバッジを出す**（[`CollectionPage.tsx:106`](../src/features/collections/CollectionPage.tsx:106)）。意味が反転している。関連が無いなら 0.0 にするか、seed 単独の提案は作らず「候補が見つかりません」を返すのが妥当。

---

## 3. P2 — 実害のある実装上の欠陥

### 3-1. 候補の並びにスコアが使われていない

[`compare_ranked_suggestion_members`](../src-tauri/src/database/queries.rs:9293) の比較キーは `series_order → link_order → episode_order → published_at → id` で、**`member_score` が入っていない**。

設計提案 §7-1 の順序優先度としては妥当だが、結果として `member_score` は「0.44 の足切り」と `overall_score` にしか使われない。話数が振られているだけの低スコア候補が、強い根拠を持つ高スコア候補より上に来る。§6-5 の提示レベル（85 以上 / 60〜84 / 40〜59）も UI に反映されていない — すべて一律 `selected: true` で提示される（§6-5 は 60〜84 を「個別確認を目立たせる」としている）。

### 3-2. 上限超過時の seed 復帰で根拠が失われ、上限も超える

```rust
ranked.truncate(limit);
for seed_id in retained_seed_ids {
    ranked.push(RankedSuggestionMember::seed(seed.clone()));
}
```

- `truncate(limit)` の後に push するので、最終的な件数は **`limit` を超える**。
- [`RankedSuggestionMember::seed`](../src-tauri/src/database/queries.rs:9069) は `series_order: None` の固定値で作り直すため、**元々計算済みだった公式シリーズ順・リンク根拠が消える**。並べ直した結果、その seed は末尾側へ飛ぶ。

3-1 の通りスコアが並び替えに効かないので、seed が `limit` の外へ出るのは実際に起こりうる経路である。

### 3-3. リーダーを開くたびに N+1 クエリが走る

[`ReaderPage.tsx`](../src/features/reader/ReaderPage.tsx) の `memberCollectionsQuery`:

```ts
const summaries = await listCollectionsForWork(...);
return Promise.all(summaries.slice(0, 20).map((s) => getWorkCollection(s.id)));
```

所属コレクション数ぶん `db_get_work_collection` を発行する（最大 20 回、各回が全メンバー＋downloads の LEFT JOIN）。作品を開くたびに毎回走る。`staleTime: 30_000` はあるが作品ごとにキーが変わる。1 コマンドでまとめて返す形が望ましい。

### 3-4. 候補生成が非常に重くなりうる

[`generate_collection_suggestion`](../src-tauri/src/database/queries.rs:2049) は Tantivy の逆引きヒットに対して 1 件ずつ `refresh_work_links` を呼ぶ。

```rust
for seed in &seed_works {                    // 最大 20
    ... search_with_total(..., 100)          // 最大 100 ヒット
    for hit in result.hits {
        if self.refresh_work_links(hit.download_id).is_ok() { ... }
```

最悪 **2,000 回**の `refresh_work_links`。各回が `get_reader_document`（ファイル I/O + 本文パース）＋ 書き込みトランザクションを行う。しかも全体が `run_library_write_blocking` の下でライブラリ書き込みロックを保持したまま走るため、その間ほかの保存・更新が待たされる。中止・進捗の手段もない（設計提案 §14 は「全再構築は…進捗・中止・再開を持たせる」としている）。

候補生成の SQL 自体も `FROM downloads candidate JOIN downloads seed` の直積で、1 万件規模では seed 数 × 全作品の走査になる。

### 3-5. EPUB 画像のパッケージ名変更が全書き出しに波及する

[`builder.rs`](../src-tauri/src/epub/builder.rs) の変更:

```rust
let base = if is_cover { "cover" } else { image.id.as_str() };
```

コメントは「合本では同じ stem が衝突しうるため」と合本を理由に挙げているが、この行は**単体作品の書き出しを含む全経路**に効く。EPUB 内の `OEBPS/images/*` のファイル名が、元ファイル名由来からマニフェスト ID 由来へ変わる。

`unique_name` がサニタイズし、`keys` に元 stem も残るため参照解決は壊れない（そこは正しい）。ただし**意図した範囲より広く挙動が変わっている**点は認識しておくべき。既存 EPUB との差分比較・回帰スナップショットがあれば影響を受ける。

### 3-6. 欠落メンバーがリーダーの並びから黙って消える

`adjacentWorks` に渡す配列は `member.downloadId ? [...] : []` で作られる。未保存メンバーは**通知なく飛ばされる**ため:

- 「次の作品」が実際には 1 つ飛ばしになる。
- 表示される `${items.length}作品の並び` が、コレクション画面の `memberCount` と食い違う。

設計提案 §13-3 は欠落を明示する方針。少なくとも 2 画面で件数が一致すべき。

### 3-7. 読了パネルに進捗表示がない

設計提案 §10-2 は「進捗: `3 / 8`」と次作品の表紙・作者・文字数・取得元の表示を求めている。実装は `${items.length}作品の並び` のみで**現在位置がない**。次作品ボタンも題名だけ。

---

## 4. P3 — 健全性・一貫性

### 4-1. `cargo test` が並列実行時のみ間欠失敗する

```
thread '...sorted_search_orders_matches_by_column_and_pages_without_gaps' panicked:
  "Tantivy commit failed: An IO error occurred: 'アクセスが拒否されました。 (os error 5)'"
```

- 単体実行（`cargo test --lib sorted_search_orders_matches`）: **成功**
- フルスイート 1 回目: **失敗**
- フルスイート 2 回目: **成功**

再現性のない Windows の ACCESS_DENIED。CI は `cargo test` をゲートにしているので、**無関係な PR がランダムに赤くなる**。加えてテスト用の一時ディレクトリ [`temp_paths`](../src-tauri/src/database/queries.rs:13669) は `std::env::temp_dir()` 直下に作られ、失敗経路では `remove_dir_all` に到達せず**残留し続ける**（各テストの末尾で手動削除する方式で、RAII ガードではない）。

### 4-2. `cargo fmt` が CI に無く、実際にドリフトしている

`cargo fmt --check` は今回の新規コード（`commands/database.rs` の import と新コマンド、`archive.rs` のテスト）に加えて、既存の `downloader.rs` にも差分を出す。`rustfmt.toml` も無い。CI に `cargo fmt --check` が無いため、恒常的にずれていく。

### 4-3. フロントエンドに linter が存在しない

`.eslintrc*` / `eslint.config.*` のいずれも無く、`package.json` に lint スクリプトも無い。`npm run check` は `tsc --noEmit` のみ。型エラーは捕まるが、未使用変数、`useEffect` の依存漏れ、`react-hooks` 規則違反は誰も見ていない。今回のようにフックを多用する新機能が入る局面では効き目が大きい。

### 4-4. `queries.rs` が 17,250 行

```
17250  src-tauri/src/database/queries.rs
 5249  src-tauri/src/commands/archive.rs
 2678  src-tauri/src/commands/downloader.rs
```

`queries.rs` 単体でバックエンド全体（51,048 行）の 34%。今回のコレクション機能も約 1,100 行がここに積まれ、`Database` の inherent impl と自由関数・テストが同居している。`database/collections.rs` のような分割が自然な切り口。過去監査は性能を扱ったがこの構造的肥大には触れていない。

### 4-5. コレクション要約の SQL が 3 箇所に重複

12 カラムの `SELECT c.id, c.name, ... COUNT(...), SUM(...)` が [`list_work_collections`](../src-tauri/src/database/queries.rs:1364) / [`get_work_collection`](../src-tauri/src/database/queries.rs:1390) / [`list_collections_for_work`](../src-tauri/src/database/queries.rs:1843) に丸ごと 3 回書かれている。カラムを 1 つ足すと 3 箇所の同期が必要で、`work_collection_summary_from_row` のインデックスとも暗黙に結合している。

ついでに **ORDER BY のタイブレークが不揃い**:

- `list_work_collections`: `updated_at DESC, name COLLATE NOCASE ASC, c.id ASC`
- `list_collections_for_work`: `updated_at DESC, name COLLATE NOCASE ASC` ← **`c.id` が無い**

同名・同時刻のコレクションで順序が不定になる。過去監査「主要 sort 索引へ `id` tie-break 列を追加」の方針が新規クエリに引き継がれていない。

### 4-6. `CollectionPage.tsx` にコンポーネントテストが無い

`src/features/collections/` には `CollectionPage.tsx` (21.6 KB) しか無い。他 feature は 14 個の `*.test.tsx` を持ち、`WorkPage` `ReaderPage` `LibraryPage` `EditorPage` などは軒並みテスト済み。新規の 275 行・モーダル 2 つ・ミューテーション 5 つを持つ画面だけが素通しになっている。`collectionApi.test.ts` は invoke の引数形だけを見る 49 行で、画面ロジック（選択状態、並べ替え、欠落表示）は覆っていない。

設計提案 §17-3 の回帰テスト項目のうち、**「Reader の context と前後作品が一致する」「EPUB の spine、目次、内部リンクが一致する」「並べ替えが安定し、同順位を作らない」**に対応するテストが見当たらない。

### 4-7. 細かい不整合

| 箇所 | 内容 |
|---|---|
| [`queries.rs:2016`](../src-tauri/src/database/queries.rs:2016) | 候補 SQL は `LENGTH(TRIM(author_name)) >= 2` で 1 文字作者名を除外するが、Rust 側 `same_author` 判定（[:2138](../src-tauri/src/database/queries.rs:2138)）には同じガードが無い |
| [`queries.rs:2143`](../src-tauri/src/database/queries.rs:2143) | `eq_ignore_ascii_case` は日本語作者名では実質完全一致。SQL 側の `COLLATE NOCASE` と意味がずれる |
| [`queries.rs:2427`](../src-tauri/src/database/queries.rs:2427) | `selected_keys` は `trim()` した値で集合を作るのに、照合側 `member.source` は未 trim。理論上マッチしない組み合わせが生じる |
| [`queries.rs:2417`](../src-tauri/src/database/queries.rs:2417) | `accept_collection_suggestion` は ID 1 件を得るために `list_collection_suggestions` で**全 pending を読み `members_json` を全件デシリアライズ**している |
| [`CollectionPage.tsx:53`](../src/features/collections/CollectionPage.tsx:53) | `EMPTY_COLLECTIONS` の型が `WorkCollection[]`。実際に返るのは `WorkCollectionSummary[]` |
| [`WorkPage.tsx`](../src/features/library/WorkPage.tsx) / [`CollectionPage.tsx`](../src/features/collections/CollectionPage.tsx) | 同じ `queryKey: ["work-collections"]` に対して queryFn と `enabled` が別々に定義されている |
| [`archive.rs`](../src-tauri/src/commands/archive.rs) | バックアップの `cover_work` は**メンバーの中からしか**解決しない。`cover_download_id` は非メンバーも指せる（`upsert_work_collection` は存在確認のみ）ため、その表紙は復元で失われる |
| [`archive.rs`](../src-tauri/src/commands/archive.rs) | `collections` 追加後も `version: "3.0"` のまま。旧版アプリが新バックアップを読むとコレクションが黙って落ちる |
| [`epub.rs`](../src-tauri/src/commands/epub.rs) | 章 ID の名前空間化を `page.html_content` への `String::replace` で章数ぶん繰り返す（本文長 × 章数）。引用符で囲っているので誤置換は無いが計算量が二乗的 |
| [`epub.rs`](../src-tauri/src/commands/epub.rs) | `merge_collection_manifests` 冒頭の `works[0].1.clone()` は `author` と `language` の 2 値のためだけに**マニフェスト全体（全ページ HTML 込み）を複製**している |

---

## 5. 誤検知だったもの（確認済みで問題なし）

調査中に疑ったが、追跡した結果**問題なし**と確認できたもの。再調査の重複を避けるため記録する。

| 疑い | 結論 |
|---|---|
| `WorkCollection` の Rust/TS 形が食い違う（`{summary, members}` 対 フラット） | `#[serde(flatten)]` があり **一致している** |
| `restore_work_collection` が復元の外側トランザクションと二重ロックし自己デッドロック | `begin_atomic_restore` は `BEGIN IMMEDIATE` を `execute_batch` で発行しロックを解放する方式。**デッドロックしない** |
| 合本の `<img src="w0001-img0001">` が拡張子・パスなしで壊れる | `builder.rs` が `keys` に `image.id` を追加して解決する。**壊れない** |
| コレクション削除でメンバー行が孤児になる | `PRAGMA foreign_keys = ON` 済み。`ON DELETE CASCADE` が効く |
| `ReaderPage` の `sortBy: "series_order"` / `personSource` 等が未対応パラメータ | `SearchV2Params` と `effective_sort_by` の双方に**存在する** |
| 章 ID 置換で `chapter-1` が `chapter-10` を巻き込む | `id="..."` と引用符ごと置換しており**誤爆しない** |

---

## 6. 推奨対応順序

**push 前（必須）**

1. clippy の 2 件を修正（§1-1）。

**コレクション機能を出す前**

2. 却下の恒久ブロックを解消し、確認ダイアログを追加（§1-2）。
3. UI 名称を「コレクション」へ統一、または設計提案を改訂して一本化（§2-1）。
4. 順序なしコレクションで「次の作品」を出さない（§2-2）。
5. EPUB 書き出しに「欠落を除外して続行」を追加し、欠落チェックをフォルダー選択より前へ（§2-3）。
6. 関連ゼロの提案のスコアを是正（§2-5）。
7. `CollectionPage` と読了パネルのテストを追加（§4-6）。設計提案 §17-3 の項目を骨子にできる。

**続けて**

8. 話数正規化を設計提案 §6-3 の一覧まで広げ、`final` に単語境界を付ける（§2-4）。
9. 候補生成の `refresh_work_links` 呼び出し回数に上限を設け、書き込みロック保持時間を短縮（§3-4）。
10. リーダーの N+1 を 1 コマンドに集約（§3-3）。

**基盤（機能とは独立に効く）**

11. CI へ `cargo fmt --check` を追加し、一度全体を整形（§4-2）。
12. ESLint（`typescript-eslint` + `react-hooks`）を導入（§4-3）。
13. 間欠失敗する Tantivy テストを隔離するか、コミットをリトライ可能にする（§4-1）。
14. `queries.rs` からコレクション関連を `database/collections.rs` へ分離（§4-4）。要約 SQL の 3 重複も同時に解消（§4-5）。

---

## 参考

- [作品コレクション設計提案 2026-08-17](work-collections-reader-epub-design-proposal-2026-08-17.md)
- [複数作品を 1 冊の EPUB にまとめる提案 2026-08-16](epub-merge-export-proposal-2026-08-16.md)
- [包括リファクタリング監査 2026-08-12](comprehensive-refactoring-audit-2026-08-12.md)
- [包括リファクタ監査 2026-08-09](comprehensive-refactor-audit-2026-08-09.md)
