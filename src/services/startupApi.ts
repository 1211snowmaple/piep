import { invoke } from "@tauri-apps/api/core";
import { isTauriRuntime } from "@/services/dbApi";

/**
 * The library opens off the main thread, and commands that need it fail until it
 * has. Nothing may call them before this resolves.
 */
export async function waitForLibrary(): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke<void>("startup_wait_for_library");
}

/** The window is coloured before the webview can read this preference. */
export async function rememberWindowColorScheme(scheme: "light" | "dark" | "auto"): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke<void>("startup_remember_color_scheme", { scheme });
}

/** Keeps the startup screen up, saying what went wrong, when the library cannot open. */
export function showStartupFailure(error: unknown): void {
  const splash = document.querySelector(".startup-splash");
  const message = splash?.querySelector(".startup-splash__message");
  if (!splash || !message) return;
  splash.setAttribute("data-failed", "");
  message.textContent = "piepを起動できませんでした";
  const detail = document.createElement("p");
  detail.className = "startup-splash__detail";
  detail.textContent = String(error);
  splash.append(detail);
}
