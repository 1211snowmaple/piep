# EPUB の既定デザイン改善 — 提案レポート

実施日: 2026-08-16
更新: 2026-08-17 — **推奨した組み合わせを実装した。縦書き（D-5 / P-6）は不採用。**
対象: `src-tauri/src/epub/templates/{default,pixiv,fanbox}/`、`src-tauri/src/epub/template.rs`（レンダリング文脈）

## 実装状況（2026-08-17）

| 案 | 状態 | 備考 |
|---|---|---|
| 基盤A 色トークン | **実装** | `prefers-color-scheme` でダーク端末に追随。未使用だった `--text-color` は削除し、`--tag-color` は `--accent-color` に改名 |
| 基盤B 余白の一元化 | **実装** | `--space` を基準に見出しと紹介文の余白を作る |
| 基盤C 情報ページの二部構成 | **実装** | 見出し部＋明細部（float 2カラム、狭い画面では自動で縦積み） |
| D-1 Quiet Paper | **実装** | 基盤A・B・C の適用形 |
| D-6 図版の作法 | **実装（受け皿のみ）** | `.illustration` の余白と改ページ抑止、`.caption` の指定。ただし **converter は現在キャプションを出力していない**ため、絵の説明が実際に出るのは変換側の対応後 |
| D-7 読める目次 | **実装（CSS のみ）** | 行の高さと区切り。数値の右揃えは入れていない |
| P-1 シリーズ帯 | **実装** | 話数は帯の中に置く（右端へ飛ばさない） |
| P-2 統計を1行に | **実装** | 3カラム float を廃止。表紙が本来の大きさに戻る |
| P-3 キャプションの器 / P-4 タグの整理 | **実装** | |
| F-1 投稿ヘッダ / F-2 抜粋リード / F-4 添付の明示 / F-5 プランのバッジ | **実装** | 添付の注記は `strings` ではなくテンプレート本文に置いた（ファイル編集で変更できるため） |
| D-2 扉ページ / D-3 奥付 / D-4 日本語組版 / F-3 画像主体 | 見送り | 価値は残るが、builder・converter に手が入るため別途 |
| **D-5・P-6 縦書き** | **不採用** | 端末差が大きく、再現性を保証できない |

実装の検証は `epub::builder::tests::every_builtin_template_renders_its_own_front_matter`（3テンプレートを実際に組んで内部検証を通す）と、
`cargo run --example epub_specimen -- <出力先>`（情報ページを1枚の HTML に書き出してブラウザで見る）で行った。

関連: [EPUB 3.3・Send to Kindle 互換性レポート](epub-3-3-kindle-compatibility-report-2026-08-12.md)、[統合書き出しの提案](epub-merge-export-proposal-2026-08-16.md)、[表紙プレースホルダーの提案](cover-placeholder-design-proposals-2026-08-16.md)

---

## 1. 現状の棚卸し

### 1-1. 生成される本の構造

`EpubBuilder::build` が組む中身は、どのテンプレートでも同じ骨格になる。

| 順 | ファイル | 出所 | 備考 |
|---|---|---|---|
| — | `OEBPS/style/style.css` | `style.css.j2`（`_base_style.css.j2` を include） | **全ページが1枚を共有** |
| 1 | `text/cover.xhtml` | `cover_page.xhtml.j2` | 表紙画像がある場合のみ |
| 2 | `text/info.xhtml` | `info_page.xhtml.j2` | `infoFields` の順に項目を並べる |
| 3 | `nav.xhtml` | `nav.xhtml.j2` | Kindle 対策で spine 内に置く |
| 4〜 | `text/page_00N.xhtml` | `page_wrapper.xhtml.j2` | 本文 |

CSS は1枚しかない。つまり**表紙・情報ページ・本文の体裁差は、すべて `body` のクラス（`cover-page` / `info-page`）と要素クラスで書き分ける**必要がある。

### 1-2. 既定テンプレートの CSS（実測）

