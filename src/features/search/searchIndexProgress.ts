import { useSyncExternalStore } from "react";
import { isTauriRuntime } from "@/services/dbApi";
import { onTauriEvent } from "@/services/eventBus";
import type { SearchRebuildProgress } from "@/types/library";

/**
 * The state of whichever search index run is happening right now, wherever it
 * was started.
 *
 * The app catches its own index up in the background at launch, so the screen
 * that needs to report progress is usually not the screen that asked for it.
 * Keeping this outside React lets the header, the settings page and anything
 * else read the same run.
 */
let current: SearchRebuildProgress | null = null;
const listeners = new Set<() => void>();
let subscribed = false;
let clearTimer: number | undefined;

/** How long a finished run stays visible, so it is seen and not just guessed at. */
const SETTLE_MS = 4_000;

function emit() {
  listeners.forEach((listener) => listener());
}

function set(next: SearchRebuildProgress | null) {
  current = next;
  emit();
}

function ensureSubscription() {
  if (subscribed || !isTauriRuntime()) return;
  subscribed = true;
  void onTauriEvent<SearchRebuildProgress>("search-index-progress", (event) => {
    const progress = event.payload;
    if (clearTimer !== undefined) {
      window.clearTimeout(clearTimer);
      clearTimer = undefined;
    }
    set(progress);
    if (progress.status === "running") return;
    clearTimer = window.setTimeout(() => {
      clearTimer = undefined;
      if (current?.jobId === progress.jobId) set(null);
    }, SETTLE_MS);
  }).catch(() => undefined);
}

function subscribe(listener: () => void) {
  ensureSubscription();
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useSearchIndexProgress(): SearchRebuildProgress | null {
  return useSyncExternalStore(subscribe, () => current, () => null);
}

export function isRebuildRunning(progress: SearchRebuildProgress | null): boolean {
  return progress?.status === "running";
}

/** Fraction 0–100 of the run in progress, or null when it cannot be known yet. */
export function rebuildPercent(progress: SearchRebuildProgress | null): number | null {
  if (!progress?.processedTotal) return null;
  return Math.min(100, (progress.processed ?? 0) / progress.processedTotal * 100);
}

