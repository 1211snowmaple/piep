import { isTauriRuntime } from "@/services/dbApi";

type Guard = () => boolean;

const guards = new Set<Guard>();

/**
 * Registers a check for work that would be lost if the app closed now.
 * Returns the unregister function.
 */
export function registerUnsavedGuard(guard: Guard): () => void {
  guards.add(guard);
  return () => { guards.delete(guard); };
}

export function hasUnsavedWork(): boolean {
  for (const guard of guards) {
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
      if (closing || !hasUnsavedWork()) return;
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