`_base_style.css.j2` は 179 行、変数は 6 個だけ。

```css
:root {
    --text-color: #202020;   /* 実際にはどこからも参照されていない */
    --muted-color: #767676;
    --tag-color: #0073bb;
    --rule-color: #dcdcdc;
    --badge-bg: rgba(0, 0, 0, 0.06);
    --cover-radius: 10px;
    --cover-width: 60%;
}
```

- `--text-color` は宣言されているが `body { color: … }` が無く、**未使用**。地の文の色は端末任せ（これは正しい判断）。
- 一方で `--muted-color: #767676` と `--tag-color: #0073bb` は**固定値のまま出力される**。端末をダークにすると、白地前提のこの2色だけが背景に沈む。
- `--badge-bg: rgba(0,0,0,0.06)` も同様で、黒背景では実質見えない。

### 1-3. 3テンプレートの差はどれくらいか

| テンプレート | 固有 CSS | 固有 XHTML | 構成の差（template.json） |
|---|---|---|---|
| default | 2行（コメントのみ） | なし | 既定 |
| pixiv | 37行 | `info_page` のみ | 「キャプション」「ブックマーク」等のラベル差 |
| fanbox | 21行 | `info_page` のみ | 更新日・支援プラン・添付を既定でオン |

**実質、3つは同じ本**である。pixiv だけが表紙と文字数を float で横並びにし、FANBOX だけが抜粋に薄い背景を敷く。それ以外は同じ組版で、取得元ごとの読み味の違い（pixiv＝連載小説、FANBOX＝日付のある記事）は体裁に出ていない。

### 1-4. 情報ページの実際の姿

`info_page.xhtml.j2` が吐くのは、ほぼ `<p>` の羅列である。

```html
<p class="series">…</p>
<h1 class="title">…</h1>
<p class="author">作者：…</p>
<div class="cover-container"><img … /></div>
<p class="text-length"><span class="badge">12,345文字</span></p>
<div class="description">…</div>
<p class="tags"><span class="tag">#タグ</span> …</p>
<p class="date">公開日：…</p>
<p class="source-link"><a …>https://…</a></p>
```

`.date` と `.meta` は同じ指定（`0.9em` / muted）で、**「公開日」も「いいね」も「R-18」も同じ重みで積まれる**。読み手が最初に知りたい「誰の・いつの・どれくらいの長さの話か」と、あとで確認したい「配信元 URL」が同列に並ぶ。

### 1-5. 現状の弱点（このレポートが解こうとするもの）

| # | 事実 | 何が困るか |
|---|---|---|
| W1 | muted / tag / badge の色が白地前提の固定値 | ダーク表示の端末で情報ページが読みにくい |
| W2 | 情報ページに階層が無い | 表題より下がすべて同じ重みで、目が滑る |
| W3 | 本文 `p` が `margin: 0 0 0.2em; text-indent: 0` | 日本語の小説として字下げも段落間も足りない。原文の全角字下げに依存している |
| W4 | 表紙画像が無いと表紙ページごと消える（`has_cover_page = include_cover_page && cover.is_some()`） | FANBOX の記事の多くが「表紙のない本」になる |
| W5 | 挿絵の `caption` を受ける CSS が無い | 中間表現には入っているのに、絵の説明が地の文と同じ見た目になる |
| W6 | `h2` に下線が固定で付く | 本文中の見出しと、情報ページの見出しが同じ装飾になる |
| W7 | 縦書きの選択肢が無い（`pageProgression` は設定にあるが CSS が横書き前提） | 日本語小説として自然な組版が選べない |

---

## 2. どの案も守る制約

[互換性レポート](epub-3-3-kindle-compatibility-report-2026-08-12.md) で確定させた方針を、そのまま設計制約として引き継ぐ。

