# Android 配布版の作成

Android 版は、デスクトップ版・単独サーバーと同じ piep のリリース（タグ `vX.Y.Z`）に載る。組み立て・署名・載せる作業は Release ワークフロー（`.github/workflows/release.yml`）がする。手で `gh release` を叩かない。

Android の APK 更新には同じアプリ ID、同じ署名鍵、増加する `versionCode` が必要です。鍵と `keystore.properties` は Git に追加しません。鍵を失うと、現在のアプリ ID のまま既存インストールへ更新を配布できなくなります。

## 版の決め方

版の番号はデスクトップ版と同じ一本の流れに乗せる。ただし、携帯に関わるもの（`android/`・`shared/`・`crates/`。説明の `.md` は除く）が変わっていないリリースでは、携帯の版を上げない。上げると中身が同じでも携帯に更新の通知が出るからである。

- **携帯側が変わったリリース：** `package.json`、`src-tauri/tauri.conf.json`、`src-tauri/Cargo.toml`（と二つの lock）の版を、デスクトップ版と同じ新しい版にする。Tauri はこの版から Android の `versionCode` を生成する。
- **変わっていないリリース：** 携帯の版は前に APK を出した版のまま触らない。Release は前の版の APK と `latest-android.json` をそのまま新しいリリースへ載せ直す。案内の版が変わらないので通知は出ず、配布ページにはいつも APK がある。

どちらにするかは `tools/android-release-plan.ps1`（リポジトリの根）が版のファイルから決めて確かめる。携帯側が変わったのに版を上げていなければ、Release はそこで止まり、上げるべき版を示す。

## 署名鍵

鍵は GitHub の `release` Environment の Secrets にあり、`v*` のタグからしか読めない。

| Secret | 中身 |
|---|---|
| `ANDROID_KEYSTORE_BASE64` | `piep-android-release.jks` を base64 にしたもの |
| `ANDROID_KEYSTORE_PASSWORD` | ストアのパスワード |
| `ANDROID_KEY_ALIAS` | 鍵の別名（`piep-android-release`） |
| `ANDROID_KEY_PASSWORD` | 鍵のパスワード |

Secrets は書き込み専用で、あとから取り出せない。`.jks` とパスワードの控えはリポジトリとも GitHub とも別の安全な場所（パスワード管理アプリなど）に置く。公開済みの版を作り直す場合も署名鍵を変更しない。

## 手元で組む

配布はしないが、実機やエミュレーターで配布版を試したいときは手元でも組める。

1. `src-tauri/gen/android/keystore.properties` に `storeFile`、`storePassword`、`keyAlias`、`keyPassword` を書く。終わったら消す。
2. `pwsh tools/release.ps1 -Version <版>` を実行する。`release-output/` に署名済み ARM64 APK と `latest-android.json` ができる。

`-Unsigned` を付けると鍵なしで組めることだけを確かめる（PR の CI がこれを使う）。その APK は配布にも上書きにも使えない。

`tools/release.ps1` は次を確かめ、どれかが違えば止まる。

- 3 か所の版が指定と同じこと、署名鍵の設定があること、試験用の更新先 `PIEP_ANDROID_TEST_UPDATE_SOURCE` が設定されていないこと。
- 組み上がった APK のアプリ ID、版、署名（`aapt`、`apksigner`）。
- APK の中にビルド端末のユーザープロファイルのパスが含まれていないこと。Rust の成果物には `--remap-path-prefix` を付けて組む。

## アプリ内更新の仕組み

アプリは 1 日 1 回、piep の最新のリリースに添付された `latest-android.json` を読む。

```json
{ "version": "0.42.0", "versionCode": 42000, "url": "https://github.com/1211snowmaple/piep/releases/download/v0.42.0/piep-android-v0.42.0-arm64.apk", "sha256": "…", "size": 12345678 }
```

- 案内の `url` が piep のリリース以外を指していれば使わない。SHA-256 と大きさは壊れた取得を捨てるためにある。
- 案内は受信中に 64KiB、APK は案内の大きさ（最大 512MiB）までに制限する。取得済み APK の SHA-256 は全体をメモリへ載せず、64KiB ずつ読んで照合する。
- 当てるかどうかを最後に決めるのは OS で、同じ署名鍵で `versionCode` が増えていなければ入らない。利用者は毎回 OS の確認画面で「更新」を選ぶ。
- リリースに `latest-android.json` が無いと、そこから先の版を見つけられない。Release は載せる物の一覧を確かめてから終わる。

開発用デバッグ APK は配布版へ上書きできない。

R8 は ML Kit の内部を消すと実行時に落とすので、`src-tauri/gen/android/app/proguard-rules.pro` の保持指定を外さない。QR 読み取りを変えたら release ビルドで開いて確かめる。
