import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { OnlineReader } from "./OnlineReader";
import type { PcWork } from "./pcApi";
import { defaultReaderBehavior } from "./readerPrefs";

const api = vi.hoisted(() => ({ getOfflinePage: vi.fn(), rememberReading: vi.fn() }));
vi.mock("./pcApi", async (original) => ({ ...await original<typeof import("./pcApi")>(), getOfflinePage: api.getOfflinePage }));
vi.mock("./localReading", async (original) => ({ ...await original<typeof import("./localReading")>(), rememberReading: api.rememberReading }));
vi.mock("./appHistory", () => ({ useAppNavigation: () => ({}) }));
vi.mock("./FavoriteButton", () => ({ WorkFavoriteButton: () => null }));

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

it("最後のページも末尾に届くまでは読みかけとして記録する", async () => {
  vi.stubGlobal("matchMedia", () => ({ matches: false, addEventListener() {}, removeEventListener() {} }));
  const work = {
    readingScope: { libraryId: "l1", subjectId: "u1" }, id: "w1", source: "pixiv", sourceId: "1", title: "作品", authorName: "作者", authorId: "1", contentType: "novel",
    tags: [], excerpt: null, textLength: 100, assetCount: 0, hasCover: false, favorite: false, seriesTitle: null, seriesId: null,
    currentVersion: 1, snapshotId: "s1", edited: false, editId: null, favoriteGeneration: "0", sourceCreatedAt: null, savedAt: "2026-11-01T00:00:00Z",
  } satisfies PcWork;
  api.getOfflinePage.mockResolvedValue({ page: 1, pageCount: 2, generation: "1", html: "<p>最後のページの本文</p>" });
  render(<OnlineReader work={work} offline startPage={1}
    type={{ fontSize: 16, lineHeight: 1.8, readerWidth: 660, fontFamily: "sans", readerTheme: "white" }}
    behavior={defaultReaderBehavior} onBack={() => {}} onTypeChange={() => {}} onBehaviorChange={() => {}} />);
  await screen.findByText("最後のページの本文");
  const viewport = screen.getByRole("main");
  Object.defineProperty(viewport, "scrollHeight", { value: 1000 });

  viewport.scrollTop = 980;
  fireEvent.scroll(viewport);
  await waitFor(() => expect(api.rememberReading).toHaveBeenCalledWith(work, 0.99, false));

  viewport.scrollTop = 1000;
  fireEvent.scroll(viewport);
  await waitFor(() => expect(api.rememberReading).toHaveBeenLastCalledWith(work, 1, true));
});
