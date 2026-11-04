# Windows インストーラー

`installer.nsi` は Tauri CLI 2.12.4 の標準テンプレートを元にしている。

元のファイル: https://github.com/tauri-apps/tauri/blob/tauri-cli-v2.12.4/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi

元ファイルの SHA-256: `d00089d3b1a2c4a30d7b6e098079100cb8f25e980f84d4515aca4597b95e967d`

標準テンプレートのライセンスは、同梱の [TAURI-LICENSE-MIT.txt](TAURI-LICENSE-MIT.txt) を参照。

変更点はデスクトップショートカットの扱いだけ。

- 完了画面の「デスクトップにショートカットを作成する」は未選択を標準にする。
- サイレント・パッシブインストールでは、既存のショートカットがある場合だけ更新する。
- スタートメニューへの登録と `/NS`・`/UPDATE` の扱いは標準テンプレートに従う。

Tauri CLI を更新するときは、元のテンプレートとの差分も確認する。

`tools/test-installer-shortcuts.ps1` は、作成処理をテンプレートから切り出して実行する。
デスクトップ変数を作業フォルダーへ差し替え、既定の抑止、明示的な選択、既存リンクの更新、
`/NS` 相当の抑止を検証する。利用中のインストール先やデスクトップには触れない。
