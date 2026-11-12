import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PcWork } from "./pcApi";

// 端末の SQLite の代わり。コマンドの名前と引数は Rust 側と同じ。
const db = vi.hoisted(() => ({ rows: new Map<string, Record<string, unknown>>(), failPut: false, calls: [] as string[] }));
vi.mock("@tauri-apps/api/core", () => ({
  isTauri: () => true,
  invoke: vi.fn(async (command: string, args: Record<string, unknown>) => {
    db.calls.push(command);
    const keyOf = (row: Record<string, unknown>) => `${row.source}:${row.sourceId}`;
    switch (command) {
      case "reading_local_get": return db.rows.get(`${args.source}:${args.sourceId}`) ?? null;
      case "reading_local_put": {
        if (db.failPut) throw { code: "device_records" };
        const row = args.reading as Record<string, unknown>;
        db.rows.set(keyOf(row), structuredClone(row));
        return null;
      }
      case "reading_local_import": {
        let count = 0;
        for (const row of args.readings as Record<string, unknown>[]) {
          const old = db.rows.get(keyOf(row));
          if (!old || (old.readAt as number) < (row.readAt as number)) { db.rows.set(keyOf(row), structuredClone(row)); count += 1; }
        }
        return count;
      }
      case "reading_local_shelf":
        return [...db.rows.values()]
          .filter((row) => row.serverUrl === args.serverUrl && row.shelved && row.finished === Boolean(args.finished))
          .sort((a, b) => (b.readAt as number) - (a.readAt as number));
      default: throw new Error(command);
    }
  }),
}));

const work: PcWork = {
  id: 7, source: "pixiv", sourceId: "100", title: "作品", authorName: "作者", authorId: "1",
  contentType: "novel", tags: [], excerpt: null, textLength: 1, assetCount: 0, hasCover: false, favorite: false, seriesTitle: null, seriesId: null,
  currentVersion: 2, contentHash: "abc",
} as PcWork;
const position = { page: 3, top: 40, ratio: 0.5, fontSize: 18, lineHeight: 1.8, readerWidth: 640, fontFamily: "serif" as const };

async function fresh() {
  vi.resetModules();
  return import("./localReading");
}

beforeEach(() => {
  db.rows.clear();
  db.failPut = false;
  db.calls.length = 0;
  window.localStorage.clear();
});
afterEach(() => vi.useRealTimers());

describe("端末の読書の記録", () => {
  it("読了は読みかけから読了棚へ移り、開き直しても残る", async () => {
    let store = await fresh();
    store.setReadingServer("https://pc.example/");
    await store.loadLocalReading(work);
    store.rememberReading(work, null, 0.5, false);
    expect(await store.loadShelf()).toHaveLength(1);
    store.rememberReading(work, null, 1, true);
    expect(await store.loadShelf()).toHaveLength(0);
    expect(await store.loadShelf(true)).toMatchObject([{ work, finished: true }]);
    store = await fresh();
    store.setReadingServer("https://pc.example/");
    expect(await store.loadShelf(true)).toHaveLength(1);
    store.setReadingServer("https://different.example/");
    expect(await store.loadShelf(true)).toHaveLength(0);
  });

  it("書いた位置は SQLite に残り、開き直しても同じ位置を返す", async () => {
    let store = await fresh();
    await store.loadLocalReading(work);
    store.adoptLocalPosition(work, null, position, 1_000);
    await store.flushLocalReading();
    expect(db.rows.get("pixiv:100")).toMatchObject({ page: 3, readAt: 1_000, editHash: null });

    store = await fresh();
    expect(store.isLocalReadingLoaded(work)).toBe(false);
    await store.loadLocalReading(work);
    expect(store.localPage(work, null)).toBe(3);
    expect(store.localPosition(work, null)).toEqual(position);
    expect(store.hasLocalPosition(work, null)).toBe(true);
  });

  it("版や編集が違う記録は使わず、書き直すときは位置と時刻を引き継がない", async () => {
    const store = await fresh();
    await store.loadLocalReading(work);
    store.adoptLocalPosition(work, null, position, 1_000);
    const revised = { ...work, currentVersion: 3 };
    expect(store.localPage(revised, null)).toBe(0);
    expect(store.localPage(work, "edit")).toBe(0);
    store.saveLocalPage(revised, null, 1);
    await store.flushLocalReading();
    expect(db.rows.get("pixiv:100")).toMatchObject({ version: 3, page: 1, position: null, readAt: 0 });
  });

  it("書けなかった記録は捨てず、次に書くときに送り直す", async () => {
    const store = await fresh();
    await store.loadLocalReading(work);
    db.failPut = true;
    store.saveLocalPosition(work, null, position);
    await store.flushLocalReading();
    expect(db.rows.has("pixiv:100")).toBe(false);
    db.failPut = false;
    await store.flushLocalReading();
    expect(db.rows.get("pixiv:100")).toMatchObject({ page: 3 });
  });

  it("以前の localStorage の位置は、開いたときに移して消す。版の分からない控えは移さない", async () => {
    const base = "piep.android.pc.7.2.abc.";
    window.localStorage.setItem(`${base}original.page`, "4");
    window.localStorage.setItem(`${base}original.position`, JSON.stringify({ ...position, page: 4 }));
    window.localStorage.setItem(`${base}original.at`, "500");
    window.localStorage.setItem(`${base}unknown.page`, "9");
    window.localStorage.setItem(`${base}unknown.at`, "900");
    window.localStorage.setItem("piep.android.pc.8.2.abc.original.page", "6");
    const store = await fresh();
    await store.loadLocalReading(work);
    expect(store.localPage(work, null)).toBe(4);
    expect(store.localReadingTime(work, null)).toBe(500);
    expect(window.localStorage.getItem(`${base}original.page`)).toBeNull();
    expect(window.localStorage.getItem(`${base}unknown.page`)).toBeNull();
    expect(window.localStorage.getItem("piep.android.pc.8.2.abc.original.page")).toBe("6");
  });

  it("読めなかったときも画面は開ける", async () => {
    db.rows.set("pixiv:100", { source: "pixiv", sourceId: "100", version: 2, contentHash: "abc", editHash: null, page: 3, position, progress: 0, finished: false, shelved: false, readAt: 1, serverUrl: null, workId: null, work: null });
    const store = await fresh();
    const core = await import("@tauri-apps/api/core");
    vi.mocked(core.invoke).mockRejectedValueOnce({ code: "device_records" });
    await store.loadLocalReading(work);
    expect(store.isLocalReadingLoaded(work)).toBe(true);
    expect(store.localPage(work, null)).toBe(0);
    // 次に開いたときは読み直す。
    await store.loadLocalReading(work);
    expect(store.localPage(work, null)).toBe(3);
  });
});

