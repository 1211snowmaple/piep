import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EntityPage } from "./EntityPage";
import { EntityBrowser } from "./LibraryGroups";
import { defaultPcSearch, type PcEntity, type PcWork } from "./pcApi";
import { useWorkList, WorkResults } from "./workList";

const api = vi.hoisted(() => ({
  searchPcWorks: vi.fn(), listPcEntities: vi.fn(), listPcEntitySeries: vi.fn(),
  getPcEntityDetail: vi.fn(), listOfflineWorks: vi.fn(),
}));
vi.mock("./pcApi", async (original) => ({ ...await original<typeof import("./pcApi")>(), ...api }));
vi.mock("./appHistory", () => ({ useAppNavigation: () => ({}) }));
// JSDOM has no viewport. Render rows directly while exercising the real query and observer hooks.
vi.mock("./VirtualWorkList", () => ({
  VirtualWorkList: function Rows<T>({ items, renderItem }: { items: T[]; renderItem: (item: T) => ReactNode }) {
    return <>{items.map(renderItem)}</>;
  },
}));

function WorkBrowser() {
  const list = useWorkList({ search: { ...defaultPcSearch, sort: "standard" }, scope: null, pagingMode: "auto", page: 1, enabled: true });
  return <WorkResults list={list} savedRevision={0} reachable onOpenWork={() => {}} onOpenDetail={() => {}} onSaved={() => {}} onPage={() => {}} emptyTitle="empty" emptyHint="" />;
}

const work: PcWork = {
  id: 1, source: "pixiv", sourceId: "1", title: "最初の作品", authorName: "作者", authorId: "1",
  contentType: "novel", tags: [], excerpt: null, textLength: 1, assetCount: 0, hasCover: false,
  currentVersion: 1, contentHash: null, favorite: false, seriesTitle: null, seriesId: null,
};
const entity: PcEntity = {
  source: "pixiv", sourceKey: "1", displayName: "最初の作者", count: 1, description: null,
  sampleTitle: null, hasCover: false, isConcluded: null,
};
const clients: QueryClient[] = [];

beforeEach(() => {
  Object.values(api).forEach((mock) => mock.mockReset());
  api.listOfflineWorks.mockResolvedValue([]);
  api.getPcEntityDetail.mockResolvedValue({ ...entity, workCount: 1, hasBanner: false, publishedCount: null, links: [], tags: [], seriesCount: 1, collections: [] });
  vi.stubGlobal("IntersectionObserver", class {
    active = true;
    constructor(private callback: IntersectionObserverCallback) {}
    observe(target: Element) {
      queueMicrotask(() => {
        if (this.active) this.callback([{ target, isIntersecting: true } as IntersectionObserverEntry], this as unknown as IntersectionObserver);
      });
    }
    disconnect() { this.active = false; }
  });
});
afterEach(() => {
  cleanup();
  clients.splice(0).forEach((client) => client.clear());
  vi.unstubAllGlobals();
});

describe("自動ページングの失敗", () => {
  it.each(["works", "entities", "person-series"] as const)("%s は失敗した続きを自動で繰り返さず、手動で再試行できる", async (kind) => {
    const request = kind === "works" ? api.searchPcWorks : kind === "entities" ? api.listPcEntities : api.listPcEntitySeries;
    const first = kind === "works" ? { items: [work], nextCursor: "next", totalEstimate: 2, indexComplete: true }
      : kind === "entities" ? { items: [entity], nextOffset: 1, total: 2 }
        : { items: [entity], nextCursor: "next", total: 2 };
    const last = kind === "entities" ? { ...first, items: [], nextOffset: null } : { ...first, items: [], nextCursor: null };
    request.mockResolvedValueOnce(first).mockRejectedValueOnce(new Error("次のページに接続できません")).mockResolvedValue(last);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    clients.push(client);
    render(<QueryClientProvider client={client}>
      {kind === "works" ? <WorkBrowser /> : kind === "entities" ? <EntityBrowser kind="people" query="" sort="name:asc" onOpen={() => {}} />
        : <EntityPage page={{ name: "entity", kind: "person", source: "pixiv", key: "1", displayName: "作者", tab: "series", query: "", tags: [], seriesQuery: "", listPage: 1 }} status={{ paired: true, reachable: true, url: "https://example.test", deviceName: "test", issue: null, capabilities: [] }} savedRevision={0} pagingMode="auto" onOpenWork={() => {}} onOpenDetail={() => {}} onSaved={() => {}} />}
    </QueryClientProvider>);
    expect(await screen.findByRole("alert")).toHaveProperty("textContent", "次のページに接続できません");
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 100)); });
    expect(request).toHaveBeenCalledTimes(2);
    fireEvent.click(screen.getByRole("button", { name: "さらに表示" }));
    await waitFor(() => expect(request).toHaveBeenCalledTimes(3));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(screen.queryByRole("button", { name: "さらに表示" })).toBeNull();
  });
});
