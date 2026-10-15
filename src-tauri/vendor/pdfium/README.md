# pdfium

添付 PDF から本文を取り出すために使う、Chromium の PDF エンジン。
`src-tauri/src/pdf/` から `pdfium-render` 経由で呼ぶ。

| | |
| --- | --- |
| 版 | 153.0.7999（`version.json`） |
| 出所 | [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries) の Windows x64 ビルド |
| ライセンス | BSD 3-Clause（`LICENSE`）。同梱物の分は `BUILD_LICENSES/` |

## なぜバイナリを置いているか

pdfium-binaries は動的ライブラリしか配っていないので、静的リンクへ逃げられない。
取得スクリプトにする案もあったが、**ビルドに網を要求すると、CI と手元の両方に
新しい落ち方が増える。** 7.3MB は、この配布物（インストーラーで約57MB）の中では
小さい。置いて済ませる。

`tauri.conf.json` の `bundle.resources` で実行ファイルの隣へ配り、
`resolve_pdfium_library_path` が実行時に探す。

## 上げるとき

1. [releases](https://github.com/bblanchon/pdfium-binaries/releases) から
   `pdfium-win-x64` を取り、`bin/pdfium.dll` をここへ置く
2. `version.json` を新しい版に直す
3. `cargo test -p piep pdf::` を通す。棚の実物でも
   `scratch/pdf-probe-rs` を回して、段落数と文字数が変わっていないことを見る

版を上げる理由が無いなら上げない。**ここは piep が読む形の要であって、
新しいほど良い場所ではない。**