1. **端末の設定を奪わない。** 本文の font-family、色、背景、行間を固定しない。指定するのは「関係」（余白比・相対サイズ）に留める。
2. **flex / grid を本文レイアウトに使わない。** 古い端末に無い。横並びは `float` で組む（pixiv テンプレートの既存方針）。
3. **CSS は1枚。** ページ種別は `body` のクラスで分岐する。
4. **表紙に SVG を使わない。** ビルダーは `cover_uses_svg` を見て `properties="svg"` を立てるが、Kindle 変換では単純な `img` が最も安全。
5. **`infoFields` の並べ替え・オンオフを壊さない。** 情報ページの改修は「for ループの中身の書き換え」に留め、項目の増減はテンプレートスタジオ側の責務のままにする。
6. **EPUBCheck 5.3.0 で error 0 を維持する。**
7. **画像を増やさない。** 装飾のための画像素材を持ち込まない（CSS と文字だけで組む）。

---

## 3. 共通の土台（3テンプレートすべてに効く改修）

以下の案は、どの意匠を選んでも先に入れる価値がある。

### 基盤A. 色トークンを「意味」で切り、ダーク端末に追随させる

```css
:root {
    /* 地の文の色は指定しない。以下は「地の文からの距離」だけを決める */
    --muted-color: #6b6b6b;
    --rule-color: #d8d8d8;
    --accent-color: #0073bb;   /* テンプレートごとに上書きする1色 */
    --badge-bg: rgba(0, 0, 0, 0.06);
    --badge-color: inherit;
}

@media (prefers-color-scheme: dark) {
    :root {
        --muted-color: #a8a8a8;
        --rule-color: #4a4a4a;
        --accent-color: #6fc0ff;
        --badge-bg: rgba(255, 255, 255, 0.12);
    }
}
```

`prefers-color-scheme` は Kindle では効かないが、Apple Books・Kobo・Thorium などでは効く。**効かない端末では現状と同じ**なので、退化しない。あわせて、未使用の `--text-color` は削除し、`.badge` は `background` ではなく `border: 1px solid var(--rule-color)` を主とする（背景反転に強い）。

### 基盤B. 余白を1つの尺度から作る

```css
:root { --space: 1em; }
h2 { margin: calc(var(--space) * 2) 0 calc(var(--space) * 0.8); }
.description { margin: calc(var(--space) * 1.2) 0; }
```

現状は `2em` `1.2em` `0.6em` `0.2em` が散っている。1変数に集約すると、テンプレートごとに「詰めた版／ゆったり版」を1行で作れる。

### 基盤C. 情報ページを「見出し部」と「明細部」に割る

`info_page.xhtml.j2` の for ループを 2 周に分け、`title` / `author` / `series` / `cover` / `description` を**扉**として組み、残りの数値・日付・URL を**明細**として下にまとめる（pixiv テンプレートが既に採っている「先に置く／後で流す」の手法をそのまま一般化する）。

```jinja
{%- set masthead = ["series", "title", "author", "cover", "textLength", "description"] %}
{%- for field in fields %}{% if field.key in masthead %}…{% endif %}{% endfor %}
<div class="details">
{%- for field in fields %}{% if field.key not in masthead %}…{% endif %}{% endfor %}
</div>
```

明細部は次の 2 カラム float で組むと、ラベルと値が縦に揃う。

```css
.details { overflow: hidden; font-size: 0.9em; }
.details .row { clear: both; margin: 0.25em 0; }
.details .label { float: left; width: 6em; color: var(--muted-color); }
.details .value { margin-left: 6.5em; }
```

（`table` を使わないのは、読み上げが「表」として扱うのと、Kindle 変換で崩れやすいため。）

---

## 4. 既定テンプレート（default）の意匠 — 7案

「どの取得元でも破綻しない、素直な組版」という現在の性格は保ったまま、W1〜W7 を別々の角度から解く。**D-1 と D-2 は他案の下敷きにもなる。**

### D-1. Quiet Paper（静かな紙）— 現状の洗練

