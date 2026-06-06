import { listen, type EventCallback, type UnlistenFn } from "@tauri-apps/api/event";

export function onTauriEvent<T>(event: string, handler: EventCallback<T>): Promise<UnlistenFn> {
  return listen<T>(event, handler);
}
