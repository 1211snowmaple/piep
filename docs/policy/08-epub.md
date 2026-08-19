# EPUB の方針

保存した作品を EPUB 3 として書き出す。**持ち出しのためであって、配布のためでは
ない。**

## 何に準拠しているか

| 基準 | 何に使っているか |
|---|---|
| [EPUB 3.3 (W3C Recommendation)](https://www.w3.org/TR/epub-33/) | OCF/ZIP、Package Document、Manifest、Spine、Navigation、XHTML、CSS の適合条件 |
| [EPUB Accessibility 1.2](https://www.w3.org/TR/epub-a11y-12/) | `accessMode` などの付与 |
| [EPUBCheck 5.3.0](https://github.com/w3c/epubcheck) | 公式の検証器。内部検証とは別に、実生成物の最終判定に使う |
| Send to Kindle / Kindle Publishing Guidelines | 目次・表紙・画像・HTML/CSS の実装判断 |

保存済みの pixiv / FANBOX 実データから生成した本は、EPUBCheck 5.3.0 で
fatal 0 / error 0 / warning 0 / usage 0 になることを確認している。

### 保証しないこと

**Send to Kindle での変換成功を仕様だけで永久保証することはしない。**
Amazon の変換処理は非公開であり、将来変更されうる。公開ガイドラインに合わせては
いるが、確認できているのは「EPUB 3.3 として適合していること」までである。

これは [設計原則](02-principles.md) の「推測はするが、断定はしない」の適用例である。

## 規格上ゆるがせにできない点

`epub/builder.rs` が担保している。

- `mimetype` が ZIP の**先頭**の項目で、無圧縮・追加フィールドなし
- マニフェストの ID が NCName で、重複しない
- href が URL として書かれ、指す先が実在する
- 本文が参照する資源がすべてマニフェストに載っている

`dcterms:modified` には**投稿の更新日ではなく、その EPUB を生成した時刻**を
UTC で書く。作品の識別子には `urn:pixiv:novel:{id}` / `urn:fanbox:post:{id}` を使い、
同じ作品からは常に同じ識別子が出るようにする。

### アクセシビリティは正直に書く

挿絵の内容を安全に推測することはできない。だから画像のある本について
`textual` だけで十分とは宣言せず `textual,visual` とし、要約には
**「説明的な代替テキストがない」ことを明示する**。

書けることを書くのではなく、**生成物について真実な範囲だけを書く**。

## 目次

Kindle は論理目次が完全であることを求める。

- EPUB 3 の `nav.xhtml`（`properties="nav"` を持つ項目をちょうど1つ）
- 旧 Kindle 系のための NCX
- `nav.xhtml` 自体も Spine に入れ、本文から HTML 目次として開けるようにする

## テンプレート

標準／pixiv／FANBOX の3種を用意し、利用者が編集できる（テンプレートスタジオ）。
編集したものは `templates/` に保存される。

| 決めたこと | |
|---|---|
| 色 | トークン化し、`prefers-color-scheme` でダーク端末に追随する |
| 余白 | `--space` を基準に一元化する |
| 情報ページ | 見出し部＋明細部の二部構成（狭い画面では自動で縦積み） |
| シリーズ帯 | 話数は帯の中に置く。右端へ飛ばさない |
| 統計 | 1行に収める（3カラムの float をやめ、表紙が本来の大きさに戻った） |

### 縦書きは採用しない

端末ごとの差が大きく、**再現性を保証できない**ため。価値がないからではなく、
piep が「こう表示される」と言えないからである。

### 図版のキャプション

CSS 側の受け皿（`.illustration` の余白と改ページ抑止、`.caption`）は用意して
あるが、**converter がまだキャプションを出力していない**。絵の説明が実際に
出るのは変換側の対応後になる。

## 複数の作品を1冊にまとめる

`export_collection_epub` でコレクションを1冊として書き出せる。
順序はコレクションの並びをそのまま使い、**その時点のスナップショット**になる。

- シリーズであることを条件にしない。pixiv と FANBOX をまたいでもよい
- 1つの元マニフェストが、1冊の中の1つ以上の章になる

**コレクションが正本で、EPUB キューは書き出し用の一時的な作業領域である。**

→ [コレクション](04-data.md#コレクション)

## 画像の最適化

`oxipng` / `zenjpeg` / `webp` で圧縮する。書き出し時に指定できる。

安全側の上限を設けてある（1枚あたりの元サイズ、総量、画素数、枚数）。
数千点の資源を持つ本が、ZIP へ流し込む前にプロセスを食い潰さないようにするため。

## 検証

- `epub::builder::tests::every_builtin_template_renders_its_own_front_matter`
  — 3テンプレートを実際に組んで内部検証を通す
- `cargo run --example epub_specimen -- <出力先>`
  — 情報ページを1枚の HTML に書き出してブラウザで見る
- アプリ内の `validate_epub_file`
