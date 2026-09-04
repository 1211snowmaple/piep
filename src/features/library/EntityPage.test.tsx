import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import EntityPage from "@/features/library/EntityPage";
import type { EntityFacet, FacetCount } from "@/types/library";

const shelfApi = vi.hoisted(() => ({
  ENTITY_SERIES_PAGE_SIZE: 60,
  listEntitySeriesPage: vi.fn(),
  listEntityTags: vi.fn(),
  getLibraryShelfCounts: vi.fn(),
  listSavedSearches: vi.fn(),
  upsertSavedSearch: vi.fn(),
  deleteSavedSearch: vi.fn(),
}));
const dbApi = vi.hoisted(() => ({ searchDownloadsV2: vi.fn(), getPerson: vi.fn(), getSeries: vi.fn() }));
const collectionApi = vi.hoisted(() => ({
  listCollectionsForPerson: vi.fn(),
  listCollectionsForSeries: vi.fn(),
}));

vi.mock("@/services/shelfApi", () => shelfApi);
vi.mock("@/services/collectionApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/collectionApi")>()),
  ...collectionApi,
}));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getPerson: dbApi.getPerson,
  getSeries: dbApi.getSeries,
  searchDownloadsV2: dbApi.searchDownloadsV2,
  listEntityVersions: vi.fn().mockResolvedValue([]),
  getLatestEntityProfileJson: vi.fn().mockResolvedValue({}),
  getUpdateTarget: vi.fn().mockResolvedValue(null),
  refreshEntityProfile: vi.fn(),
  upsertUpdateTarget: vi.fn(),
  getAssetUrl: () => null,
}));

const person = {
  id: 1, source: "pixiv", sourceKey: "aoba", displayName: "青葉しおり", iconPath: null, linksJson: null,
  coverPath: null, description: null, contentHash: null, currentVersion: 1,
  lastCheckedAt: null, lastFetchedAt: null, createdAt: "", updatedAt: "2026-08-01T00:00:00Z", workCount: 3,
};

function series(overrides: Partial<EntityFacet> = {}): EntityFacet {
  return {
    source: "pixiv", sourceKey: "s1", displayName: "季節の栞", count: 2,
    coverPath: null, description: null, updatedAt: null, latestDownloadedAt: null,
    sampleTitle: null, iconPath: null, bannerPath: null,
    ...overrides,
  } as EntityFacet;
}

const tags: FacetCount[] = [
  { name: "ファンタジー", count: 12 },
  { name: "長編", count: 7 },
  { name: "短編", count: 2 },
];

function renderAuthor(hash = "#/people/pixiv/aoba") {
  window.location.hash = hash;
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MantineProvider>
      <QueryClientProvider client={client}>
        <AppRouter><EntityPage kind="person" /></AppRouter>
      </QueryClientProvider>
    </MantineProvider>,
  );
}

