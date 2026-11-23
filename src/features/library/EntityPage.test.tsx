import { MantineProvider } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import EntityPage from "@/features/library/EntityPage";
import type { ShelfEntityHead, ShelfSeriesFacet } from "@/services/shelfApi";
import type { FacetCount } from "@/types/library";

const PERSON = "0195f1c2-0000-7000-8000-0000000000a1";
const SERIES = "0195f1c2-0000-7000-8000-0000000000b1";

const api = vi.hoisted(() => ({
  shelfEntity: vi.fn(),
  shelfEntityTags: vi.fn(),
  shelfPersonSeries: vi.fn(),
  shelfEntityOriginal: vi.fn(),
  shelfEntityCollections: vi.fn(),
}));
const shelf = vi.hoisted(() => ({ searchShelf: vi.fn() }));
const dialogs = vi.hoisted(() => ({ openSingleDialog: vi.fn() }));

vi.mock("@/services/shelfApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/shelfApi")>()),
  ...api,
}));
vi.mock("@/features/library/shelf", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/features/library/shelf")>()),
  searchShelf: shelf.searchShelf,
}));
vi.mock("@/services/dialogApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dialogApi")>()),
  openSingleDialog: dialogs.openSingleDialog,
}));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getUpdateTarget: vi.fn().mockResolvedValue(null),
  refreshEntityProfile: vi.fn(),
  upsertUpdateTarget: vi.fn(),
}));

function head(overrides: Partial<ShelfEntityHead> = {}): ShelfEntityHead {
  return {
    id: PERSON,
    kind: "person",
    provider: "pixiv",
    external_key: "aoba",
    name: "青葉しおり",
    description: null,
    links: [],
    icon: null,
    banner: null,
    cover: null,
    is_concluded: null,
    published_content_count: null,
    work_count: 3,
    snapshot: null,
    version_number: 1,
    last_checked_at: null,
    last_fetched_at: null,
    versions: [],
    ...overrides,
  };
}

function series(overrides: Partial<ShelfSeriesFacet> = {}): ShelfSeriesFacet {
  return {
    id: SERIES,
    provider: "pixiv",
    external_key: "s1",
    name: "季節の栞",
    work_count: 2,
    cover: null,
    description: null,
    is_concluded: null,
    sample_title: null,
    latest_saved_at: 0,
    latest_published_at: 0,
    latest_updated_at: 0,
    total_text_length: 0,
    total_size: 0,
    total_media: 0,
    total_revisions: 0,
    ...overrides,
  };
}

const tags: FacetCount[] = [
  { name: "ファンタジー", count: 12 },
  { name: "長編", count: 7 },
  { name: "短編", count: 2 },
];

const emptyWorks = {
  items: [], nextCursor: null, totalEstimate: 0,
  searchMeta: { engine: "shelf", query: null, totalEstimate: 0, indexComplete: true, explanations: [] },
  facetsVersion: 0,
};

function renderEntity(kind: "person" | "series", hash: string) {
  window.location.hash = hash;
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MantineProvider>
      <QueryClientProvider client={client}>
        <AppRouter><WorkspaceProvider><div><EntityPage kind={kind} /></div></WorkspaceProvider></AppRouter>
      </QueryClientProvider>
    </MantineProvider>,
  );
}

const renderAuthor = (hash = "#/people/pixiv/aoba") => renderEntity("person", hash);

beforeAll(() => {
  Element.prototype.scrollIntoView = () => {};
});

beforeEach(() => {
  window.localStorage.clear();
  vi.clearAllMocks();
  api.shelfEntity.mockResolvedValue(head());
  api.shelfEntityTags.mockResolvedValue(tags);
  api.shelfPersonSeries.mockResolvedValue({ items: [series()], next: null, total: 1 });
  api.shelfEntityOriginal.mockResolvedValue(null);
  api.shelfEntityCollections.mockResolvedValue([]);
  shelf.searchShelf.mockResolvedValue(emptyWorks);
  dialogs.openSingleDialog.mockReset();
});

