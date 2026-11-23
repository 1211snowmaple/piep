import { invoke, isTauri } from "@tauri-apps/api/core";
import { sameReadingScope, type PcWork, type ReadingScope } from "./pcApi";
import type { ReaderFont, ReaderLayout } from "./readerPrefs";
import { minimumReadProgress } from "./workText";

/**
 * 端末での読書の記録。Rust 側の SQLite（`reading_local_*`）に置く。
 *
 * 読書画面は位置を同期して読む必要があるので、開く前に `loadLocalReading` で
 * 記録を手元へ読み込み、そのあとは読み書きともにここで受け持つ。書き込みは
 * 順に Rust へ送り、送った時点で端末に確定する。
 */

// layout が違う位置は、割合だけを使って戻す。ページごとと一続きでは top の基準が違う。
export type SavedPosition = { page: number; top: number; ratio: number; fontSize: number; lineHeight: number; readerWidth: number; fontFamily: ReaderFont; layout?: ReaderLayout };

type ReadingKey = { scope: ReadingScope; workId: string };

export interface LocalReading {
  key: ReadingKey;
  /** 位置が指す保存版と編集。 */
  snapshotId: string;
  editId: string | null;
  page: number;
  position: SavedPosition | null;
  /** 0 から 1。 */
  progress: number;
  finished: boolean;
  /** 「続きを読む」に並べるか。開いただけの作品は並べない。 */
  shelved: boolean;
  /** この端末で最後に読み進めた時刻。0 はまだ読み進めていない。 */
  readAt: number;
  work: PcWork | null;
}

export interface RecentWork {
  work: PcWork;
  at: number;
  progress: number;
  finished: boolean;
}

export type WorkIdentity = Pick<PcWork, "readingScope" | "id" | "snapshotId" | "editId">;

const shelfChanged = "piep:recent-reading-changed";
const shelfLimit = 120;
const shelfNoticeMs = 4000;

const cache = new Map<string, LocalReading>();
const loading = new Map<string, Promise<void>>();
// 読み込めなかった作品。記録なしとして開き、画面を止めない。
const unreadable = new Set<string>();
const queued = new Map<string, LocalReading>();
let flushing: Promise<void> | null = null;
let currentScope: ReadingScope | null = null;
let shelfNoticeAt = 0;
let shelfTimer: number | undefined;
const lastShelves = new Map<boolean, RecentWork[]>();

const keyOf = ({ scope: { libraryId, subjectId }, workId }: ReadingKey) => `${libraryId}/${subjectId}/${workId}`;
const workKey = (row: LocalReading) => keyOf(row.key);
const identityOf = (work: WorkIdentity): ReadingKey => ({ scope: work.readingScope, workId: work.id });
export const localReadingKey = (work: WorkIdentity): string => keyOf(identityOf(work));

function sameRevision(row: LocalReading, work: WorkIdentity): boolean {
  return row.snapshotId === work.snapshotId && row.editId === work.editId;
}

/** 今の本文の記録。版が違う記録は、別の本文のものなので使わない。 */
function current(work: WorkIdentity): LocalReading | null {
  const row = cache.get(localReadingKey(work));
  return row && sameRevision(row, work) ? row : null;
}

/** 書き換える行。版が変わっていたら、前の版の位置と時刻を引き継がない。 */
function editable(work: WorkIdentity): LocalReading {
  const row = cache.get(localReadingKey(work));
  if (row && sameRevision(row, work)) return { ...row };
  return {
    key: identityOf(work),
    snapshotId: work.snapshotId,
    editId: work.editId,
    page: 0,
    position: null,
    progress: row?.progress ?? 0,
    finished: row?.finished ?? false,
    shelved: row?.shelved ?? false,
    readAt: 0,
    work: row?.work ?? null,
  };
}

function noticeShelf(immediate: boolean): void {
  window.clearTimeout(shelfTimer);
  const wait = immediate ? 0 : shelfNoticeMs - (Date.now() - shelfNoticeAt);
  const notify = () => { shelfNoticeAt = Date.now(); window.dispatchEvent(new CustomEvent(shelfChanged)); };
  if (wait <= 0) notify();
  else shelfTimer = window.setTimeout(notify, wait);
}

function flush(): Promise<void> {
  if (flushing) return flushing;
  flushing = (async () => {
    while (queued.size) {
      const batch = [...queued.values()];
      queued.clear();
      for (const reading of batch) {
        try { await invoke<void>("reading_local_put", { reading }); }
        catch {
          // 書けなかった記録は、次に書くときにもう一度送る。新しい記録が来ていればそちらを送る。
          if (!queued.has(workKey(reading))) queued.set(workKey(reading), reading);
          return;
        }
      }
    }
  })().finally(() => { flushing = null; });
  return flushing;
}

function store(row: LocalReading, shelfMoved: boolean, shelfNow = false): void {
  cache.set(workKey(row), row);
  if (!isTauri()) return;
  queued.set(workKey(row), row);
  void flush().then(() => { if (shelfMoved) noticeShelf(shelfNow); });
}

