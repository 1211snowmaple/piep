# AI 開発支援の入口（Android 版）

Android 版は Tauri 2 / Rust / React 19 / Vite 8 の別アプリ。現在 Mantine は使っていない。製品の実装順は `PLAN.md`、エミュレーターの操作は `SETUP.md`、PC との契約は `PROTOCOL.md` を参照する。

Codex の全プロジェクト設定に `mantine`、`playwright`、`openaiDeveloperDocs` MCP を登録済み。各サーバーの MCP 初期化応答も確認した。`mantine` は主にデスクトップ版用。`playwright` はブラウザー画面の検証に使え、Android WebView 自体の検証は `npx tauri android dev` とエミュレーターで行う。`openaiDeveloperDocs` は Codex や OpenAI 製品/API の資料用。各設定は `codex mcp get <名前>` で確認できる。新しいツールは Codex の新しいチャットで利用可能性を確認する。

## 公式資料

- [Tauri 2 Android の前提条件](https://v2.tauri.app/start/prerequisites/)・[モバイル開発](https://v2.tauri.app/develop/)
- [Tauri 2 の外部 URL を開くプラグイン](https://v2.tauri.app/ja/plugin/opener/)・[Android の戻る操作](https://developer.android.com/guide/navigation/custom-back)
- [Tauri 2 LLM 向け索引](https://v2.tauri.app/llms.txt)・[設定リファレンス](https://v2.tauri.app/reference/config/)・[JavaScript API](https://v2.tauri.app/reference/javascript/api/)
- [React LLM 向け索引](https://react.dev/llms.txt)・[リファレンス](https://react.dev/reference/react)
- [Vite の LLM 向け資料](https://vite.dev/llms.txt)
- [Rust 公式ドキュメント](https://doc.rust-lang.org/stable/)
- [Android Developers](https://developer.android.com/docs)
- [OpenAI Docs MCP の設定](https://developers.openai.com/learn/docs-mcp)

実装時は `package-lock.json`、`src-tauri/Cargo.lock` と公式資料を照合する。Tauri 向けコミュニティ MCP はアプリ側のデバッグ用プラグインが必要なため、現在は導入していない。

Android WebView の結合確認には `adb forward` で DevTools を開き、`tools/devtools-eval.mjs` から画面の状態を読み取れる。これは開発用で、製品の機能には含めない。本文 HTML の許可リストを変える際は [DOMPurify の公式 README](https://github.com/cure53/DOMPurify) を確認する。

本文のリンク処理は `src/OnlineReader.tsx`、Android の戻る操作は `src-tauri/gen/android/app/src/main/java/io/github/u1211snowmaple/piep/android/MainActivity.kt` と `src/App.tsx` にある。Tauri が自動生成する `generated/` 配下は編集しない。
