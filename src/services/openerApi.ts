import { openUrl } from "@tauri-apps/plugin-opener";
import { invoke } from "@tauri-apps/api/core";

export function openExternalUrl(url: string): Promise<void> {
  return openUrl(url);
}

/**
 * 場所をファイルマネージャーで開く。
 *
 * プラグインの `openPath` は使わない。あれは capability に書いた固定の
 * スコープでしか開けず、保存先も EPUB の出力先も使う人が決めるものなので、
 * 静的な glob には書けない。「Not allowed to open path」はそれが理由だった。
 * どこなら開いてよいかはアプリ自身が判断する。
 */
export function openFilesystemPath(path: string): Promise<void> {
  return invoke<void>("open_managed_path", { path });
}

export function revealPathInFileManager(path: string): Promise<void> {
  return invoke<void>("reveal_managed_path", { path });
}
