import { listen, type EventCallback, type UnlistenFn } from "@tauri-apps/api/event";

export function onTauriEvent<T>(event: string, handler: EventCallback<T>): Promise<UnlistenFn> {
  return listen<T>(event, handler);
}

/**
 * Installs an async Tauri listener while exposing a synchronous React cleanup.
 *
 * Tauri resolves `listen` asynchronously. A component can unmount before that
 * happens; assigning the resolved unlisten function to a local variable then
 * leaks the native listener forever. This wrapper immediately records the
 * disposal request and invokes a late unlisten as soon as it arrives.
 */
export function subscribeTauriEvent<T>(event: string, handler: EventCallback<T>): UnlistenFn {
  let disposed = false;
  let unlisten: UnlistenFn | undefined;
  void onTauriEvent<T>(event, handler)
    .then((registered) => {
      if (disposed) registered();
      else unlisten = registered;
    })
    .catch(() => undefined);
  return () => {
    disposed = true;
    unlisten?.();
    unlisten = undefined;
  };
}
