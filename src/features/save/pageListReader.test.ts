import { describe, expect, it, vi } from "vitest";
import { normalizeListReadOptions, readListPages, type ListPage, type ListReadDeps } from "./pageListReader";

const base = "https://www.pixiv.net/novel/bookmark_new.php";

function listPage(url: string, ids: string[], saved: string[] = []): ListPage {
  return {
    url,
    truncated: false,
    works: ids.map((sourceId) => ({ sourceId, title: sourceId, author: "", saved: saved.includes(sourceId) })),
  };
}

/** ページごとの中身を持つ、取得元の代わり。開いたページだけが読める。 */
function browser(pages: Record<string, ListPage>, overrides: Partial<ListReadDeps> = {}) {
  let url = base;
  const navigate = vi.fn(async (next: string) => { url = next; });
  const onPage = vi.fn();
  const waited: number[] = [];
  const deps: ListReadDeps = {
    read: async () => pages[url] ?? listPage(url, []),
    currentUrl: async () => url,
    navigate,
    wait: async (milliseconds) => { waited.push(milliseconds); },
    cancelled: () => false,
    onPage,
    ...overrides,
  };
  return { deps, navigate, onPage, waited, goTo: (next: string) => { url = next; } };
}

const options = { pages: 3, intervalSeconds: 3, stopAtSaved: true };

describe("一覧を順に読む", () => {
  it.each([false, true])("読み取りの応答待ちに止めたら、候補を追加しない（開いた直後: %s）", async (opened) => {
    let stop = false;
    let reads = 0;
    const { deps, onPage, navigate } = browser({}, {
      read: async () => {
        if (++reads === (opened ? 3 : 1)) stop = true;
        return listPage(base, ["1"]);
      },
      cancelled: () => stop,
    });
    const result = await readListPages({ ...options, pages: 1 }, deps, opened);
    expect(result).toMatchObject({ stop: "cancelled", pagesRead: 0 });
    expect(onPage).not.toHaveBeenCalled();
    expect(navigate).not.toHaveBeenCalled();
  });

  it("現在のURLの応答待ちに止めたら、次のページを開かない", async () => {
    let stop = false;
    const { deps, navigate } = browser({ [base]: listPage(base, ["1"]) }, {
      currentUrl: async () => { stop = true; return base; },
      cancelled: () => stop,
    });
    expect(await readListPages(options, deps)).toMatchObject({ stop: "cancelled", pagesRead: 1 });
    expect(navigate).not.toHaveBeenCalled();
  });
  it("ページ数が 1 なら次のページを開かない", async () => {
    const { deps, navigate, onPage } = browser({ [base]: listPage(base, ["1"]) });
    const result = await readListPages({ ...options, pages: 1 }, deps);
    expect(result).toMatchObject({ stop: "complete", pagesRead: 1, total: 1 });
    expect(navigate).not.toHaveBeenCalled();
    expect(onPage).toHaveBeenCalledTimes(1);
  });

  it("指定のページ数で止まり、間隔をあけて開く", async () => {
    const { deps, navigate, waited } = browser({
      [base]: listPage(base, ["1", "2"]),
      [`${base}?p=2`]: listPage(`${base}?p=2`, ["3"]),
      [`${base}?p=3`]: listPage(`${base}?p=3`, ["4"]),
      [`${base}?p=4`]: listPage(`${base}?p=4`, ["5"]),
    });
    const result = await readListPages(options, deps);
    expect(result).toMatchObject({ stop: "complete", pagesRead: 3, total: 4 });
    expect(navigate.mock.calls.map(([url]) => url)).toEqual([`${base}?p=2`, `${base}?p=3`]);
    expect(waited.reduce((sum, value) => sum + value, 0)).toBeGreaterThanOrEqual(6000);
  });

  it("棚にある作品だけのページで止める", async () => {
    const { deps, navigate } = browser({
      [base]: listPage(base, ["1", "2"], ["1"]),
      [`${base}?p=2`]: listPage(`${base}?p=2`, ["3", "4"], ["3", "4"]),
    });
    const result = await readListPages(options, deps);
    expect(result).toMatchObject({ stop: "saved", pagesRead: 2, total: 4, saved: 3 });
    expect(navigate).toHaveBeenCalledTimes(1);
  });

  it("止める条件を切れば、棚にある作品だけのページでも進む", async () => {
    const { deps } = browser({
      [base]: listPage(base, ["1"], ["1"]),
      [`${base}?p=2`]: listPage(`${base}?p=2`, ["2"], ["2"]),
      [`${base}?p=3`]: listPage(`${base}?p=3`, ["3"]),
    });
    const result = await readListPages({ ...options, stopAtSaved: false }, deps);
    expect(result).toMatchObject({ stop: "complete", pagesRead: 3 });
  });

  it("新しい作品が無くなったら止める", async () => {
    const { deps, navigate } = browser({
      [base]: listPage(base, ["1"]),
      [`${base}?p=2`]: listPage(`${base}?p=2`, ["1"]),
    });
    const result = await readListPages(options, deps);
    expect(result).toMatchObject({ stop: "end", pagesRead: 1, total: 1 });
    expect(navigate).toHaveBeenCalledTimes(1);
  });

  it("次のページが空なら止める", async () => {
    const { deps } = browser({ [base]: listPage(base, ["1"]) });
    const result = await readListPages(options, deps);
    expect(result).toMatchObject({ stop: "end", pagesRead: 1 });
  });

  it("待っているあいだに利用者が移ったら、連れ戻さない", async () => {
    let waits = 0;
    const ctx = browser({ [base]: listPage(base, ["1"]) }, {
      wait: async () => {
        waits += 1;
        if (waits === 1) ctx.goTo("https://www.pixiv.net/users/1");
      },
    });
    const result = await readListPages(options, ctx.deps);
    expect(result.stop).toBe("moved");
    expect(ctx.navigate).not.toHaveBeenCalled();
  });

  it("開いた先が別のページへ飛ばされたら止める", async () => {
    const ctx = browser({ [base]: listPage(base, ["1"]) }, {
      navigate: async () => { ctx.goTo("https://www.pixiv.net/"); },
    });
    const result = await readListPages(options, ctx.deps);
    expect(result).toMatchObject({ stop: "moved", pagesRead: 1 });
  });

  it("止められたら次のページを開かない", async () => {
    let stop = false;
    const { deps, navigate } = browser({ [base]: listPage(base, ["1"]) }, {
      cancelled: () => stop,
      onPage: () => { stop = true; },
    });
    const result = await readListPages(options, deps);
    expect(result).toMatchObject({ stop: "cancelled", pagesRead: 1 });
    expect(navigate).not.toHaveBeenCalled();
  });

  it("続きの開き方が分からない一覧は 1 ページで終える", async () => {
    const top = "https://www.pixiv.net/";
    const ctx = browser({ [top]: listPage(top, ["1"]) });
    ctx.goTo(top);
    const result = await readListPages(options, ctx.deps);
    expect(result).toMatchObject({ stop: "complete", pagesRead: 1 });
    expect(ctx.navigate).not.toHaveBeenCalled();
  });

  it("描き終わるまで待ってから読む", async () => {
    const next = `${base}?p=2`;
    let reads = 0;
    const ctx = browser({ [base]: listPage(base, ["1"]) }, {
      read: async () => {
        reads += 1;
        // 開いた直後は空で、次に一部だけ、そのあと全部が出る。
        if (reads === 1) return listPage(base, ["1"]);
        if (reads === 2) return listPage(next, []);
        if (reads === 3) return listPage(next, ["2"]);
        return listPage(next, ["2", "3"]);
      },
    });
    const pages: string[][] = [];
    ctx.deps.onPage = (page) => pages.push(page.works.map((work) => work.sourceId));
    await readListPages({ ...options, pages: 2 }, ctx.deps);
    expect(pages).toEqual([["1"], ["2", "3"]]);
  });
});

