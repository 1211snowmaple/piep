# piep Android

[piep デスクトップ版](https://github.com/1211snowmaple/piep)の小説を Android で読むための付属アプリです。piep サーバーにつないで本棚を検索して読み、選んだ作品だけ端末に保存してオフラインで読めます。読書位置は端末のあいだで同期します。Android 版はデスクトップ版とは別の Tauri 2 / React / Rust プロジェクトです。

Android 端末へのインストールは [最新版の配布ページ](https://github.com/1211snowmaple/piep-android/releases/latest) から APK をダウンロードします。操作は [INSTALL.md](INSTALL.md) にまとめました。

開発環境とエミュレーターでの起動方法は [SETUP.md](SETUP.md)、接続 API は [PROTOCOL.md](PROTOCOL.md)、デスクトップ版との表示対応は [DESKTOP-PARITY.md](DESKTOP-PARITY.md) を参照してください。

配布版は GitHub Releases の署名済み APK です。アプリからの自動更新は準備中のため、新しい版が公開された際は配布ページから APK を再度開いて更新します。開発用のデバッグ APK は署名が異なります。