describe("続きを読む", () => {
  it("読み進めた作品だけを、接続先ごとに新しい順で並べる", async () => {
    const store = await fresh();
    store.setReadingServer("https://pc.example/");
    const other = { ...work, id: 8, sourceId: "200" } as PcWork;
    const opened = { ...work, id: 9, sourceId: "300" } as PcWork;
    for (const item of [work, other, opened]) await store.loadLocalReading(item);
    store.markLocalReading(work, null, 100);
    store.rememberReading(work, null, 0.4, false);
    store.markLocalReading(other, null, 200);
    store.rememberReading(other, null, 0.1, false);
    store.markLocalReading(opened, null, 300);
    store.rememberReading(opened, null, 0, false);
    const shelf = await store.loadShelf();
    expect(shelf.map((item) => item.work.sourceId)).toEqual(["200", "100"]);
    expect(store.cachedShelf()).toEqual(shelf);

    store.rememberReading(other, null, 1, true);
    expect((await store.loadShelf()).map((item) => item.work.sourceId)).toEqual(["100"]);
    // 頭へ戻すと棚から降り、読み進めればまた並ぶ。
    store.rememberReading(work, null, 0.002, false);
    expect(await store.loadShelf()).toEqual([]);
    store.rememberReading(work, null, 0.2, false);
    expect((await store.loadShelf()).map((item) => item.work.sourceId)).toEqual(["100"]);
    store.setReadingServer("https://other.example/");
    expect(await store.loadShelf()).toEqual([]);
  });

  it("以前の棚は最初に分かった接続先へ一度だけ移し、localStorage から消す", async () => {
    window.localStorage.setItem("piep.android.reading.recent.v1", JSON.stringify([
      { work, at: 700, progress: 0.3 },
      { work: { ...work, id: 8, sourceId: "200" }, at: 600, progress: 1, finished: true },
    ]));
    window.localStorage.setItem("piep.android.pc.7.2.abc.original.page", "5");
    const store = await fresh();
    store.setReadingServer("https://pc.example/");
    await vi.waitFor(() => expect(window.localStorage.getItem("piep.android.reading.recent.v1")).toBeNull());
    expect(window.localStorage.getItem("piep.android.pc.7.2.abc.original.page")).toBeNull();
    expect(db.rows.get("pixiv:100")).toMatchObject({ page: 5, readAt: 700, shelved: true, serverUrl: "https://pc.example/" });
    expect((await store.loadShelf()).map((item) => item.work.sourceId)).toEqual(["100"]);
  });
});
