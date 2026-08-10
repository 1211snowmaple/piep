/**
 * Which works have been opened and left part-read.
 *
 * The reader records where it got to under `piep.reader-position.{id}.{version}`
 * in per-device storage, so the database has no idea which works are part-read.
 * That makes the client the only source for this shelf: it collects the ids and
 * asks the library to resolve them.
 */
const POSITION_PREFIX = "piep.reader-position.";

/** Guards against a storage full of stale entries producing an enormous query. */
const MAX_SHELF_IDS = 2_000;

function storage(): Storage | null {
  try {
    return typeof window === "undefined" ? null : window.localStorage;
  } catch {
    return null;
  }
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
    const raw = store.getItem(key);
    if (raw === null) continue;
    const position = Number.parseFloat(raw.replace(/"/g, ""));
    if (Number.isFinite(position) && position <= 0) continue;
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
}
