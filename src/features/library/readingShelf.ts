import { SHELF_WORK_ID } from "@/services/shelfApi";

/**
 * Which works have been opened and left part-read.
 *
 * The reader records where it got to under `piep.reader-position.{id}.{version}`
 * in per-device storage, so the database has no idea which works are part-read.
 * That makes the client the only source for this shelf: it collects the ids and
 * asks the library to resolve them.
 */
const POSITION_PREFIX = "piep.reader-position.";
const POSITION_CHANGE_EVENT = "piep:reading-position-change";

/** Guards against a storage full of stale entries producing an enormous query. */
const MAX_SHELF_IDS = 2_000;

function storage(): Storage | null {
  try {
    return typeof window === "undefined" ? null : window.localStorage;
  } catch {
    return null;
  }
}

function legacySessionStorage(): Storage | null {
  try {
    return typeof window === "undefined" ? null : window.sessionStorage;
  } catch {
    return null;
  }
}

export interface ReadingPosition {
  page: number;
  /**
   * 読み始めからの距離。横書きなら下向き、縦書きなら右から左向きに測る。
   * どちらで測ったかは `mode` が持つ。
   */
  top: number;
  /**
   * 何行目にいたか。組み方に依らないので、文字サイズや行間を変えても同じ
   * ところへ戻れる。無いときは `top` に落とす（古い記録と、行の目印が
   * 一つも無い本文のため）。
   */
  anchor?: number;
  /**
   * `top` をどちらの組み方で測ったか。縦書きと横書きでは距離の意味が違うので、
   * 組み方を変えたあとの `top` は当てにしない。
   */
  mode?: "horizontal" | "vertical";
  /** 書いた時刻（ミリ秒）。棚にほかの端末が書いた位置と、どちらが新しいかを比べる。 */
  at?: number;
}

/**
 * 端末の記録を新しい棚の ID で引くようにしたか（棚を正本に切り替えたあと、一度だけ書き直す）。
 * 書き直したあとは旧棚の番号を使わない。
 */
const ADOPTED_KEY = "piep.reading-keys-adopted";

export function readingKeysAdopted(): boolean {
  try {
    return storage()?.getItem(ADOPTED_KEY) === "1";
  } catch {
    return false;
  }
}

/**
 * 読書位置を憶える作品。旧棚は番号、新しい棚（試験中）は UUID。旧棚から移した作品は
 * 新しい棚でも旧棚の番号で憶える（`readingKeyOf`）。UUID の鍵は読みかけの棚に数えない。
 */
export type ReadingWorkKey = number | string;

function validKey(workId: ReadingWorkKey): boolean {
  // 新しい棚の鍵は小文字にそろえたものだけ。大文字と小文字で同じ作品の位置が二つに割れない。
  return typeof workId === "number"
    ? Number.isSafeInteger(workId) && workId > 0
    : SHELF_WORK_ID.test(workId) && workId === workId.toLowerCase();
}

function keyFor(workId: ReadingWorkKey, version: number | null): string {
  return `${POSITION_PREFIX}${workId}.${version ?? "current"}`;
}

function normalizePosition(page: unknown, top: unknown, anchor?: unknown, mode?: unknown, at?: unknown): ReadingPosition | null {
  const normalizedPage = Number(page);
  const normalizedTop = Number(top);
  if (!Number.isSafeInteger(normalizedPage) || normalizedPage < 1) return null;
  if (!Number.isFinite(normalizedTop) || normalizedTop < 0) return null;
  const normalizedAnchor = Number(anchor);
  const position: ReadingPosition = { page: normalizedPage, top: normalizedTop };
  if (anchor !== undefined && Number.isSafeInteger(normalizedAnchor) && normalizedAnchor >= 0) {
    position.anchor = normalizedAnchor;
  }
  if (mode === "horizontal" || mode === "vertical") position.mode = mode;
  if (typeof at === "number" && Number.isSafeInteger(at) && at > 0) position.at = at;
  return position;
}

