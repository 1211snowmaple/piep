import { invoke, isTauri } from "@tauri-apps/api/core";
import type { PcWork } from "./pcApi";
import type { ReaderFont, ReaderLayout } from "./readerPrefs";
import { minimumReadProgress } from "./workText";

/**
 * 端末での読書の記録。Rust 側の SQLite（`reading_local_*`）に置く。
 *
 * 読書画面は位置を同期して読む必要があるので、開く前に `loadLocalReading` で
 * 記録を手元へ読み込み、そのあとは読み書きともにここで受け持つ。書き込みは
 * 順に Rust へ送り、送った時点で端末に確定する。以前の localStorage は書き込みが
 * 遅れ、アプリを閉じた直後に最後の位置が消えることがあった。
 */

// layout が違う位置は、割合だけを使って戻す。ページごとと一続きでは top の基準が違う。
export type SavedPosition = { page: number; top: number; ratio: number; fontSize: number; lineHeight: number; readerWidth: number; fontFamily: ReaderFont; layout?: ReaderLayout };

export interface LocalReading {
  source: string;
  sourceId: string;
  /** 位置が指す本文の版。 */
  version: number;
  contentHash: string | null;
  editHash: string | null;
  page: number;
  position: SavedPosition | null;
  /** 0 から 1。 */
  progress: number;
  finished: boolean;
  /** 「続きを読む」に並べるか。開いただけの作品は並べない。 */
  shelved: boolean;
  /** この端末で最後に読み進めた時刻。0 はまだ読み進めていない。 */
  readAt: number;
  /** 最後に読んだときの接続先。作品の番号はサーバーごとに振られるので、別のサーバーの棚に混ぜない。 */
  serverUrl: string | null;
  workId: number | null;
  work: PcWork | null;
}

export interface RecentWork {
  work: PcWork;
  at: number;
  progress: number;
  finished: boolean;
  server?: string;
}

export type WorkIdentity = Pick<PcWork, "id" | "source" | "sourceId" | "currentVersion" | "contentHash">;

const shelfChanged = "piep:recent-reading-changed";
const shelfLimit = 120;
const shelfNoticeMs = 4000;
const legacyRecentKey = "piep.android.reading.recent.v1";
const legacyPrefix = "piep.android.pc.";

const cache = new Map<string, LocalReading>();
const loading = new Map<string, Promise<void>>();
// 読み込めなかった作品。記録なしとして開き、画面を止めない。
const unreadable = new Set<string>();
const queued = new Map<string, LocalReading>();
let flushing: Promise<void> | null = null;
let currentServer: string | null = null;
let shelfNoticeAt = 0;
let shelfTimer: number | undefined;
const lastShelves = new Map<boolean, { server: string; items: RecentWork[] }>();

const keyOf = (source: string, sourceId: string) => `${source}:${sourceId}`;
const workKey = (work: Pick<PcWork, "source" | "sourceId">) => keyOf(work.source, work.sourceId);

function sameRevision(row: LocalReading, work: WorkIdentity, editHash: string | null): boolean {
  return row.version === work.currentVersion && (row.contentHash ?? null) === (work.contentHash ?? null) && (row.editHash ?? null) === (editHash ?? null);
}

/** 今の本文の記録。版が違う記録は、別の本文のものなので使わない。 */
function current(work: WorkIdentity, editHash: string | null): LocalReading | null {
  const row = cache.get(workKey(work));
  return row && sameRevision(row, work, editHash) ? row : null;
}

