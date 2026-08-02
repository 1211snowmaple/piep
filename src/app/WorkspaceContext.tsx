import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";

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

export function WorkspaceProvider({ children }: { children: ReactNode }) {
  const [epubQueue, setEpubQueue] = useState<number[]>(readQueue);
  const persist = useCallback((next: number[]) => {
    setEpubQueue(next);
    localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  }, []);
  const addToEpubQueue = useCallback((input: number | number[]) => {
    const ids = Array.isArray(input) ? input : [input];
    setEpubQueue((current) => {
      const next = [...new Set([...current, ...ids])];
      localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
      return next;
    });
  }, []);
  const removeFromEpubQueue = useCallback((input: number | number[]) => {
    const ids = new Set(Array.isArray(input) ? input : [input]);
    setEpubQueue((current) => {
      const next = current.filter((id) => !ids.has(id));
      localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
      return next;
    });
  }, []);
  const clearEpubQueue = useCallback(() => persist([]), [persist]);
  const value = useMemo<WorkspaceContextValue>(() => ({
    epubQueue,
    addToEpubQueue,
    removeFromEpubQueue,
    clearEpubQueue,
    isQueuedForEpub: (id) => epubQueue.includes(id),
  }), [addToEpubQueue, clearEpubQueue, epubQueue, removeFromEpubQueue]);
  return <WorkspaceContext.Provider value={value}>{children}</WorkspaceContext.Provider>;
}

export function useWorkspace() {
  const value = useContext(WorkspaceContext);
  if (!value) throw new Error("useWorkspace must be used inside WorkspaceProvider");
  return value;
}
