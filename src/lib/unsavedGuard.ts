import { isTauriRuntime } from "@/services/dbApi";

type Guard = () => boolean;

/**
 * 何から守るのか。
 *
 * 二つは別の話である。書きかけの本文は、画面を離れれば消える - どちらでも
 * 止めて訊く。一方、走っている保存はアプリの中では続いていて、進行状況も
 * 中止もアクティビティから触れる。移動のたびに「失われるかもしれません」と
 * 訊くのは、事実でないうえ邪魔でしかない。閉じるときだけは本当に消える。
 */
export type UnsavedGuardScope = "close" | "navigate";

const ALL_SCOPES: readonly UnsavedGuardScope[] = ["close", "navigate"];

const guards = new Map<Guard, readonly UnsavedGuardScope[]>();

/**
 * Registers a check for work that would be lost now.
 * Returns the unregister function.
 */
export function registerUnsavedGuard(guard: Guard, scopes: readonly UnsavedGuardScope[] = ALL_SCOPES): () => void {
  guards.set(guard, scopes);
  return () => { guards.delete(guard); };
}

export function hasUnsavedWork(scope: UnsavedGuardScope = "close"): boolean {
  for (const [guard, scopes] of guards) {
    if (!scopes.includes(scope)) continue;
    try {
      if (guard()) return true;
    } catch {
      /* A broken guard must not be able to block the window from closing. */
    }
  }
  return false;
}

/**
 * `beforeunload` is a browser-only contract: closing the Tauri window never
 * fires it, so unsaved edits vanished without a prompt. This hooks the native
 * close request instead.
 *
 * Every failure path lets the window close. Trapping the user in an app they
 * cannot quit would be far worse than losing a draft.
 */
export async function installCloseGuard(confirmDiscard: () => Promise<boolean>) {
  if (!isTauriRuntime()) return () => undefined;
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    const appWindow = getCurrentWindow();
    let closing = false;
    return await appWindow.onCloseRequested(async (event) => {
      if (closing || !hasUnsavedWork("close")) return;
      event.preventDefault();
      let discard = false;
      try {
        discard = await confirmDiscard();
      } catch {
        discard = true;
      }
      if (!discard) return;
      closing = true;
      try {
        await appWindow.destroy();
      } catch {
        guards.clear();
        await appWindow.close().catch(() => undefined);
      }
    });
  } catch {
    return () => undefined;
  }
}