describe("author page", () => {
  beforeEach(() => {
    window.localStorage.clear();
    dbApi.getPerson.mockResolvedValue(person);
    dbApi.searchDownloadsV2.mockResolvedValue({
      items: [], nextCursor: null, totalEstimate: 0,
      searchMeta: { engine: "sqlite-metadata", query: null, totalEstimate: 0, indexComplete: true, explanations: [] },
      facetsVersion: 0,
    });
    shelfApi.listEntitySeriesPage.mockResolvedValue({ items: [series()], nextCursor: null, total: 1 });
    shelfApi.listEntityTags.mockResolvedValue(tags);
    collectionApi.listCollectionsForPerson.mockResolvedValue([]);
    collectionApi.listCollectionsForSeries.mockResolvedValue([]);
  });

  it("does not truncate the primary entity title on its own detail page", async () => {
    dbApi.getPerson.mockResolvedValue({
      ...person,
      displayName: "催眠アプリに翻弄されるLOビアンたち",
    });
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
  });

  it("narrows the works to a tag without leaving the author", async () => {
    renderAuthor();
    fireEvent.click(await screen.findByText("ファンタジー"));

    // The tag becomes part of the address, so it survives the back button and
    // a filtered view of one author can be linked to.
    await waitFor(() => expect(window.location.hash).toContain("tag=%E3%83%95%E3%82%A1%E3%83%B3%E3%82%BF%E3%82%B8%E3%83%BC"));
    await waitFor(() => expect(dbApi.searchDownloadsV2).toHaveBeenCalledWith(
      expect.objectContaining({ personKey: "aoba", tagsInclude: ["ファンタジー"] }),
    ));
  });

  it("lists the author's series and links each one", async () => {
    renderAuthor();
    fireEvent.click(await screen.findByRole("tab", { name: /シリーズ/ }));
    const link = await screen.findByText("季節の栞");
    expect(link).toBeInTheDocument();
    await waitFor(() => expect(shelfApi.listEntitySeriesPage).toHaveBeenCalledWith("pixiv", "aoba", {
      query: null,
      limit: 60,
      cursor: null,
    }));
  });

  it("continues an author's series with the opaque cursor instead of stopping at 200", async () => {
    const firstItems = Array.from({ length: 60 }, (_, index) => series({
      sourceKey: `s${index}`,
      displayName: `保存シリーズ ${index + 1}`,
    }));
    shelfApi.listEntitySeriesPage.mockImplementation(async (_source, _key, params) => params.cursor
      ? { items: [series({ sourceKey: "s60", displayName: "続きのシリーズ" })], nextCursor: null, total: 61 }
      : { items: firstItems, nextCursor: "opaque-next", total: 61 });
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
    expect(shelfApi.listEntitySeriesPage).toHaveBeenLastCalledWith("pixiv", "aoba", {
      query: null,
      limit: 60,
      cursor: "opaque-next",
    });
  });

  it("searches all saved series on the server, keeps the query in the URL, and names its controls", async () => {
    shelfApi.listEntitySeriesPage.mockImplementation(async (_source, _key, params) => params.query === "目当て"
      ? { items: [series({ sourceKey: "wanted", displayName: "目当ての長編" })], nextCursor: null, total: 1 }
      : { items: [series()], nextCursor: null, total: 1 });
    renderAuthor("#/people/pixiv/aoba?tab=series");

    const search = await screen.findByRole("textbox", { name: "この作者のシリーズを検索" });
    expect(screen.getByText("保存済みシリーズ全体を名前・説明から検索します")).toBeInTheDocument();
    fireEvent.change(search, { target: { value: "目当て" } });
    await waitFor(() => expect(window.location.hash).toContain("series_q=%E7%9B%AE%E5%BD%93%E3%81%A6"), { timeout: 1500 });
    await waitFor(() => expect(shelfApi.listEntitySeriesPage).toHaveBeenCalledWith("pixiv", "aoba", {
      query: "目当て",
      limit: 60,
      cursor: null,
    }));
    expect(await screen.findByText("目当ての長編")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "シリーズの検索語を消す" })).toHaveAccessibleName();
  });

  it("keeps loaded series visible when a continuation cursor becomes stale", async () => {
    shelfApi.listEntitySeriesPage.mockImplementation(async (_source, _key, params) => {
      if (params.cursor) throw new Error("Library changed while paging entity series; restart the list");
      return { items: [series()], nextCursor: "stale-cursor", total: 2 };
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

  it("bounds a pasted deep page before it becomes an OFFSET and normalizes the URL", async () => {
    window.localStorage.setItem("piep.paging-mode", JSON.stringify("pages"));
    dbApi.searchDownloadsV2.mockResolvedValue({
      items: [], nextCursor: null, totalEstimate: 20_000,
      searchMeta: { engine: "sqlite-metadata", query: null, totalEstimate: 20_000, indexComplete: true, explanations: [] },
      facetsVersion: 0,
    });
    renderAuthor("#/people/pixiv/aoba?page=999999");

    await waitFor(() => expect(dbApi.searchDownloadsV2).toHaveBeenCalledWith(expect.objectContaining({ offset: 5_000 })));
    await waitFor(() => expect(window.location.hash).toContain("page=251"));
    expect(screen.getByRole("status")).toHaveTextContent("直接開けるのは251ページ目まで");
  });

  it("searches and sorts within the author", async () => {
    renderAuthor();
    const search = await screen.findByLabelText("青葉しおりの作品を検索");
    fireEvent.change(search, { target: { value: "灯台" } });
    await waitFor(() => expect(dbApi.searchDownloadsV2).toHaveBeenCalledWith(
      // A text search inside an author ranks by relevance, as the library does.
      expect.objectContaining({ text: "灯台", sortBy: "relevance" }),
    ));
  });

  it("restores its narrowing from the address", async () => {
    renderAuthor("#/people/pixiv/aoba?q=%E7%81%AF%E5%8F%B0&tag=%E9%95%B7%E7%B7%A8&sort=title");
    await waitFor(() => expect(dbApi.searchDownloadsV2).toHaveBeenCalledWith(
      expect.objectContaining({ text: "灯台", tagsInclude: ["長編"], sortBy: "title", sortOrder: "asc" }),
    ));
  });

  it("says so when a filter leaves nothing, and offers the way out", async () => {
    renderAuthor("#/people/pixiv/aoba?tag=%E7%9F%AD%E7%B7%A8");
    expect(await screen.findByText("条件に合う作品がありません")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "絞り込みを解除" }));
    await waitFor(() => expect(window.location.hash).not.toContain("tag="));
  });

  it("keeps working for an author with no series and no tags", async () => {
    shelfApi.listEntitySeriesPage.mockResolvedValue({ items: [], nextCursor: null, total: 0 });
    shelfApi.listEntityTags.mockResolvedValue([]);
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
    dbApi.getPerson.mockResolvedValue({ ...person, sourceKey: "creator%key" });
    dbApi.searchDownloadsV2.mockResolvedValue({
      items: [], nextCursor: null, totalEstimate: 0,
      searchMeta: { engine: "sqlite-metadata", query: null, totalEstimate: 0, indexComplete: true, explanations: [] },
      facetsVersion: 0,
    });
    shelfApi.listEntitySeriesPage.mockResolvedValue({ items: [], nextCursor: null, total: 0 });
    shelfApi.listEntityTags.mockResolvedValue([]);
    renderAuthor("#/people/pixiv/creator%25key");

    expect(await screen.findByRole("heading", { level: 1 })).toBeInTheDocument();
  });
});

