import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import EntityPage from "@/features/library/EntityPage";
import type { EntityFacet, FacetCount } from "@/types/library";

const shelfApi = vi.hoisted(() => ({
  listEntitySeries: vi.fn(),
  listEntityTags: vi.fn(),
  getLibraryShelfCounts: vi.fn(),
  listSavedSearches: vi.fn(),
  upsertSavedSearch: vi.fn(),
  deleteSavedSearch: vi.fn(),
}));
const dbApi = vi.hoisted(() => ({ searchDownloadsV2: vi.fn(), getPerson: vi.fn() }));

vi.mock("@/services/shelfApi", () => shelfApi);
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getPerson: dbApi.getPerson,
  getSeries: vi.fn(),
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
    shelfApi.listEntitySeries.mockResolvedValue([series()]);
    shelfApi.listEntityTags.mockResolvedValue(tags);
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
    await waitFor(() => expect(shelfApi.listEntitySeries).toHaveBeenCalledWith("pixiv", "aoba"));
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
    shelfApi.listEntitySeries.mockResolvedValue([]);
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
    shelfApi.listEntitySeries.mockResolvedValue([]);
    shelfApi.listEntityTags.mockResolvedValue([]);
    renderAuthor("#/people/pixiv/creator%25key");

    expect(await screen.findByRole("heading", { level: 1 })).toBeInTheDocument();
  });
});

describe("series page", () => {
  beforeEach(() => {
    window.localStorage.clear();
    dbApi.searchDownloadsV2.mockResolvedValue({
      items: [], nextCursor: null, totalEstimate: 0,
      searchMeta: { engine: "sqlite-metadata", query: null, totalEstimate: 0, indexComplete: true, explanations: [] },
      facetsVersion: 0,
    });
  });

  it("does not ask for author-only data", async () => {
    shelfApi.listEntitySeries.mockClear();
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
    expect(shelfApi.listEntitySeries).not.toHaveBeenCalled();
    expect(shelfApi.listEntityTags).toHaveBeenCalledWith("series", "pixiv", "s1");
    expect(within(document.body).queryByRole("tab", { name: /シリーズ/ })).toBeNull();
  });
});
