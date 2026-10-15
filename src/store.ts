import { LazyStore } from "@tauri-apps/plugin-store";

// アプリ全体で共有される単一のLazyStoreインスタンス
export const store = new LazyStore("settings.json");
