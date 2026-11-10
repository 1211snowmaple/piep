# 共通の可逆保管

デスクトップと Android が使う、Tauri・SQLite・HTTP に依存しない保管処理。
`PIEPOB01`、元バイト列の SHA-256、manifest の serde 表現は切り出し前と同じ。
バックグラウンド処理からも WebView を起動せず利用できる。

- 8 MiB 以下は単一実体、大きいファイルは 4 MiB ごとに分割する。
- v1 の読み取り上限は、展開後 8 MiB、保存 payload 16 MiB とヘッダー 57 byte。
  新規のエンコードでは、十分な削減が得られる場合だけ Zstandard を選び、
  それ以外は raw とする。Zstandard の展開ウィンドウも 8 MiB に制限する。
- `import_chunk` は長さ・形式・展開結果・元ハッシュを確認した一時ファイルだけを
  上書きせず公開する。途中失敗時は `.part` を片付け、検証済みの他の実体は残す。
  入力は一つの HTTP response body 等、その実体で終わるストリームを渡す。
  Android ではハードリンクを使わず、`renameat2(RENAME_NOREPLACE)` で公開する。
  Windows は置き換えない `MoveFileExW` で公開し、exFAT・FAT32 の外付けドライブでも使える。
  非対応の OS・保存先では失敗を返し、上書きや公開途中のコピーへ切り替えない。
- 既存の実体が壊れていた場合、`put` と `import_chunk` は検証済みの同じ中身で置き換える。
  壊れたファイルは `<hash>.<UUID>.damaged` として同じ場所に残し、この crate では消さない。
  壊れた入力では置き換えない。
- `export_chunk` は検証した保存バイト列を送り、転送のための再圧縮を行わない。
  `has_verified_chunk` は未受信と破損を区別する。受信し直す部分を決めるときは、
  検証済み・未受信・破損を返す `chunk_state` を使う。
- 一つの chunk の受信成功はファイル全体や読書 bundle の完成を意味しない。
  `verify` で全 chunk と元ファイル全体のハッシュを検査した後、呼び手の DB で
  bundle・利用者・世代を照合して公開する。
- この crate は認可しない。サーバーは作品・保存版・file の所属を検査してから
  使用する。端末は接続・棚・利用者ごとの管理領域に分離する。
  `ObjectStore::open` に渡すルートと祖先は、呼び手が信頼できる領域に限定する。

`tests/fixtures` は切り出し前の `dbb18b1` の検証道具が生成した合成データ。
既存の raw / Zstandard をそのまま読み、転送後も保存バイト列が同じことを確認する。
エンコーダーの将来の出力を同一バイト列に固定するものではない。

```sh
cargo test --manifest-path crates/piep-storage/Cargo.toml --locked
cargo clippy --manifest-path crates/piep-storage/Cargo.toml --all-targets --locked -- -D warnings
```

HTTP API v2、認証、端末 DB、bundle の原子的な公開や GC はそれぞれの層で実装する。
