import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PcWork, ReadingScope } from "./pcApi";
import type { LocalReading } from "./localReading";

// 端末の SQLite の代わり。コマンドの名前と引数は Rust 側と同じ。
const db = vi.hoisted(() => ({ rows: new Map<string, LocalReading>(), failPut: false, failKeys: new Set<string>(), calls: [] as string[] }));
const keyOf = ({ scope: { libraryId, subjectId }, workId }: LocalReading["key"]) => `${libraryId}/${subjectId}/${workId}`;
vi.mock("@tauri-apps/api/core", () => ({
  isTauri: () => true,
  invoke: vi.fn(async (command: string, args: Record<string, unknown>) => {
    db.calls.push(command);
    switch (command) {
      case "reading_local_get": return db.rows.get(keyOf(args.key as LocalReading["key"])) ?? null;
      case "reading_local_put": {
        const row = args.reading as LocalReading;
        if (db.failPut || db.failKeys.has(keyOf(row.key))) throw { code: "device_records" };
        db.rows.set(keyOf(row.key), structuredClone(row));
        return null;
      }
      case "reading_local_shelf": {
        const { libraryId, subjectId } = args.scope as ReadingScope;
        return [...db.rows.values()]
          .filter((row) => row.key.scope.libraryId === libraryId && row.key.scope.subjectId === subjectId && row.shelved && row.finished === Boolean(args.finished))
          .sort((a, b) => b.readAt - a.readAt);
      }
      default: throw new Error(command);
    }
  }),
}));

const scope = { libraryId: "l1", subjectId: "u1" };
const otherScope = { ...scope, libraryId: "l2" };
const otherReader = { ...scope, subjectId: "u2" };

const work: PcWork = {
  readingScope: scope,
  id: "w7", source: "pixiv", sourceId: "100", title: "作品", authorName: "作者", authorId: "1",
  contentType: "novel", tags: [], excerpt: null, textLength: 1, assetCount: 0, hasCover: false, favorite: false, seriesTitle: null, seriesId: null,
  currentVersion: 2, snapshotId: "s1", edited: false, editId: null, favoriteGeneration: "0", sourceCreatedAt: null, savedAt: "2026-11-01T00:00:00Z",
};
const workKey = keyOf({ scope, workId: work.id });
const position = { page: 3, top: 40, ratio: 0.5, fontSize: 18, lineHeight: 1.8, readerWidth: 640, fontFamily: "serif" as const };

async function fresh() {
  vi.resetModules();
  return import("./localReading");
}

beforeEach(() => {
  db.rows.clear();
  db.failPut = false;
  db.failKeys.clear();
  db.calls.length = 0;
  window.localStorage.clear();
});
afterEach(() => vi.useRealTimers());