- **ねらい**: 意匠を足さず、W1・W2・W6 だけを解く。既存ユーザーの本の見た目を大きく変えない。
- **見え方**: 表題は今より一段大きく、作者・シリーズは表題に寄せて1ブロック。数値と日付は下部の明細ブロックに落ちる。`h2` の下線は情報ページだけに残し、本文の見出しは太さと余白で差を付ける。
- **実装**: 基盤A + 基盤B + 基盤C。`_base_style.css.j2` の差し替えのみで、XHTML は基盤Cのループ2周化だけ。
- **リスク**: 低。既存の自作テンプレートは `_base_style.css.j2` を include しているため、**利用者が編集済みのテンプレートには反映されない**（`reset_template_file` で戻せる旨を UI で案内する必要あり）。
- **手間**: 小（CSS 約80行の書き換え + j2 30行）

### D-2. Title Page（扉ページ）— 情報ページを本の扉にする

- **ねらい**: W2 の抜本対応。情報ページを「メタデータの一覧」から「本の扉」に格上げする。
- **見え方**: 1ページ目は上から 30% の位置にシリーズ名（小）、表題（大）、作者名。表紙画像はその下に `--cover-width: 55%` で中央。紹介文・タグ・数値・URL は**次のセクション**（`<hr>` ではなく余白で切る）。
- **実装**:

```css
.info-page .masthead { margin: 2.5em 0 2em; text-align: center; }
.info-page .masthead .title { font-size: 1.9em; line-height: 1.3; margin: 0.2em 0; }
.info-page .masthead .author { margin-top: 1.2em; }
.info-page .details { margin-top: 3em; border-top: 1px solid var(--rule-color); padding-top: 1em; }
```

- **リスク**: 中。中央寄せは長い日本語タイトルで折り返しが増える。`text-align: center` は本文には掛けない。
- **手間**: 中

### D-3. Colophon（奥付）— メタデータを巻末に移す

- **ねらい**: 「読み始めるまでの距離」を最短にする。情報ページには表題・作者・表紙・紹介文だけを置き、公開日・URL・タグ・統計は**巻末の奥付ページ**にまとめる。
- **見え方**: 本を開くと扉、次のページから本文。最後に「この本について」（出典 URL、取得日、piep のバージョン）。
- **実装**: `TemplateSettings` に `colophon: bool` を足し、ビルダーが `text/colophon.xhtml` を spine 末尾に足す。テンプレートに `colophon.xhtml.j2` を追加。`landmarks` に `epub:type="colophon"` を追加。
- **リスク**: 中。ビルダーとテンプレートの両方に手が入る（新規ファイル1、Rust 約40行）。
- **手間**: 中〜大
- **補足**: [統合書き出し](epub-merge-export-proposal-2026-08-16.md) で「出典一覧」を作る際、この奥付をそのまま流用できる。

### D-4. Japanese Novel（日本語組版寄せ）— 本文そのものを直す

- **ねらい**: W3 の対応。読み物として最も効く。
- **見え方**: 段落は 1 字下げ、段落間は空けない（日本語の小説の作法）。ただし**原文が既に全角スペースで字下げしている場合に二重にならない**よう、変換側で行頭の全角スペースを落とすか、CSS 側で `text-indent` を選択制にする。
- **実装**:

```css
.chapter p { text-indent: 1em; margin: 0; line-height: inherit; }
.chapter p.blank-line { text-indent: 0; }
/* 会話文・記号始まりは字下げしない */
.chapter p.no-indent { text-indent: 0; }
/* 場面転換 */
.chapter p.scene-break { text-indent: 0; text-align: center; margin: 1.5em 0; }
```

行頭が `「`『`（`—`＊` の段落に `no-indent` を付ける判定は `converter.rs` 側で行う（1行の判定関数）。
- **リスク**: 中。判定を誤ると全段落が不揃いになる。**テンプレート設定 `indentStyle: "none" | "first-line"` を用意して既定を選べるようにする**のが安全。
- **手間**: 中（CSS 小 + Rust の段落分類）

