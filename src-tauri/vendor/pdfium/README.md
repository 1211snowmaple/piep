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

## 使用許諾を配る

BSD 3-Clause も同梱物の許諾も、配布物に許諾の文を添えることを求める。

- デスクトップ版：`bundle.resources` で `LICENSE` と `BUILD_LICENSES/` を DLL と一緒に配る。
- 単独サーバー：zip は平らな名前しか受け取らないので、一つにまとめた `pdfium-licenses.txt` として配る。
  更新と導入はこの名前を受け取れるようになった。ただし、今動いているサーバーの更新は知らない名前の入った
  zip を丸ごと断る。そのため、zip に入れるのは、受け取れる版が出回った**次のリリースから**にする
  （`tools/package-server.ps1` に足す）。

## 上げるとき

1. [releases](https://github.com/bblanchon/pdfium-binaries/releases) から
   `pdfium-win-x64` を取り、`bin/pdfium.dll` をここへ置く
2. `version.json` を新しい版に直す
3. `cargo test -p piep pdf::` を通す。棚の実物の添付 PDF でも、取り出した段落数と文字数が
   変わっていないことを前後で見比べる
4. `LICENSE` と `BUILD_LICENSES/` を新しい版のものに入れ替える

版を上げる理由が無いなら上げない。**ここは piep が読む形の要であって、
新しいほど良い場所ではない。**
