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
  top: number;
}

function keyFor(workId: number, version: number | null): string {
  return `${POSITION_PREFIX}${workId}.${version ?? "current"}`;
}

function normalizePosition(page: unknown, top: unknown): ReadingPosition | null {
  const normalizedPage = Number(page);
  const normalizedTop = Number(top);
  if (!Number.isSafeInteger(normalizedPage) || normalizedPage < 1) return null;
  if (!Number.isFinite(normalizedTop) || normalizedTop < 0) return null;
  return { page: normalizedPage, top: normalizedTop };
}

/** Reads both the current object schema and scroll-offset values from older builds. */
function parsePosition(raw: string | null): ReadingPosition | null {
  if (raw === null) return null;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed === "number" || typeof parsed === "string") return normalizePosition(1, parsed);
    if (parsed && typeof parsed === "object") {
      const value = parsed as { page?: unknown; top?: unknown };
      return normalizePosition(value.page, value.top);
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
export function readReadingPosition(workId: number, version: number | null): ReadingPosition | null {
  if (!Number.isSafeInteger(workId) || workId <= 0) return null;
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
export function writeReadingPosition(workId: number, version: number | null, position: ReadingPosition): void {
  if (!Number.isSafeInteger(workId) || workId <= 0) return;
  const normalized = normalizePosition(position.page, position.top);
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
    // piep.reader-position.{id}.{version}
    const id = Number.parseInt(key.slice(POSITION_PREFIX.length).split(".")[0] ?? "", 10);
    if (!Number.isSafeInteger(id) || id <= 0) continue;
    // A position of 0 is the top of the work, which is not "part-read".
    const position = parsePosition(store.getItem(key));
    if (!isPartRead(position)) continue;
    ids.add(id);
    if (ids.size >= MAX_SHELF_IDS) break;
  }
  return [...ids].sort((a, b) => a - b);
}

/** Drops the recorded position for a work, used when it leaves the library. */
export function forgetReadingPositions(workId: number): void {
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