### D-5. Vertical（縦書き）— 別テンプレートとして追加

- **ねらい**: W7。pixiv の小説を「縦で読む」ための選択肢。
- **実装**:

```css
html { writing-mode: vertical-rl; -epub-writing-mode: vertical-rl; }
.chapter p { text-indent: 1em; }
.number { -epub-text-combine: horizontal; text-combine-upright: all; } /* 縦中横 */
```

`template.json` は `pageProgression: "rtl"`。ビルダーは既に `page-progression-direction` を OPF に書いている。
- **リスク**: 高め。Kindle は縦書き EPUB を受け付けるが変換結果の再現性が低く、Apple Books と Kobo で挙動が違う。**既定にはせず「縦書き（実験）」という別テンプレートとして追加**し、検証は実機で行う。
- **手間**: 中（新規テンプレート一式 = 既存のコピー + CSS 20行）

### D-6. Figure（図版の作法）— 挿絵とキャプション

- **ねらい**: W5。画像の多い FANBOX にも効くが、default に置いて全テンプレートの土台にする。
- **見え方**: 画像は中央、キャプションは画像直下に小さく muted、前後の余白は本文より広い。画像のみのページは上下中央に。
- **実装**:

```css
.illustration { margin: 1.5em 0; text-align: center; page-break-inside: avoid; break-inside: avoid; }
.illustration img { max-width: 100%; height: auto; }
.illustration .caption { display: block; margin-top: 0.4em; font-size: 0.85em; color: var(--muted-color); text-align: center; }
.illustration--full { margin: 0; height: 100%; }
```

`EpubImage.caption` は中間表現に既にあるので、`converter.rs` が `<figure>`/`<figcaption>` 相当を吐くかを確認して繋ぐだけで済む。
- **リスク**: 低
- **手間**: 小

### D-7. Reader's TOC（読める目次）— nav を案内板にする

- **ねらい**: `nav.xhtml` は spine に入っていて**実際に読者が開くページ**なのに、素の `ol` である。
- **見え方**: 目次の各行に、章題＋（あれば）ページ番号／文字数を右寄せで添える。階層は字下げではなく太さで示す。
- **実装**: `nav.xhtml.j2` の `li` に `<span class="toc-num">` を足し、`nav[epub|type~='toc'] a { display: block; border-bottom: 1px dotted var(--rule-color); }` 程度。統合書き出しでは、この目次が**本の主要な入口**になるため先行投資になる。
- **リスク**: 低（`nav` の中身は EPUB 3 の規定が厳しいので、`a` の中に入れる要素は `span` に留める）
- **手間**: 小

---

## 5. pixiv テンプレートの意匠 — 6案

pixiv の小説は「シリーズの第 n 話」「文字数」「タグ」「キャプション」が読み手の判断材料になる。取得元の画面がそうなっている以上、本の側でもその 4 つを主役にする。

### P-1. Series Band（シリーズ帯）

- 現状の `.series`（0.8em / 灰色）を、表題の上に**帯**として置く。左に細いアクセント罫、右に「第3話」。
```css
.info-page .series { border-left: 3px solid var(--accent-color); padding-left: 0.6em; color: var(--muted-color); }
.info-page .series .order { float: right; }
```
- シリーズ物を連続で書き出したとき、扉を見ただけで順番が分かる。**統合書き出しの部扉にもそのまま使える。**
- 手間: 小

### P-2. Stat Line（1行の統計）