describe("author page", () => {
  it("does not truncate the primary entity title on its own detail page", async () => {
    api.shelfEntity.mockResolvedValue(head({ name: "催眠アプリに翻弄されるLOビアンたち" }));
    renderAuthor();

    const heading = await screen.findByRole("heading", {
      name: "催眠アプリに翻弄されるLOビアンたち",
    });
    expect(heading).toHaveClass("entity-hero__title");
    expect(heading).not.toHaveClass("line-clamp-2");
    const actions = screen.getByRole("group", { name: "催眠アプリに翻弄されるLOビアンたちの操作" });
    expect(actions.closest(".entity-hero__primary")).not.toBeNull();
    expect(actions.closest(".entity-hero__identity-copy")).toBeNull();
    expect(actions.compareDocumentPosition(heading) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  /**
   * シリーズの件数は、タブを開くまで取りに行っていなかった。数字はその
   * 問い合わせの `total` から出しているので、**開いてから遅れて現れる**。
   * 見出しに出す数字は、見出しが出た時点で揃っていること。
   */
  it("puts the series count on the tab without waiting for the tab to be opened", async () => {
    renderAuthor();
    const seriesTab = await screen.findByRole("tab", { name: /シリーズ/ });
    await waitFor(() => expect(within(seriesTab).getByText("1")).toBeInTheDocument());
  });

  it("shows the tags this author uses, with counts", async () => {
    renderAuthor();
    expect(await screen.findByText("ファンタジー")).toBeInTheDocument();
    expect(screen.getByText("長編")).toBeInTheDocument();
    expect(screen.getByText("12")).toBeInTheDocument();
    expect(api.shelfEntityTags).toHaveBeenCalledWith("person", PERSON);
  });

  it("narrows the works to a tag without leaving the author", async () => {
    renderAuthor();
    fireEvent.click(await screen.findByText("ファンタジー"));

    // The tag becomes part of the address, so it survives the back button and
    // a filtered view of one author can be linked to.
    await waitFor(() => expect(window.location.hash).toContain("tag=%E3%83%95%E3%82%A1%E3%83%B3%E3%82%BF%E3%82%B8%E3%83%BC"));
    await waitFor(() => expect(shelf.searchShelf).toHaveBeenCalledWith(
      expect.objectContaining({ tagsInclude: ["ファンタジー"] }),
      { person: PERSON },
    ));
  });

  it("uses the complete work ordering vocabulary with both directions", async () => {
    renderAuthor();
    const sort = await screen.findByRole("combobox", { name: "並び順" });
    expect(sort).toHaveAttribute("readonly");
    fireEvent.click(sort);
    expect(await screen.findByRole("option", { name: "容量：小さい順", hidden: true })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "版番号：大きい順", hidden: true })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: "版番号：小さい順", hidden: true }));

    await waitFor(() => expect(window.location.hash).toContain("sort=version"));
    expect(window.location.hash).toContain("order=asc");
    await waitFor(() => expect(shelf.searchShelf).toHaveBeenCalledWith(
      expect.objectContaining({ sortBy: "version", sortOrder: "asc" }),
      { person: PERSON },
    ));
  });

  it("defaults an author search to relevance just like the main library", async () => {
    renderAuthor("#/people/pixiv/aoba?q=%E5%AD%A3%E7%AF%80");
    expect(await screen.findByRole("combobox", { name: "並び順" })).toHaveValue("関連度：高い順");
    await waitFor(() => expect(shelf.searchShelf).toHaveBeenCalledWith(
      expect.objectContaining({ text: "季節", sortBy: "relevance", sortOrder: "desc" }),
      { person: PERSON },
    ));
  });

  it("lists the author's series and links each one", async () => {
    renderAuthor();
    fireEvent.click(await screen.findByRole("tab", { name: /シリーズ/ }));
    await waitFor(() => expect(window.location.hash).toContain("tab=series"));
    expect(await screen.findByText("季節の栞")).toBeInTheDocument();
    await waitFor(() => expect(api.shelfPersonSeries).toHaveBeenCalledWith(PERSON, {
      query: null,
      sort: "work_count",
      order: "desc",
      offset: 0,
      limit: 60,
    }));
  });

  it("sorts all of an author's series on the shelf in either direction", async () => {
    renderAuthor("#/people/pixiv/aoba?tab=series");
    const sort = await screen.findByRole("combobox", { name: "作者のシリーズの並び順" });
    fireEvent.click(sort);
    expect(await screen.findByRole("option", { name: "合計容量：大きい順", hidden: true })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: "合計容量：小さい順", hidden: true }));

    await waitFor(() => expect(window.location.hash).toContain("series_sort=size"));
    expect(window.location.hash).toContain("series_order=asc");
    await waitFor(() => expect(api.shelfPersonSeries).toHaveBeenCalledWith(PERSON, expect.objectContaining({
      sort: "size",
      order: "asc",
    })));
  });

  it("continues an author's series from where the last page ended instead of stopping at 200", async () => {
    const firstItems = Array.from({ length: 60 }, (_, index) => series({
      id: `0195f1c2-0000-7000-8000-${String(index).padStart(12, "0")}`,
      external_key: `s${index}`,
      name: `保存シリーズ ${index + 1}`,
    }));
    api.shelfPersonSeries.mockImplementation(async (_id: string, params: { offset: number }) => params.offset
      ? { items: [series({ external_key: "s60", name: "続きのシリーズ" })], next: null, total: 61 }
      : { items: firstItems, next: 60, total: 61 });
    renderAuthor();
    fireEvent.click(await screen.findByRole("tab", { name: /シリーズ/ }));

    expect(await screen.findByText("保存シリーズ 1")).toBeInTheDocument();
    expect(screen.queryByText(/最大200件/)).toBeNull();
    // Production may already prefetch when the pager sentinel enters the
    // viewport; isolated DOM tests normally expose the explicit fallback.
    const loadMore = screen.queryByRole("button", { name: /さらに読み込む/ });
    if (loadMore) fireEvent.click(loadMore);
    expect(await screen.findByText("続きのシリーズ")).toBeInTheDocument();
    expect(screen.getByText("61 / 61件を表示中")).toBeInTheDocument();
    expect(api.shelfPersonSeries).toHaveBeenLastCalledWith(PERSON, {
      query: null,
      sort: "work_count",
      order: "desc",
      offset: 60,
      limit: 60,
    });
  });

  it("searches all saved series on the shelf, keeps the query in the URL, and names its controls", async () => {
    api.shelfPersonSeries.mockImplementation(async (_id: string, params: { query: string | null }) => params.query === "目当て"
      ? { items: [series({ external_key: "wanted", name: "目当ての長編" })], next: null, total: 1 }
      : { items: [series()], next: null, total: 1 });
    renderAuthor("#/people/pixiv/aoba?tab=series");

    const search = await screen.findByRole("textbox", { name: "この作者のシリーズを検索" });
    expect(screen.getByText("保存済みシリーズ全体を名前・説明から検索します")).toBeInTheDocument();
    fireEvent.change(search, { target: { value: "目当て" } });
    await waitFor(() => expect(window.location.hash).toContain("series_q=%E7%9B%AE%E5%BD%93%E3%81%A6"), { timeout: 1500 });
    await waitFor(() => expect(api.shelfPersonSeries).toHaveBeenCalledWith(PERSON, expect.objectContaining({ query: "目当て", offset: 0 })));
    expect(await screen.findByText("目当ての長編")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "シリーズの検索語を消す" })).toHaveAccessibleName();
  });

  it("keeps loaded series visible when the next page cannot be read", async () => {
    api.shelfPersonSeries.mockImplementation(async (_id: string, params: { offset: number }) => {
      if (params.offset) throw new Error("棚が変わりました。先頭から読み直してください");
      return { items: [series()], next: 1, total: 2 };
    });
    renderAuthor("#/people/pixiv/aoba?tab=series");

    expect(await screen.findByText("季節の栞")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /さらに読み込む/ }));
    const alert = await screen.findByRole("alert", { name: "シリーズの続きを読み込めません" });
    expect(alert).toHaveTextContent("表示中のシリーズはそのまま残しています");
    expect(screen.getByText("季節の栞")).toBeInTheDocument();
    expect(within(alert).getByRole("button", { name: "もう一度試す" })).toBeInTheDocument();
    expect(within(alert).getByRole("button", { name: "先頭から読み直す" })).toBeInTheDocument();
  });

  it("keeps pending series search text when its sort changes before the debounce", async () => {
    renderAuthor("#/people/pixiv/aoba?tab=series");
    const search = await screen.findByRole("textbox", { name: "この作者のシリーズを検索" });
    fireEvent.change(search, { target: { value: "目当て" } });
    const sort = screen.getByRole("combobox", { name: "作者のシリーズの並び順" });
    fireEvent.click(sort);
    fireEvent.click(await screen.findByRole("option", { name: "合計容量：小さい順", hidden: true }));

    expect(search).toHaveValue("目当て");
    await waitFor(() => expect(window.location.hash).toContain("series_q=%E7%9B%AE%E5%BD%93%E3%81%A6"), { timeout: 1500 });
    expect(window.location.hash).toContain("series_sort=size");
  });

  it("reports when the archive folder dialog fails", async () => {
    dialogs.openSingleDialog.mockRejectedValueOnce(new Error("folder dialog unavailable"));
    const show = vi.spyOn(notifications, "show");
    try {
      renderAuthor();
      fireEvent.click(await screen.findByRole("button", { name: "アーカイブ" }));
      await waitFor(() => expect(show).toHaveBeenCalledWith(expect.objectContaining({
        color: "red",
        title: "書き出しに失敗しました",
        message: "folder dialog unavailable",
      })));
      expect(dialogs.openSingleDialog).toHaveBeenCalledOnce();
    } finally {
      show.mockRestore();
    }
  });

  it("searches and sorts within the author", async () => {
    renderAuthor();
    const search = await screen.findByLabelText("青葉しおりの作品を検索");
    fireEvent.change(search, { target: { value: "灯台" } });
    await waitFor(() => expect(shelf.searchShelf).toHaveBeenCalledWith(
      // A text search inside an author ranks by relevance, as the library does.
      expect.objectContaining({ text: "灯台", sortBy: "relevance" }),
      { person: PERSON },
    ));
  });

  it("restores its narrowing from the address", async () => {
    renderAuthor("#/people/pixiv/aoba?q=%E7%81%AF%E5%8F%B0&tag=%E9%95%B7%E7%B7%A8&sort=title");
    await waitFor(() => expect(shelf.searchShelf).toHaveBeenCalledWith(
      expect.objectContaining({ text: "灯台", tagsInclude: ["長編"], sortBy: "title", sortOrder: "asc" }),
      { person: PERSON },
    ));
  });

  it("says so when a filter leaves nothing, and offers the way out", async () => {
    renderAuthor("#/people/pixiv/aoba?tag=%E7%9F%AD%E7%B7%A8");
    expect(await screen.findByText("条件に合う作品がありません")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "絞り込みを解除" }));
    await waitFor(() => expect(window.location.hash).not.toContain("tag="));
  });

  it("keeps working for an author with no series and no tags", async () => {
    api.shelfPersonSeries.mockResolvedValue({ items: [], next: null, total: 0 });
    api.shelfEntityTags.mockResolvedValue([]);
    renderAuthor();
    // No tag bar rather than an empty one, and the series tab still opens.
    expect(await screen.findByRole("tab", { name: /作品/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: /シリーズ/ }));
    expect(await screen.findByText("シリーズはありません")).toBeInTheDocument();
  });
});