/** Reads both the current object schema and scroll-offset values from older builds. */
function parsePosition(raw: string | null): ReadingPosition | null {
  if (raw === null) return null;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed === "number" || typeof parsed === "string") return normalizePosition(1, parsed);
    if (parsed && typeof parsed === "object") {
      const value = parsed as { page?: unknown; top?: unknown; anchor?: unknown; mode?: unknown; at?: unknown };
      return normalizePosition(value.page, value.top, value.anchor, value.mode, value.at);
    }
  } catch {
    // The oldest build stored an unquoted numeric offset.
    return normalizePosition(1, raw);
  }
  return null;
}

function isPartRead(position: ReadingPosition | null): boolean {
  return Boolean(position && (position.page > 1 || position.top > 0));
}

function announcePositionChange(): void {
  try { window.dispatchEvent(new Event(POSITION_CHANGE_EVENT)); } catch { /* No browser event target. */ }
}

/**
 * Loads a reader position. A session-only value from the previous schema is
 * migrated once so an in-progress read is not lost during the upgrade.
 */
/**
 * 端末に作品ごとに残す記録（読書位置・しおり）を引く鍵。
 *
 * 旧棚が正本で、新しい棚がその写しのあいだは、新しい棚の作品でも旧棚から移したものは旧棚の
 * 番号で引く。両方の棚で同じ記録を読み書きするので、どちらで読んでも続きから読め、読みかけの
 * 棚にも載る。旧棚に無い作品だけが新しい棚の ID（小文字の UUID）で憶える。
 */
export function readingKeyOf(work: { id: ReadingWorkKey; legacyId?: number | null }): ReadingWorkKey {
  // 棚を正本に切り替えて端末の記録を書き直したあとは、新しい棚の ID だけで引く。
  if (typeof work.id === "string" && readingKeysAdopted()) return work.id;
  return work.legacyId ?? work.id;
}

export function readReadingPosition(workId: ReadingWorkKey, version: number | null): ReadingPosition | null {
  if (!validKey(workId)) return null;
  const key = keyFor(workId, version);
  const store = storage();
  try {
    const current = parsePosition(store?.getItem(key) ?? null);
    if (current) return current;
    const legacy = parsePosition(legacySessionStorage()?.getItem(key) ?? null);
    if (!legacy || !store) return legacy;
    store.setItem(key, JSON.stringify(legacy));
    if (isPartRead(legacy)) announcePositionChange();
    return legacy;
  } catch {
    return null;
  }
}

/** Writes the single schema consumed by both the reader and the shelf. */
export function writeReadingPosition(workId: ReadingWorkKey, version: number | null, position: ReadingPosition): void {
  if (!validKey(workId)) return;
  const normalized = normalizePosition(position.page, position.top, position.anchor, position.mode, Date.now());
  const store = storage();
  if (!normalized || !store) return;
  const key = keyFor(workId, version);
  try {
    const wasPartRead = isPartRead(parsePosition(store.getItem(key)));
    store.setItem(key, JSON.stringify(normalized));
    if (wasPartRead !== isPartRead(normalized)) announcePositionChange();
  } catch {
    // Reading must remain usable when storage is blocked or full.
  }
}

/** Notifies the persistent sidebar when a work enters or leaves the shelf. */
export function subscribeReadingPositions(listener: () => void): () => void {
  if (typeof window === "undefined") return () => undefined;
  window.addEventListener(POSITION_CHANGE_EVENT, listener);
  return () => window.removeEventListener(POSITION_CHANGE_EVENT, listener);
}

/**
 * Work ids with a recorded reading position, most recently written first is not
 * knowable from storage, so these come back in ascending id order for a stable
 * list. Entries whose work has since been deleted are filtered out by the
 * backend, not here - only it knows what still exists.
 */