- 現在の 3 カラム float（`spacer` / `cover` / `count`）をやめ、表紙の直下に「12,345文字 ・ 全3ページ ・ 2018-08-21」を中黒区切りの 1 行で置く。
- 3 カラムは表紙を画面の 1/3 幅に固定するため、**縦長の表紙が極端に小さくなる**。1行化すると表紙は `--cover-width` に従い、統計は読みやすい位置に落ちる。
```css
.stat-line { text-align: center; font-size: 0.85em; color: var(--muted-color); margin: 0.6em 0 1.4em; }
.stat-line span + span::before { content: " ・ "; }
```
- 手間: 小（pixiv の `info_page.xhtml.j2` 内、現在の `cover-meta` ブロックの置き換え）

### P-3. Caption Card（キャプションの器）

- pixiv のキャプションは `<br>` 主体で、URL がそのまま入ることが多い。地の文と同じ体裁だと本文の続きに見える。
```css
.info-page .description { border-left: 2px solid var(--rule-color); padding-left: 0.8em; font-size: 0.95em; }
.info-page .description a { word-break: break-all; color: var(--accent-color); }
```
- 手間: 小

### P-4. Tag Cloud（タグの整理）

- 現状はタグを空白区切りの1段落で流している。R-18 やキャプション由来の長いタグが混ざると行が荒れる。
- 各タグを `border` 付きの小片にし、`line-height: 2` で行間を確保する。年齢制限は独立した注記として先頭に置く（`.meta.adult` の赤は据え置き、ただしダークでも見える明度に）。
```css
.tags .tag { display: inline-block; margin: 0 0.3em 0.3em 0; padding: 0 0.5em;
             border: 1px solid var(--rule-color); border-radius: 1em; font-size: 0.85em; color: var(--accent-color); }
```
- 手間: 小

### P-5. Chapter Break（話の切れ目）

- pixiv の小説は 1 作品が複数ページに分かれる。現状はページごとに XHTML が分かれるだけで、本文側に「区切り」の表現が無い。
- ページ先頭に控えめな柱（`第2ページ` ではなく、章題があれば章題）を置き、無ければ何も置かない。目次の階層（`chapter_toc`）と揃える。
```css
.chapter > h2:first-child { margin-top: 0; border-bottom: none; font-size: 1.1em; color: var(--muted-color); }
```
- 手間: 小

### P-6. Pixiv Vertical（縦書き pixiv）

- D-5 を pixiv 用に仕立てた別テンプレート。`pageProgression: "rtl"`、`--cover-width: 70%`、縦中横を数字とアルファベット2文字に適用。
- pixiv 小説の投稿者が想定している読み方（pixiv 本体のビューアは縦書き切り替えを持つ）に最も近い。
- 手間: 中、リスク: 高（実機確認必須）

---

## 6. FANBOX テンプレートの意匠 — 6案

FANBOX は「日付のある記事」であり、画像・添付・支援プランが本文と同じくらい情報を持つ。

### F-1. Post Header（投稿ヘッダ）

- 表題の下に「クリエイター名 @creatorId ・ 2026-05-01 ・ 500円プラン」を1行で置き、罫線で本文と切る。ブログ記事の作法。
```css
.post-header { border-bottom: 1px solid var(--rule-color); padding-bottom: 0.8em; margin-bottom: 1.4em; }
.post-header .meta { display: inline; font-size: 0.85em; }
.post-header .plan { color: var(--plan-color); }
```
- 手間: 小（fanbox の `info_page.xhtml.j2` の並べ替え）

### F-2. Excerpt as Lead（抜粋をリード文に）

- 現在の `.excerpt` は薄いグレー背景。ダーク端末で沈む（W1）ため、**背景を捨てて左罫＋やや大きめの文字**にする。記事の導入文として読ませる。
```css
.info-page .description.excerpt { background: none; border-left: 3px solid var(--accent-color);
                                   padding: 0.2em 0 0.2em 0.9em; font-size: 1.02em; line-height: 1.7; }
```
- 手間: 小

### F-3. Image-first（画像主体の記事向け）

