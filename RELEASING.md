# Android 配布版の作成

Android の APK 更新には同じアプリ ID、同じ署名鍵、増加する `versionCode` が必要です。鍵と `keystore.properties` は Git に追加しません。鍵を失うと、現在のアプリ ID のまま既存インストールへ更新を配布できなくなります。

1. `package.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml` の版を揃えて更新する。Tauri はこの版から Android の `versionCode` を生成する。
2. `src-tauri/gen/android/keystore.properties` に `storeFile`、`storePassword`、`keyAlias`、`keyPassword` を設定する。署名鍵はリポジトリ外に保管する。
3. `pwsh tools/release.ps1 -Version <版>` を実行する。`release-output/` に署名済み ARM64 APK と更新の案内 `latest-android.json` ができる。実機で確かめるならここで入れる。
4. 変更をコミットして `v<版>` タグを付けたうえで、`pwsh tools/release.ps1 -Version <版> -Publish` を実行する。GitHub Releases に APK と案内が載る。実機未検証などの制限があればリリースノートに書き足す。

`tools/release.ps1` は次を確かめ、どれかが違えば止まる。

- 3 か所の版が指定と同じこと、署名鍵の設定があること、試験用の更新先 `PIEP_ANDROID_TEST_UPDATE_SOURCE` が設定されていないこと。
- 組み上がった APK のアプリ ID、版、署名（`aapt`、`apksigner`）。
- APK の中にビルド端末のユーザープロファイルのパスが含まれていないこと。Rust の成果物には `--remap-path-prefix` を付けて組む。

## アプリ内更新の仕組み

アプリは 1 日 1 回、最新のリリースに添付された `latest-android.json` を読む。

```json
{ "version": "0.3.0", "versionCode": 3000, "url": "https://github.com/1211snowmaple/piep-android/releases/download/v0.3.0/piep-android-v0.3.0-arm64.apk", "sha256": "…", "size": 12345678 }
```

- 案内の `url` がこのリポジトリのリリース以外を指していれば使わない。SHA-256 と大きさは壊れた取得を捨てるためにある。
- 案内は受信中に 64KiB、APK は案内の大きさ（最大 512MiB）までに制限する。取得済み APK の SHA-256 は全体をメモリへ載せず、64KiB ずつ読んで照合する。
- 当てるかどうかを最後に決めるのは OS で、同じ署名鍵で `versionCode` が増えていなければ入らない。利用者は毎回 OS の確認画面で「更新」を選ぶ。
- リリースに `latest-android.json` が無いと、そこから先の版を見つけられない。手で添付するときも必ず一緒に載せる。

公開済みの版を作り直す場合も署名鍵を変更しない。開発用デバッグ APK は配布版へ上書きできない。署名鍵とパスワードはリポジトリとは別の安全な場所にバックアップする。

R8 は ML Kit の内部を消すと実行時に落とすので、`src-tauri/gen/android/app/proguard-rules.pro` の保持指定を外さない。QR 読み取りを変えたら release ビルドで開いて確かめる。
