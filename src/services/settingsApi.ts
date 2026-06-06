import { store } from "@/store";

export function getSetting<T>(key: string): Promise<T | undefined> {
  return store.get<T>(key);
}

export function setSetting<T>(key: string, value: T): Promise<void> {
  return store.set(key, value);
}