- FANBOX の `postType: image` は本文がほとんど無く、画像の連続になる。1画像＝1ページに割り、連番とキャプションを添える。
```css
.chapter--gallery .illustration { margin: 0; text-align: center; }
.chapter--gallery .illustration .index { font-size: 0.8em; color: var(--muted-color); }
```
- ビルダー側で `provider.postType == "image"` のとき `body` に `chapter--gallery` を付ける（`page_wrapper.xhtml.j2` は `provider` を参照できる）。
- 手間: 中

### F-4. Attachments Notice（添付の明示）

- EPUB に添付ファイルは収録されない（`EpubAttachment` は存在だけを伝える）。現状は箇条書きの灰色テキストで、**「本の中にあるが開けない」のか「元記事にある」のかが読者に伝わらない**。
```css
.attachments { border: 1px solid var(--rule-color); border-radius: 6px; padding: 0.6em 0.9em; list-style: none; }
.attachments::before { content: "この投稿には添付ファイルがあります（本書には収録されていません）"; display: block;
                       font-size: 0.85em; color: var(--muted-color); margin-bottom: 0.4em; }
```
- 文言は `settings.strings` に `ATTACHMENTS_NOTE` として持たせ、テンプレートスタジオから変えられるようにする。
- 手間: 小

### F-5. Plan Badge（支援プランの表示）

- `feeRequired` は現在「支援プラン：500円」という平文。**バッジ**にして、無料公開との差が一目で分かるようにする。ダークでも読めるよう枠線ベース。
```css
.badge--plan { border: 1px solid var(--plan-color); color: var(--plan-color); background: none; }
```
- 手間: 小

### F-6. Diary（連載記事のための日付主導レイアウト）

- 1つの投稿を1冊にするのではなく、**同じクリエイターの投稿を束ねて読む**ことを前提にした体裁。各投稿の先頭に日付（大）＋表題（中）を置き、目次は日付順に並ぶ。
- 単体書き出しでは「日記の1日分」に見えるだけだが、[統合書き出し](epub-merge-export-proposal-2026-08-16.md) と組み合わせると、そのまま「◯◯の2026年上半期」という1冊になる。
- 手間: 中（`work_title` 相当のパーツを共有する）

---

## 7. 推奨する組み合わせ

3段階で提示する。上から順に積める。

| 段階 | 内容 | 影響範囲 | 想定工数 |
|---|---|---|---|
| **最小** | 基盤A/B/C + D-1 + D-6 + P-2 + F-2 + F-5 | CSS 中心。XHTML は info_page の 2 周化のみ | 1〜2日 |
| **標準** | 上記 + D-2 + D-7 + P-1 + P-3 + P-4 + F-1 + F-4 | テンプレート3種の info_page と nav | +2〜3日 |
| **意欲的** | 上記 + D-3（奥付）+ D-4（日本語組版）+ F-3 + D-5/P-6（縦書き別テンプレート） | ビルダーと converter に手が入る | +1週間 |

「最小」だけでも W1・W2・W5 が解け、**既存テンプレートを編集していない利用者には自動で反映される**（`_base_style.css.j2` は組み込みから読まれるため）。編集済みの利用者には、テンプレートスタジオの「既定に戻す」導線を案内する。

---

## 8. 検証方法

1. **テンプレートスタジオのプレビュー**（`preview_epub_template`）— 3テンプレート × 情報ページ／本文で目視。
2. **`cargo test --manifest-path src-tauri/Cargo.toml`** — `builder.rs` の通し検査（敵対的メタデータの本が EPUBCheck 相当の内部検証を通ること）。
3. **EPUBCheck 5.3.0** — 実データから書き出した pixiv / FANBOX の 2 冊で error 0 / warning 0 を維持。
4. **端末確認** — Kindle Previewer（変換後の見た目）、Thorium / Apple Books（`prefers-color-scheme` のダーク）、Kobo（float の組み）。
5. **ビジュアル回帰** — `e2e/__screenshots__/…/epub-templates.png` が既にあるため、プレビュー画面のスクリーンショットで差分を確認できる。
