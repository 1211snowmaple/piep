import { useCallback, useEffect, useState } from "react";

export interface Bookmark {
  id: string;
  page: number;
  top: number;
  label: string;
  createdAt: string;
}

const STORAGE_KEY = "piep.bookmarks.v1";
const CHANGED_EVENT = "piep:bookmarks-changed";

type Store = Record<string, Bookmark[]>;

function readStore(): Store {
  try {
    const raw = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}") as unknown;
    return raw && typeof raw === "object" ? raw as Store : {};
  } catch {
    return {};
  }
}

function writeStore(store: Store) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(store));
  } catch {
    /* Bookmarks are a convenience; a full quota must not break reading. */
  }
  window.dispatchEvent(new CustomEvent(CHANGED_EVENT));
}

/**
 * Bookmarks live in local storage rather than the library database: they are a
 * per-device reading aid, and keeping them here means adding one costs nothing
 * and works the same in the browser preview.
 */
export function useBookmarks(workId: number) {
  const [bookmarks, setBookmarks] = useState<Bookmark[]>([]);

  const refresh = useCallback(() => {
    setBookmarks(readStore()[String(workId)] ?? []);
  }, [workId]);

  useEffect(() => {
    refresh();
    // Keeps every reader instance (and a second window) in step.
    window.addEventListener(CHANGED_EVENT, refresh);
    window.addEventListener("storage", refresh);
    return () => {
      window.removeEventListener(CHANGED_EVENT, refresh);
      window.removeEventListener("storage", refresh);
    };
  }, [refresh]);

  const add = useCallback((bookmark: Omit<Bookmark, "id" | "createdAt">) => {
    const store = readStore();
    const key = String(workId);
    const entry: Bookmark = { ...bookmark, id: crypto.randomUUID(), createdAt: new Date().toISOString() };
    store[key] = [...(store[key] ?? []), entry].sort((a, b) => a.page - b.page || a.top - b.top);
    writeStore(store);
    return entry;
  }, [workId]);

  const remove = useCallback((id: string) => {
    const store = readStore();
    const key = String(workId);
    store[key] = (store[key] ?? []).filter((item) => item.id !== id);
    if (!store[key].length) delete store[key];
    writeStore(store);
  }, [workId]);

  return { bookmarks, add, remove };
}