describe("端末の読書の記録", () => {
  it("取得元 ID と版番号が同じ別の棚では位置を戻さず、互いの位置を上書きしない", async () => {
    let store = await fresh();
    const first = { ...work, currentVersion: 1 };
    const second = { ...first, id: "w8", snapshotId: "s2", readingScope: otherScope };
    const nextPosition = { ...position, page: 5 };
    store.setReadingScope(scope);
    await store.loadLocalReading(first);
    store.adoptLocalPosition(first, position, 1_000);
    store.setReadingScope(otherScope);
    await store.loadLocalReading(second);
    expect(store.localPosition(second)).toBeNull();
    store.adoptLocalPosition(second, nextPosition, 2_000);
    await store.flushLocalReading();

    store = await fresh();
    store.setReadingScope(scope);
    await store.loadLocalReading(first);
    expect(store.localPosition(first)).toEqual(position);
    store.setReadingScope(otherScope);
    await store.loadLocalReading(second);
    expect(store.localPosition(second)).toEqual(nextPosition);
  });

  it("オフラインで読んだ記録は保存した棚と利用者に属する", async () => {
    const store = await fresh();
    store.setReadingScope(otherScope);
    const anotherSubject = { ...work, readingScope: otherReader };
    await store.loadLocalReading(work);
    store.adoptLocalPosition(work, position, 1_000);
    store.rememberReading(work, 0.5, false);
    expect(await store.loadShelf()).toEqual([]);

    store.setReadingScope(otherReader);
    await store.loadLocalReading(anotherSubject);
    expect(store.localPosition(anotherSubject)).toBeNull();
    expect(await store.loadShelf()).toEqual([]);

    store.setReadingScope(scope);
    expect(await store.loadShelf()).toMatchObject([{ work, progress: 0.5 }]);
    expect(store.localPosition(work)).toEqual(position);
  });

  it("読了は読みかけから読了棚へ移り、開き直しても残る", async () => {
    let store = await fresh();
    store.setReadingScope(scope);
    await store.loadLocalReading(work);
    store.rememberReading(work, 0.5, false);
    expect(await store.loadShelf()).toHaveLength(1);
    store.rememberReading(work, 1, true);
    expect(await store.loadShelf()).toHaveLength(0);
    expect(await store.loadShelf(true)).toMatchObject([{ work, finished: true }]);
    store = await fresh();
    store.setReadingScope(scope);
    expect(await store.loadShelf(true)).toHaveLength(1);
    store.setReadingScope(otherScope);
    expect(await store.loadShelf(true)).toHaveLength(0);
  });

  it("書いた位置は SQLite に残り、開き直しても同じ位置を返す", async () => {
    let store = await fresh();
    await store.loadLocalReading(work);
    store.adoptLocalPosition(work, position, 1_000);
    await store.flushLocalReading();
    expect(db.rows.get(workKey)).toMatchObject({ page: 3, readAt: 1_000, editId: null });

    store = await fresh();
    expect(store.isLocalReadingLoaded(work)).toBe(false);
    await store.loadLocalReading(work);
    expect(store.localPage(work)).toBe(3);
    expect(store.localPosition(work)).toEqual(position);
    expect(store.hasLocalPosition(work)).toBe(true);
  });

  it("版番号が同じでも保存版の UUID が違えば位置を戻さない", async () => {
    const store = await fresh();
    await store.loadLocalReading(work);
    store.adoptLocalPosition(work, position, 1_000);
    expect(store.localPosition({ ...work, snapshotId: "s2" })).toBeNull();
    const renumbered = { ...work, currentVersion: 3 };
    expect(store.localPosition(renumbered)).toEqual(position);
  });

  it("版や編集が違う記録は使わず、書き直すときは位置と時刻を引き継がない", async () => {
    const store = await fresh();
    await store.loadLocalReading(work);
    store.adoptLocalPosition(work, position, 1_000);
    const revised = { ...work, currentVersion: 3, snapshotId: "s2" };
    expect(store.localPage(revised)).toBe(0);
    expect(store.localPage({ ...work, editId: "edit" })).toBe(0);
    store.saveLocalPage(revised, 1);
    await store.flushLocalReading();
    expect(db.rows.get(workKey)).toMatchObject({ snapshotId: "s2", page: 1, position: null, readAt: 0 });
  });

  it("書けなかった記録は捨てず、次に書くときに送り直す", async () => {
    const store = await fresh();
    await store.loadLocalReading(work);
    db.failPut = true;
    store.saveLocalPosition(work, position);
    await store.flushLocalReading();
    expect(db.rows.has(workKey)).toBe(false);
    db.failPut = false;
    await store.flushLocalReading();
    expect(db.rows.get(workKey)).toMatchObject({ page: 3 });
  });

  it("一件書けなくても、同じまとまりのほかの記録は捨てない", async () => {
    const store = await fresh();
    const [first, second, third] = ["100", "200", "300"].map((sourceId, i) => ({ ...work, id: `w${i}`, sourceId }) as PcWork);
    const key = (item: PcWork) => keyOf({ scope, workId: item.id });
    for (const item of [first, second, third]) await store.loadLocalReading(item);
    db.failKeys.add(key(second));
    // 一件目を送っているあいだに、二件目と三件目が同じまとまりに溜まる。
    store.saveLocalPosition(first, position);
    store.saveLocalPosition(second, position);
    store.saveLocalPosition(third, position);
    await store.flushLocalReading();
    expect(db.rows.has(key(first))).toBe(true);
    expect(db.rows.has(key(third))).toBe(true);
    expect(db.rows.has(key(second))).toBe(false);
    // 書けなかった一件は残っていて、次に送る。
    db.failKeys.clear();
    await store.flushLocalReading();
    expect(db.rows.get(key(second))).toMatchObject({ page: 3 });
  });

  it("読めなかったときも画面は開ける", async () => {
    db.rows.set(workKey, { key: { scope, workId: work.id }, snapshotId: work.snapshotId, editId: null, page: 3, position, progress: 0, finished: false, shelved: false, readAt: 1, work: null });
    const store = await fresh();
    const core = await import("@tauri-apps/api/core");
    vi.mocked(core.invoke).mockRejectedValueOnce({ code: "device_records" });
    await store.loadLocalReading(work);
    expect(store.isLocalReadingLoaded(work)).toBe(true);
    expect(store.localPage(work)).toBe(0);
    // 次に開いたときは読み直す。
    await store.loadLocalReading(work);
    expect(store.localPage(work)).toBe(3);
  });
});

describe("続きを読む", () => {
  it("読み進めた作品だけを、棚と利用者ごとに新しい順で並べる", async () => {
    const store = await fresh();
    store.setReadingScope(scope);
    const other = { ...work, id: "w8", sourceId: "200" } as PcWork;
    const opened = { ...work, id: "w9", sourceId: "300" } as PcWork;
    for (const item of [work, other, opened]) await store.loadLocalReading(item);
    store.markLocalReading(work, 100);
    store.rememberReading(work, 0.4, false);
    store.markLocalReading(other, 200);
    store.rememberReading(other, 0.1, false);
    store.markLocalReading(opened, 300);
    store.rememberReading(opened, 0, false);
    const shelf = await store.loadShelf();
    expect(shelf.map((item) => item.work.sourceId)).toEqual(["200", "100"]);
    expect(store.cachedShelf()).toEqual(shelf);

    store.rememberReading(other, 1, true);
    expect((await store.loadShelf()).map((item) => item.work.sourceId)).toEqual(["100"]);
    // 頭へ戻すと棚から降り、読み進めればまた並ぶ。
    store.rememberReading(work, 0.002, false);
    expect(await store.loadShelf()).toEqual([]);
    store.rememberReading(work, 0.2, false);
    expect((await store.loadShelf()).map((item) => item.work.sourceId)).toEqual(["100"]);
    store.setReadingScope(otherScope);
    expect(await store.loadShelf()).toEqual([]);
  });
});
