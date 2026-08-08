import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";

interface WorkspaceContextValue {
  epubQueue: number[];
  addToEpubQueue: (ids: number | number[]) => void;
  removeFromEpubQueue: (ids: number | number[]) => void;
  clearEpubQueue: () => void;
  isQueuedForEpub: (id: number) => boolean;
}

const WorkspaceContext = createContext<WorkspaceContextValue | null>(null);
const STORAGE_KEY = "piep.epub-queue.v2";

function readQueue(): number[] {
  try {
    const value = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "[]") as unknown;
    return Array.isArray(value) ? [...new Set(value.filter((id): id is number => Number.isInteger(id) && id > 0))] : [];
  } catch {
    return [];
  }
}

// Storage is unavailable in tests and can throw on quota in a real webview;
// losing the persisted queue must never take the whole screen down with it.
function writeQueue(queue: number[]) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(queue));
  } catch {
    /* Keep the in-memory queue authoritative for this session. */
  }
}

export function WorkspaceProvider({ children }: { children: ReactNode }) {
  const [epubQueue, setEpubQueue] = useState<number[]>(readQueue);
  // Writing to storage inside the state updater runs twice under StrictMode and
  // makes the reducer impure, so persistence is a plain effect on the result.
  useEffect(() => { writeQueue(epubQueue); }, [epubQueue]);
  const addToEpubQueue = useCallback((input: number | number[]) => {
    const ids = Array.isArray(input) ? input : [input];
    setEpubQueue((current) => {
      const next = [...new Set([...current, ...ids])];
      return next.length === current.length ? current : next;
    });
  }, []);
  const removeFromEpubQueue = useCallback((input: number | number[]) => {
    const ids = new Set(Array.isArray(input) ? input : [input]);
    setEpubQueue((current) => {
      const next = current.filter((id) => !ids.has(id));
      return next.length === current.length ? current : next;
    });
  }, []);
  const clearEpubQueue = useCallback(() => setEpubQueue((current) => (current.length ? [] : current)), []);
  // A Set keeps the per-card lookup O(1); `Array.includes` on every rendered
  // card turned a long queue into quadratic work across a full library page.
  const queuedIds = useMemo(() => new Set(epubQueue), [epubQueue]);
  const value = useMemo<WorkspaceContextValue>(() => ({
    epubQueue,
    addToEpubQueue,
    removeFromEpubQueue,
    clearEpubQueue,
    isQueuedForEpub: (id) => queuedIds.has(id),
  }), [addToEpubQueue, clearEpubQueue, epubQueue, queuedIds, removeFromEpubQueue]);
  return <WorkspaceContext.Provider value={value}>{children}</WorkspaceContext.Provider>;
}

export function useWorkspace() {
  const value = useContext(WorkspaceContext);
  if (!value) throw new Error("useWorkspace must be used inside WorkspaceProvider");
  return value;
}
