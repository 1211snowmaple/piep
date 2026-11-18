import type { ReactElement, ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render as renderRaw, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { OfflineWork, PcWork } from "./pcApi";

const api = vi.hoisted(() => ({ listOfflineWorks: vi.fn(), listOfflineIssues: vi.fn(), searchOfflineText: vi.fn() }));
vi.mock("./pcApi", async (original) => ({ ...await original<typeof import("./pcApi")>(), ...api }));
vi.mock("@tauri-apps/api/core", async (original) => ({ ...await original<typeof import("@tauri-apps/api/core")>(), isTauri: () => true }));
vi.mock("./appHistory", () => ({ useAppNavigation: () => ({ push: vi.fn(), canBrowseServer: false }) }));
vi.mock("./VirtualWorkList", () => ({
  VirtualWorkList: function Rows<T>({ items, renderItem }: { items: T[]; renderItem: (item: T) => ReactNode }) {
    return <>{items.map(renderItem)}</>;
  },
}));
vi.mock("./WorkThumbnail", () => ({ WorkThumbnail: () => null }));

const { OfflineLibrary } = await import("./OfflineLibrary");

function render(element: ReactElement) {
  return renderRaw(<QueryClientProvider client={new QueryClient()}>{element}</QueryClientProvider>);
}

function saved(id: number, title: string): OfflineWork {
  const work: PcWork = {
    id, source: "pixiv", sourceId: String(id), title, authorName: "作者", authorId: "1", contentType: "novel",
    tags: [], excerpt: null, textLength: 100, assetCount: 0, hasCover: false, favorite: false, seriesTitle: null, seriesId: null,
    currentVersion: 1, contentHash: "h",
  } as PcWork;
  return { work, savedAtMs: id, savedVersion: 1, savedContentHash: "h", savedEditHash: null, pageCount: 5 };
}

afterEach(cleanup);

describe("保存した作品の本文検索", () => {
  it("題名に無い語でも本文にあれば並べ、見つかったページから開ける", async () => {
    api.listOfflineWorks.mockResolvedValue([saved(1, "夏の話"), saved(2, "冬の話")]);
    api.listOfflineIssues.mockResolvedValue([]);
    api.searchOfflineText.mockResolvedValue([{ workId: 2, page: 3, snippet: "…そこで灯台守は言った…", pages: 2 }]);
    const open = vi.fn();
    render(<OfflineLibrary page={{ name: "offline", tab: "works", groupId: null }} onOpenWork={open} onOpenDetail={() => {}} onShowPc={() => {}} savedRevision={0} pagingMode="auto" rememberedSort="dateDesc" onRememberedSortChange={() => {}} />);
    await screen.findByText("夏の話");

    const field = screen.getByRole("searchbox");
    fireEvent.change(field, { target: { value: "灯台守" } });
    fireEvent.submit(field.closest("form")!);

    await waitFor(() => expect(screen.queryByText("夏の話")).toBeNull());
    expect(api.searchOfflineText).toHaveBeenCalledWith("灯台守");
    const hit = screen.getByRole("button", { name: "本文の 4 ページ目を開く" });
    expect(hit.textContent).toContain("4 ページ目ほか 1 ページ");
    expect(within(hit).getByText("灯台守").tagName).toBe("MARK");
    fireEvent.click(hit);
    expect(open).toHaveBeenCalledWith(expect.objectContaining({ work: expect.objectContaining({ id: 2 }) }), 3);
  });

  it("本文を探せなかったときも、題名などでは探せる", async () => {
    api.listOfflineWorks.mockResolvedValue([saved(1, "夏の話"), saved(2, "冬の話")]);
    api.listOfflineIssues.mockResolvedValue([]);
    api.searchOfflineText.mockRejectedValue({ code: "device_records" });
    render(<OfflineLibrary page={{ name: "offline", tab: "works", groupId: null }} onOpenWork={() => {}} onOpenDetail={() => {}} onShowPc={() => {}} savedRevision={1} pagingMode="auto" rememberedSort="dateDesc" onRememberedSortChange={() => {}} />);
    await screen.findByText("夏の話");
    const field = screen.getByRole("searchbox");
    fireEvent.change(field, { target: { value: "冬" } });
    fireEvent.submit(field.closest("form")!);
    await waitFor(() => expect(screen.queryByText("夏の話")).toBeNull());
    expect(screen.getByText("冬の話")).toBeTruthy();
  });
});