/** 書きかけの記録を送り終えるまで待つ。画面を閉じるときとアプリを離れるときに呼ぶ。 */
export function flushLocalReading(): Promise<void> {
  return queued.size || flushing ? flush() : Promise.resolve();
}

export function isLocalReadingLoaded(work: WorkIdentity): boolean {
  const key = localReadingKey(work);
  return cache.has(key) || unreadable.has(key) || !isTauri();
}

/** 作品を開く前に記録を読み込む。 */
export function loadLocalReading(work: WorkIdentity): Promise<void> {
  const key = localReadingKey(work);
  if (cache.has(key) || !isTauri()) return Promise.resolve();
  const inFlight = loading.get(key);
  if (inFlight) return inFlight;
  const task = (async () => {
    let row: LocalReading | null = null;
    try { row = await invoke<LocalReading | null>("reading_local_get", { key: identityOf(work) }); }
    catch { unreadable.add(key); return; }
    // 送りかけの記録があれば、そちらのほうが新しい。
    unreadable.delete(key);
    if (!cache.has(key)) cache.set(key, row ?? editable(work));
  })().finally(() => loading.delete(key));
  loading.set(key, task);
  return task;
}

export function localPage(work: WorkIdentity): number {
  return current(work)?.page ?? 0;
}

export function localPosition(work: WorkIdentity): SavedPosition | null {
  return current(work)?.position ?? null;
}

export function localReadingTime(work: WorkIdentity): number {
  return current(work)?.readAt ?? 0;
}

/** 詳細画面の「続きから読む」のため。開いただけのものは続きとして扱わない。 */
export function hasLocalPosition(work: WorkIdentity): boolean {
  const saved = localPosition(work);
  return saved !== null && (saved.page > 0 || (saved.ratio ?? 0) > 0.01);
}

export function saveLocalPage(work: WorkIdentity, page: number): void {
  const row = editable(work);
  if (row.page === page && cache.has(localReadingKey(work))) return;
  store({ ...row, page }, false);
}

export function saveLocalPosition(work: WorkIdentity, position: SavedPosition): void {
  store({ ...editable(work), page: position.page, position }, false);
}

/** 読み進めた時刻を残す。サーバーの位置と比べるときに使う。 */
export function markLocalReading(work: WorkIdentity, time: number): void {
  store({ ...editable(work), readAt: time }, true);
}

/** サーバーの位置で開いたとき。位置と時刻を一緒に書かないと、次に開いたとき古い位置を最新と扱う。 */
export function adoptLocalPosition(work: WorkIdentity, position: SavedPosition, time: number): void {
  store({ ...editable(work), page: position.page, position, readAt: time }, true);
}

/** 読書画面で 1% 以上と出る作品だけを棚に載せる（`readingPercent` と同じ境目）。 */
const partRead = (progress: number) => progress >= minimumReadProgress;

/**
 * 「続きを読む」の材料。サーバーの検索は「読みかけ」で絞れず、読書位置からは作品を引けないので、
 * この端末で読んだ作品を表示に要る情報ごと覚える。開いただけでは棚に並べない。
 */
export function rememberReading(work: PcWork, progress: number, finished: boolean): void {
  const row = editable(work);
  // 頭へ戻した作品は棚から降ろす。読み進めればまた並ぶ。
  const shelved = finished || partRead(progress);
  const joined = shelved !== row.shelved || finished !== row.finished;
  store({ ...row, progress: Math.min(1, Math.max(0, progress)), finished, shelved, work }, shelved || row.shelved, joined);
}

/** 「続きを読む」に出す棚と利用者を切り替える。 */
export function setReadingScope(scope: ReadingScope | null): void {
  if (sameReadingScope(scope, currentScope)) return;
  currentScope = scope ? { libraryId: scope.libraryId, subjectId: scope.subjectId } : null;
  lastShelves.clear();
  noticeShelf(true);
}

export function readingScope(): ReadingScope | null {
  return currentScope;
}

export const shelfChangedEvent = shelfChanged;

/** 前に読み込んだ棚。画面を開き直したとき、読み込みを待たずに同じ棚を出す。 */
export function cachedShelf(finished = false): RecentWork[] {
  return lastShelves.get(finished) ?? [];
}

/** 読み終えたものを除いた「続きを読む」。新しく読んだ順。 */
export async function loadShelf(finished = false): Promise<RecentWork[]> {
  const scope = currentScope;
  if (!scope || !isTauri()) return [];
  await flushLocalReading();
  if (scope !== currentScope) return [];
  try {
    const rows = await invoke<LocalReading[]>("reading_local_shelf", { scope, limit: finished ? 200 : shelfLimit, finished });
    if (scope !== currentScope) return [];
    for (const row of rows) if (!cache.has(workKey(row))) cache.set(workKey(row), row);
    const items = rows.filter((row) => row.work && row.finished === finished && (finished || partRead(row.progress))).map((row) => ({ work: row.work!, at: row.readAt, progress: row.progress, finished: row.finished }));
    lastShelves.set(finished, items);
    return items;
  } catch { return scope === currentScope ? cachedShelf(finished) : []; }
}