export function readingWorkIds(): number[] {
  const store = storage();
  if (!store) return [];
  const ids = new Set<number>();
  for (let index = 0; index < store.length; index += 1) {
    const key = store.key(index);
    if (!key || !key.startsWith(POSITION_PREFIX)) continue;
    // piep.reader-position.{id}.{version}。数字だけの鍵が旧棚の作品。新しい棚の UUID を
    // `parseInt` すると頭の数字だけが読まれ、別の作品の番号になる。
    const segment = key.slice(POSITION_PREFIX.length).split(".")[0] ?? "";
    if (!/^\d+$/.test(segment)) continue;
    const id = Number(segment);
    if (!Number.isSafeInteger(id) || id <= 0) continue;
    // A position of 0 is the top of the work, which is not "part-read".
    const position = parsePosition(store.getItem(key));
    if (!isPartRead(position)) continue;
    ids.add(id);
    if (ids.size >= MAX_SHELF_IDS) break;
  }
  return [...ids].sort((a, b) => a - b);
}

/**
 * 読み終えた1つを、読みかけの棚から降ろす。
 *
 * 位置を消す口が「作品を消したとき」しか無かったので、最後まで読んだ本も
 * 棚に残り続けていた。棚は読みかけを並べる場所であって、履歴ではない。
 */
export function clearReadingPosition(workId: ReadingWorkKey, version: number | null): void {
  const store = storage();
  if (!store || !validKey(workId)) return;
  const key = keyFor(workId, version);
  try {
    const wasPartRead = isPartRead(parsePosition(store.getItem(key)));
    store.removeItem(key);
    if (wasPartRead) announcePositionChange();
  } catch {
    // 消せなくても読書は続く。
  }
}

/**
 * 棚を正本に切り替えたあと、端末に旧棚の番号で残した読書位置を一度だけ棚の ID へ書き直す。
 * `adopt` は読みかけの番号とページを棚へ渡し、番号に当たる作品の ID を返す（棚は位置の
 * ない作品にだけ、そのページの頭として書く）。棚に無い作品の記録は番号のまま残す。
 */
export async function adoptDeviceReadingKeys(
  adopt: (positions: [number, number][]) => Promise<[number, string][]>,
  rekeyBookmarks: (ids: Map<number, string>) => void,
): Promise<void> {
  const store = storage();
  if (!store || readingKeysAdopted()) return;
  const numbered: { key: string; id: number; rest: string }[] = [];
  const pages = new Map<number, number>();
  for (let index = 0; index < store.length; index += 1) {
    const key = store.key(index);
    if (!key || !key.startsWith(POSITION_PREFIX)) continue;
    const [segment = "", ...rest] = key.slice(POSITION_PREFIX.length).split(".");
    if (!/^\d+$/.test(segment)) continue;
    const id = Number(segment);
    numbered.push({ key, id, rest: rest.join(".") });
    // 棚へは今の本文の位置だけを渡す（ほかの版の位置は端末の記録のまま）。
    const position = parsePosition(store.getItem(key));
    if (rest.join(".") === "current" && isPartRead(position)) pages.set(id, position!.page);
  }
  const ids = new Map(await adopt([...pages.entries()]));
  for (const { key, id, rest } of numbered) {
    const work = ids.get(id);
    if (!work) continue;
    const value = store.getItem(key);
    const target = `${POSITION_PREFIX}${work}.${rest}`;
    if (value !== null && store.getItem(target) === null) store.setItem(target, value);
    store.removeItem(key);
  }
  rekeyBookmarks(ids);
  store.setItem(ADOPTED_KEY, "1");
  announcePositionChange();
}

/** Drops the recorded position for a work, used when it leaves the library. */
export function forgetReadingPositions(workId: ReadingWorkKey): void {
  const store = storage();
  if (!store) return;
  const prefix = `${POSITION_PREFIX}${workId}.`;
  const doomed: string[] = [];
  for (let index = 0; index < store.length; index += 1) {
    const key = store.key(index);
    if (key && key.startsWith(prefix)) doomed.push(key);
  }
  doomed.forEach((key) => store.removeItem(key));
  if (doomed.length) announcePositionChange();
}
