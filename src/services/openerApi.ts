import { openPath, openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";

export function openExternalUrl(url: string): Promise<void> {
  return openUrl(url);
}

export function openFilesystemPath(path: string): Promise<void> {
  return openPath(path);
}

export function revealPathInFileManager(path: string): Promise<void> {
  return revealItemInDir(path);
}