describe("series page", () => {
  beforeEach(() => {
    window.localStorage.clear();
    dbApi.getSeries.mockResolvedValue({
      id: 1,
      source: "pixiv",
      sourceKey: "s1",
      displayName: "季節の栞",
      coverPath: null,
      description: null,
      metadataJson: null,
      contentHash: null,
      currentVersion: 1,
      lastCheckedAt: null,
      lastFetchedAt: null,
      createdAt: "",
      updatedAt: "2026-08-01T00:00:00Z",
      workCount: 2,
    });
    dbApi.searchDownloadsV2.mockResolvedValue({
      items: [], nextCursor: null, totalEstimate: 0,
      searchMeta: { engine: "sqlite-metadata", query: null, totalEstimate: 0, indexComplete: true, explanations: [] },
      facetsVersion: 0,
    });
    shelfApi.listEntityTags.mockResolvedValue(tags);
    collectionApi.listCollectionsForPerson.mockResolvedValue([]);
    collectionApi.listCollectionsForSeries.mockResolvedValue([]);
  });

  /**
   * コレクションへの入口は作者にしか無く、同じ画面で作りが割れていた。
   * 辿る経路が `download_people` か `download_series` かの違いしかない。
   */
  it("opens the collections this series belongs to, the same way an author does", async () => {
    collectionApi.listCollectionsForSeries.mockResolvedValue([{
      id: "collection-1", name: "連作の棚", description: null, collectionKind: "unordered",
      coverDownloadId: null, coverPath: null, coverMode: "mosaic", coverImagePath: null,
      coverTiles: [], nameSource: "manual", track: "manual", revision: 1,
      memberCount: 2, availableCount: 2, totalTextLength: 1200,
      createdAt: "2026-08-01T00:00:00Z", updatedAt: "2026-08-01T00:00:00Z",
    }]);
    window.location.hash = "#/series/pixiv/s1?tab=collections";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <MantineProvider>
        <QueryClientProvider client={client}>
          <AppRouter><EntityPage kind="series" /></AppRouter>
        </QueryClientProvider>
      </MantineProvider>,
    );

    const tab = await screen.findByRole("tab", { name: /コレクション/ });
    expect(within(tab).getByText("1")).toBeInTheDocument();
    expect(collectionApi.listCollectionsForSeries).toHaveBeenCalledWith("pixiv", "s1");
    // 作者側の入口は、シリーズを開いたときには使わない。
    expect(collectionApi.listCollectionsForPerson).not.toHaveBeenCalled();
    expect(await screen.findByText("連作の棚")).toBeInTheDocument();
  });

  it("does not ask for author-only data", async () => {
    shelfApi.listEntitySeriesPage.mockClear();
    shelfApi.listEntityTags.mockClear();
    window.location.hash = "#/series/pixiv/s1";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <MantineProvider>
        <QueryClientProvider client={client}>
          <AppRouter><EntityPage kind="series" /></AppRouter>
        </QueryClientProvider>
      </MantineProvider>,
    );
    await waitFor(() => expect(dbApi.searchDownloadsV2).toHaveBeenCalled());
    // A series has no list of series, but it does have tags worth narrowing by.
    expect(shelfApi.listEntitySeriesPage).not.toHaveBeenCalled();
    expect(shelfApi.listEntityTags).toHaveBeenCalledWith("series", "pixiv", "s1");
    expect(within(document.body).queryByRole("tab", { name: /シリーズ/ })).toBeNull();
  });
});
