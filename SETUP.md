# Android 開発環境

このリポジトリは Windows 版 piep の閲覧用 Android アプリです。デスクトップ版は別リポジトリで開発します。Android Studio、Android SDK、JDK 17、Rust の Android ターゲット、Node.js を用意します。Tauri 2 の [Android 前提条件](https://v2.tauri.app/start/prerequisites/) を参照してください。

## エミュレーターで起動

Android Studio の Device Manager で仮想端末を起動し、このリポジトリで次を実行します。

```powershell
npm ci
npx tauri android dev
```

コマンドラインで既存のスマホ・タブレット AVD を使う場合は、次のように名前を確認し、起動コマンドを別々の PowerShell 窓で実行します。名前は手元の `-list-avds` の出力に読み替えてください。

```powershell
& "$env:ANDROID_HOME\emulator\emulator.exe" -list-avds
& "$env:ANDROID_HOME\emulator\emulator.exe" -avd Piep_Pixel_8_API_36
& "$env:ANDROID_HOME\emulator\emulator.exe" -avd Piep_Pad5_Layout_API_36
adb devices -l
```

画面を APK に同梱した状態で確認するときは、両端末の起動後に次を実行します。`emulator-5554` と `emulator-5556` は `adb devices -l` の結果に合わせてください。

```powershell
npx tauri android build --debug --apk --target x86_64 --ci
$apk = "src-tauri\gen\android\app\build\outputs\apk\universal\debug\app-universal-debug.apk"
adb -s emulator-5554 install -r $apk
adb -s emulator-5556 install -r $apk
adb -s emulator-5554 shell monkey -p io.github.u1211snowmaple.piep.android 1
adb -s emulator-5556 shell monkey -p io.github.u1211snowmaple.piep.android 1
```

Windows では Tauri のシンボリックリンク生成に開発者モードが必要です。JDK の版が合わない場合は `JAVA_HOME` を JDK 17 のインストール先に設定してください。

PC 版の本棚に接続するには、共有機能を含むデスクトップ版で「設定 → Android で読む」から共有を開始し、Android 版の「設定 → PC との接続」に登録文字列を貼り付けます。エミュレーターから PC のループバック接続口へつなぐ場合は、接続前に次の転送を設定できます。

```powershell
adb reverse tcp:38777 tcp:38777
```

2 台を同時に使う場合は `adb -s emulator-5554 reverse tcp:38777 tcp:38777` のように端末を指定し、それぞれに設定します。

転送はエミュレーターの再起動などで消えることがあります。`adb reverse --list` で確認できます。家庭内ネットワークから接続する場合は、PC 側の共有先に LAN アドレスを選びます。

## 確認

```powershell
npm run build
npx tauri android build --debug --apk
```

デバッグ APK は動作確認用です。公開配布には署名済みリリースビルドと配布手順が別途必要です。接続方式は [PROTOCOL.md](PROTOCOL.md)、デスクトップ版との表示対応は [DESKTOP-PARITY.md](DESKTOP-PARITY.md) を参照してください。
実用化の監査項目と検証結果は [QUALITY-PLAN.md](QUALITY-PLAN.md) に記録します。
