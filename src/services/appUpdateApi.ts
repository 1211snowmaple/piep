import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { isTauriRuntime } from "@/services/dbApi";

/**
 * piep 自身の版を上げるための入り口。
 *
 * 「更新」という言葉はこのアプリでは二つの意味を持つ。作品の更新監視
 * （`features/updates`）は保存元に新しい話が出ていないかを見るもので、こちらは
 * アプリの実行ファイルそのものを入れ替える。混ざると危険なので、名前と場所を
 * 分けてある。
 *
 * 配布物の署名検証は Tauri 側で行われる。公開鍵が tauri.conf.json に無ければ
 * 確認は失敗し、その理由がそのまま呼び出し側へ返る。
 */

export type { Update };

export interface AppUpdateProgress {
  downloaded: number;
  /** 配信側が長さを告げないことがあるので、割合を出せるとは限らない。 */
  total: number | null;
}

/**
 * 新しい版があれば返す。無ければ `null`。
 *
 * ブラウザプレビューには入れ替える実行ファイルが無いので、そこでは常に `null`。
 */
export async function checkForAppUpdate(): Promise<Update | null> {
  if (!isTauriRuntime()) return null;
  return (await check()) ?? null;
}

/**
 * 落として入れるところまで。再起動は呼び出し側の判断に残す。
 *
 * 読んでいる途中で勝手に再起動されるのが最も困るので、この関数は入れ替えの
 * 準備までしかしない。
 */
export async function downloadAndInstallAppUpdate(
  update: Update,
  onProgress?: (progress: AppUpdateProgress) => void,
): Promise<void> {
  let downloaded = 0;
  let total: number | null = null;
  await update.downloadAndInstall((event) => {
    if (event.event === "Started") {
      total = event.data.contentLength ?? null;
      downloaded = 0;
    } else if (event.event === "Progress") {
      downloaded += event.data.chunkLength;
    }
    onProgress?.({ downloaded, total });
  });
}

/** 入れ替えた版で起動し直す。 */
export async function restartForAppUpdate(): Promise<void> {
  await relaunch();
}