describe("EntityPage route handling", () => {
  it("accepts a literal percent sign in an already-decoded source key", async () => {
    // The router decodes route params; decoding again crashes on a valid '%'.
    api.shelfEntity.mockResolvedValue(head({ external_key: "creator%key" }));
    api.shelfPersonSeries.mockResolvedValue({ items: [], next: null, total: 0 });
    api.shelfEntityTags.mockResolvedValue([]);
    renderAuthor("#/people/pixiv/creator%25key");

    expect(await screen.findByRole("heading", { level: 1 })).toBeInTheDocument();
    expect(api.shelfEntity).toHaveBeenCalledWith("person", "pixiv", "creator%key");
  });
});

describe("series page", () => {
  beforeEach(() => {
    api.shelfEntity.mockResolvedValue(head({ id: SERIES, kind: "series", external_key: "s1", name: "季節の栞", work_count: 2 }));
  });

  /**
   * コレクションへの入口は作者にしか無く、同じ画面で作りが割れていた。
   * 辿る経路が作者かシリーズかの違いしかない。
   */
  it("opens the collections this series belongs to, the same way an author does", async () => {
    api.shelfEntityCollections.mockResolvedValue([{
      id: "0195f1c2-0000-7000-8000-0000000000d1",
      name: "連作の棚",
      description: null,
      ordered: false,
      cover: null,
      cover_image: null,
      cover_mode: "mosaic",
      name_source: "manual",
      track: "manual",
      tiles: [],
      revision: 1,
      member_count: 2,
      available_count: 2,
      total_text_length: 1200,
      created_at: Date.UTC(2026, 7, 1),
      updated_at: Date.UTC(2026, 7, 1),
    }]);
    renderEntity("series", "#/series/pixiv/s1?tab=collections");

    const tab = await screen.findByRole("tab", { name: /コレクション/ });
    expect(within(tab).getByText("1")).toBeInTheDocument();
    expect(api.shelfEntityCollections).toHaveBeenCalledWith("series", SERIES);
    expect(await screen.findByText("連作の棚")).toBeInTheDocument();
    const sort = screen.getByRole("combobox", { name: "コレクションの並び順" });
    fireEvent.click(sort);
    fireEvent.click(await screen.findByRole("option", { name: "合計文字数：少ない順", hidden: true }));
    await waitFor(() => expect(window.location.hash).toContain("collection_sort=length"));
    expect(window.location.hash).toContain("collection_order=asc");
  });

  it("does not ask for author-only data", async () => {
    renderEntity("series", "#/series/pixiv/s1");
    await waitFor(() => expect(shelf.searchShelf).toHaveBeenCalledWith(expect.anything(), { series: SERIES }));
    // A series has no list of series, but it does have tags worth narrowing by.
    expect(api.shelfPersonSeries).not.toHaveBeenCalled();
    expect(api.shelfEntityTags).toHaveBeenCalledWith("series", SERIES);
    expect(within(document.body).queryByRole("tab", { name: /シリーズ/ })).toBeNull();
  });
});
