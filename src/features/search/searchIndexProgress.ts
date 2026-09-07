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
    forwardToTracked(progress);
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

/**
 * 手で始めた作り直しを、画面から切り離して見張る。
 *
 * **見張りを画面の中に置いてはいけない。** 設定画面を離れると unmount され、
 * 覚えていた仕事の番号も操作履歴の控えも失われる。戻ってきても結び付けが
 * 無いので、履歴の進捗は最初の1塊で止まったままになる（実際、64件で
 * 止まっていた）。ヘッダーはこの入れ物を直接読むので動き続け、二つの表示が
 * 食い違って見えていた。
 *
 * ここは module の入れ物なので、どの画面にいても最後まで面倒を見る。
 */
interface TrackedRun {
  jobId: string;
  operation: { progress: (done: number, total?: number | null) => void; complete: (note?: string) => void; fail: (note?: string) => void; cancel: (note?: string) => void };
  onSettled?: (progress: SearchRebuildProgress) => void;
}

let tracked: TrackedRun | null = null;

export function trackManualRebuild(run: TrackedRun) {
  tracked = run;
}

/** 見張っている仕事の番号。画面はこれで「自分が始めたもの」を見分ける。 */
export function trackedRebuildJobId(): string | null {
  return tracked?.jobId ?? null;
}

function forwardToTracked(progress: SearchRebuildProgress) {
  const run = tracked;
  if (!run || progress.jobId !== run.jobId) return;
  run.operation.progress(progress.processed ?? progress.indexedDownloads, progress.processedTotal ?? progress.totalDownloads);
  if (progress.status === "running") return;
  tracked = null;
  const note = progress.status === "failed"
    ? (progress.error || "検索インデックスの再構築に失敗しました")
    : progress.status === "canceled"
      ? `${progress.processed ?? 0}件を処理した時点で中止しました`
      : `${progress.processed ?? 0}件を索引しました`;
  if (progress.status === "failed") run.operation.fail(note);
  else if (progress.status === "canceled") run.operation.cancel(note);
  else run.operation.complete(note);
  run.onSettled?.(progress);
}

/**
 * 試験から進捗を流し込む。**イベントの購読は Tauri でしか動かない。**
 * 見張りの結び付きは画面から切り離した約束なので、そこだけを直に確かめる。
 */
export function __testEmitSearchIndexProgress(progress: SearchRebuildProgress) {
  set(progress);
  forwardToTracked(progress);
}