/** 書き換える行。版が変わっていたら、前の版の位置と時刻を引き継がない。 */
function editable(work: WorkIdentity, editHash: string | null): LocalReading {
  const row = cache.get(workKey(work));
  if (row && sameRevision(row, work, editHash)) return { ...row };
  return {
    source: work.source,
    sourceId: work.sourceId,
    version: work.currentVersion,
    contentHash: work.contentHash ?? null,
    editHash,
    page: 0,
    position: null,
    progress: row?.progress ?? 0,
    finished: row?.finished ?? false,
    shelved: row?.shelved ?? false,
    readAt: 0,
    serverUrl: row?.serverUrl ?? null,
    workId: row?.workId ?? null,
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

export function isLocalReadingLoaded(work: Pick<PcWork, "source" | "sourceId">): boolean {
  return cache.has(workKey(work)) || unreadable.has(workKey(work)) || !isTauri();
}

/** 作品を開く前に記録を読み込む。localStorage に以前の記録が残っていれば、ここで移す。 */
export function loadLocalReading(work: WorkIdentity): Promise<void> {
  const key = workKey(work);
  if (cache.has(key) || !isTauri()) return Promise.resolve();
  const inFlight = loading.get(key);
  if (inFlight) return inFlight;
  const task = (async () => {
    let row: LocalReading | null = null;
    try { row = await invoke<LocalReading | null>("reading_local_get", { source: work.source, sourceId: work.sourceId }); }
    catch { unreadable.add(key); return; }
    if (!row) {
      const legacy = legacyPosition(work);
      if (legacy) {
        try {
          await invoke<number>("reading_local_import", { readings: [legacy.row] });
          row = legacy.row;
          forgetLegacy(legacy.keys);
        } catch { /* 移せなければ、次に開いたときにもう一度試す */ }
      }
    }
    // 送りかけの記録があれば、そちらのほうが新しい。
    unreadable.delete(key);
    if (!cache.has(key)) cache.set(key, row ?? editable(work, null));
  })().finally(() => loading.delete(key));
  loading.set(key, task);
  return task;
}

export function localPage(work: WorkIdentity, editHash: string | null): number {
  return current(work, editHash)?.page ?? 0;
}

export function localPosition(work: WorkIdentity, editHash: string | null): SavedPosition | null {
  return current(work, editHash)?.position ?? null;
}

export function localReadingTime(work: WorkIdentity, editHash: string | null): number {
  return current(work, editHash)?.readAt ?? 0;
}

/** 詳細画面の「続きから読む」のため。開いただけのものは続きとして扱わない。 */
export function hasLocalPosition(work: WorkIdentity, editHash: string | null): boolean {
  const saved = localPosition(work, editHash);
  return saved !== null && (saved.page > 0 || (saved.ratio ?? 0) > 0.01);
}

export function saveLocalPage(work: WorkIdentity, editHash: string | null, page: number): void {
  const row = editable(work, editHash);
  if (row.page === page && cache.has(workKey(work))) return;
  store({ ...row, page }, false);
}

export function saveLocalPosition(work: WorkIdentity, editHash: string | null, position: SavedPosition): void {
  store({ ...editable(work, editHash), page: position.page, position }, false);
}

/** 読み進めた時刻を残す。サーバーの位置と比べるときに使う。 */
export function markLocalReading(work: WorkIdentity, editHash: string | null, time: number): void {
  store({ ...editable(work, editHash), readAt: time }, true);
}

/** サーバーの位置で開いたとき。位置と時刻を一緒に書かないと、次に開いたとき古い位置を最新と扱う。 */
export function adoptLocalPosition(work: WorkIdentity, editHash: string | null, position: SavedPosition, time: number): void {
  store({ ...editable(work, editHash), page: position.page, position, readAt: time }, true);
}

/** 読書画面で 1% 以上と出る作品だけを棚に載せる（`readingPercent` と同じ境目）。 */
const partRead = (progress: number) => progress >= minimumReadProgress;

/**
 * 「続きを読む」の材料。サーバーの検索は「読みかけ」で絞れず、読書位置からは作品を引けないので、
 * この端末で読んだ作品を表示に要る情報ごと覚える。開いただけでは棚に並べない。
 */
export function rememberReading(work: PcWork, editHash: string | null, progress: number, finished: boolean): void {
  const row = editable(work, editHash);
  // 頭へ戻した作品は棚から降ろす。読み進めればまた並ぶ。
  const shelved = finished || partRead(progress);
  const joined = shelved !== row.shelved || finished !== row.finished;
  store({ ...row, progress: Math.min(1, Math.max(0, progress)), finished, shelved, serverUrl: currentServer ?? row.serverUrl, workId: work.id, work }, shelved || row.shelved, joined);
}

/** 接続先が変わったら呼ぶ。接続先を記録していない古い履歴は、最初に分かった接続先のものとして扱う。 */
export function setReadingServer(server: string | null): void {
  if (server === currentServer) return;
  currentServer = server;
  if (server) void migrateLegacyShelf(server).finally(() => noticeShelf(true));
  else noticeShelf(true);
}

export function readingServer(): string | null {
  return currentServer;
}

export const shelfChangedEvent = shelfChanged;

/** 前に読み込んだ棚。画面を開き直したとき、読み込みを待たずに同じ棚を出す。 */
export function cachedShelf(finished = false): RecentWork[] {
  const lastShelf = lastShelves.get(finished);
  return lastShelf && lastShelf.server === currentServer ? lastShelf.items : [];
}

/** 読み終えたものを除いた「続きを読む」。新しく読んだ順。 */
export async function loadShelf(finished = false): Promise<RecentWork[]> {
  const server = currentServer;
  if (!server || !isTauri()) return [];
  await flushLocalReading();
  try {
    const rows = await invoke<LocalReading[]>("reading_local_shelf", { serverUrl: server, limit: finished ? 200 : shelfLimit, finished });
    for (const row of rows) if (!queued.has(workKey(row))) cache.set(workKey(row), row);
    // 0.6 までは少しでも動かせば棚に載せたので、頭にいる作品が記録に残っている。
    const items = rows.filter((row) => row.work && row.finished === finished && (finished || partRead(row.progress))).map((row) => ({ work: row.work!, at: row.readAt, progress: row.progress, finished: row.finished, server: row.serverUrl ?? undefined }));
    lastShelves.set(finished, { server, items });
    return items;
  } catch { return cachedShelf(finished); }
}

// ---- 0.5 までの localStorage からの移行 ----

function readStorage(key: string): string | null {
  try { return window.localStorage.getItem(key); } catch { return null; }
}

function legacyKeys(prefix: string): string[] {
  const keys: string[] = [];
  try {
    for (let index = 0; index < window.localStorage.length; index += 1) {
      const key = window.localStorage.key(index);
      if (key?.startsWith(prefix)) keys.push(key);
    }
  } catch { /* storage unavailable */ }
  return keys;
}

function forgetLegacy(keys: string[]): void {
  for (const key of keys) {
    try { window.localStorage.removeItem(key); } catch { /* storage unavailable */ }
  }
}

function parsePosition(raw: string | null): SavedPosition | null {
  if (raw === null) return null;
  try {
    const value = JSON.parse(raw) as Partial<SavedPosition> | null;
    if (!value || !Number.isSafeInteger(value.page) || !Number.isFinite(value.top) || !Number.isFinite(value.ratio)) return null;
    if (value.page! < 0 || value.top! < 0 || value.ratio! < 0 || value.ratio! > 1) return null;
    return value as SavedPosition;
  } catch { return null; }
}

/**
 * 以前の記録は `piep.android.pc.<作品番号>.<版>.<本文ハッシュ>.<編集ハッシュ>.<page|position|at>`。
 * 作品番号だけでは別のサーバーの作品と区別できないので、版と本文ハッシュまで同じものだけを移す。
 */
function legacyPosition(work: WorkIdentity): { row: LocalReading; keys: string[] } | null {
  const prefix = `${legacyPrefix}${work.id}.${work.currentVersion}.${work.contentHash ?? ""}.`;
  const keys = legacyKeys(prefix);
  let best: { editHash: string | null; page: number; position: SavedPosition | null; at: number } | null = null;
  const variants = new Set(keys.map((key) => key.slice(prefix.length).replace(/\.(page|position|at)$/, "")));
  for (const variant of variants) {
    const base = `${prefix}${variant}`;
    const page = Number(readStorage(`${base}.page`) ?? "0");
    const at = Number(readStorage(`${base}.at`) ?? "0");
    const candidate = {
      editHash: variant === "original" ? null : variant,
      page: Number.isSafeInteger(page) && page >= 0 && page <= 1_000_000 ? page : 0,
      position: parsePosition(readStorage(`${base}.position`)),
      at: Number.isSafeInteger(at) && at > 0 ? at : 0,
    };
    if (variant !== "unknown" && (!best || candidate.at > best.at)) best = candidate;
  }
  if (!best || (best.page === 0 && !best.position)) return null;
  return {
    keys,
    row: {
      source: work.source,
      sourceId: work.sourceId,
      version: work.currentVersion,
      contentHash: work.contentHash ?? null,
      editHash: best.editHash,
      page: best.page,
      position: best.position,
      progress: 0,
      finished: false,
      shelved: false,
      readAt: best.at,
      serverUrl: null,
      workId: null,
      work: null,
    },
  };
}

/** 以前の「続きを読む」を、読書位置と一緒に一度で移す。移し終えたら localStorage から消す。 */
async function migrateLegacyShelf(server: string): Promise<void> {
  const raw = readStorage(legacyRecentKey);
  if (raw === null || !isTauri()) return;
  let entries: RecentWork[] = [];
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (Array.isArray(parsed)) entries = parsed.filter((entry): entry is RecentWork => Boolean(entry?.work?.id && entry.work.source && entry.work.sourceId) && Number.isFinite(entry.at) && Number.isFinite(entry.progress));
  } catch { /* 壊れた記録は移さない */ }
  const consumed: string[] = [];
  const rows = new Map<string, LocalReading>();
  for (const entry of entries) {
    const key = workKey(entry.work);
    if (rows.has(key)) continue; // 新しい順に並んでいる
    const legacy = legacyPosition(entry.work);
    if (legacy) consumed.push(...legacy.keys);
    const base = legacy?.row ?? editable(entry.work, null);
    rows.set(key, {
      ...base,
      progress: Math.min(1, Math.max(0, entry.progress)),
      finished: Boolean(entry.finished),
      shelved: Boolean(entry.finished) || partRead(entry.progress),
      readAt: Math.max(base.readAt, Math.round(entry.at)),
      serverUrl: entry.server ?? server,
      workId: entry.work.id,
      work: entry.work,
    });
  }
  try { await invoke<number>("reading_local_import", { readings: [...rows.values()] }); }
  catch { return; } // 次に起動したときにもう一度試す
  forgetLegacy([legacyRecentKey, ...consumed]);
}