describe("開いたばかりの一覧を読む", () => {
  it("描き終わるのを待ってから読む", async () => {
    let reads = 0;
    const ctx = browser({}, {
      read: async () => {
        reads += 1;
        if (reads === 1) throw new Error("内蔵ブラウザが開いていません");
        return listPage(base, reads === 2 ? [] : ["1", "2"]);
      },
    });
    const result = await readListPages({ ...options, pages: 1 }, ctx.deps, true);
    expect(result).toMatchObject({ stop: "complete", pagesRead: 1, total: 2 });
  });

  it("ログインの画面やトップに飛ばされたら何も読まない", async () => {
    const top = "https://www.pixiv.net/?return_to=%2Fnovel%2Fbookmark_new.php";
    const ctx = browser({}, { read: async () => listPage(top, ["1"]) });
    const result = await readListPages({ ...options, pages: 3 }, ctx.deps, true);
    expect(result).toMatchObject({ stop: "moved", pagesRead: 0, total: 0 });
    expect(ctx.onPage).not.toHaveBeenCalled();
    expect(ctx.navigate).not.toHaveBeenCalled();
  });
});

describe("読み方の指定", () => {
  it("範囲の外や壊れた値を収める", () => {
    expect(normalizeListReadOptions({ pages: 0, intervalSeconds: 1, stopAtSaved: false })).toEqual({ pages: 1, intervalSeconds: 3, stopAtSaved: false });
    expect(normalizeListReadOptions({ pages: 1000, intervalSeconds: 999 })).toEqual({ pages: 100, intervalSeconds: 60, stopAtSaved: true });
    expect(normalizeListReadOptions(null)).toEqual({ pages: 1, intervalSeconds: 5, stopAtSaved: true });
    expect(normalizeListReadOptions({ pages: Number.NaN })).toMatchObject({ pages: 1 });
  });
});
