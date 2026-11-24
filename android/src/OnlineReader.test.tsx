import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { OnlineReader } from "./OnlineReader";
import type { PcConnectionStatus, PcWork } from "./pcApi";
import { defaultReaderBehavior } from "./readerPrefs";
import { adoptShelf } from "./shelf";

const api = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: api.invoke, isTauri: () => true }));
vi.mock("./appHistory", () => ({ useAppNavigation: () => ({}) }));
vi.mock("./FavoriteButton", () => ({ WorkFavoriteButton: () => null }));
vi.mock("./readingSync", () => ({
  refreshReading: async () => {},
  refreshReadingWithin: async () => {},
  shelfReadingFor: () => null,
}));

afterEach(() => {
  cleanup();
  adoptShelf(null);
});

it("作品情報で版を確定してから本文を開き、食い違ったら情報から取り直す", async () => {
  const scope = { libraryId: "library", subjectId: "reader", syncEpoch: "epoch", connectionGeneration: 1 };
  adoptShelf({ reachable: true, shelf: { scope, readerBundle: true } } as PcConnectionStatus);
  const work: PcWork = {
    id: "work", source: "pixiv", sourceId: "42", snapshotId: "older", editId: null, title: "作品", readingScope: scope,
    authorName: "作者", authorId: "7", contentType: "novel", tags: [], excerpt: null, textLength: 10, assetCount: 0,
    hasCover: false, currentVersion: 1, edited: false, favorite: false, favoriteGeneration: "1",
    seriesTitle: null, seriesId: null, sourceCreatedAt: null, savedAt: "2026-11-18T00:00:00.000Z",
  };
  const metadata = (snapshotId: string) => ({ work: { ...work, snapshotId }, people: [], series: [], versions: [] });
  let deliverMetadata!: (value: ReturnType<typeof metadata>) => void;
  const first = new Promise<ReturnType<typeof metadata>>((resolve) => { deliverMetadata = resolve; });
  const calls: Array<[string, unknown]> = [];
  let requests = 0;
  let opens = 0;
  api.invoke.mockImplementation(async (command: string, args: unknown) => {
    calls.push([command, args]);
    if (command === "pc_work") return ++requests === 1 ? first : metadata("newest");
    if (command === "pc_open") {
      const id = ++opens === 1 ? "older" : "newest";
      return { key: id, entityId: work.id, view: { kind: "snapshot", id }, pageCount: 1, sourcePageStarts: [0], totalPlainTextChars: 10, outline: [] };
    }
    if (command === "pc_page") return { page: 0, pageCount: 1, html: "<p>一致した本文</p>", totalPlainTextChars: 10, sourcePageStarts: [0] };
    if (command === "reading_local_put") return null;
    throw new Error(command);
  });

  render(<OnlineReader work={work} type={{ fontSize: 16, lineHeight: 1.8, readerWidth: 600, fontFamily: "sans", readerTheme: "white" }} behavior={defaultReaderBehavior} onBack={() => {}} onTypeChange={() => {}} onBehaviorChange={() => {}} />);
  expect(calls.map(([command]) => command)).toEqual(["pc_work"]);
  await act(async () => { deliverMetadata(metadata("current")); });
  await screen.findByText("一致した本文");
  expect(calls.filter(([command]) => command.startsWith("pc_"))).toEqual([
    ["pc_work", { library: scope, work: work.id }],
    ["pc_open", { scope, work: work.id, view: { kind: "snapshot", id: "current" } }],
    ["pc_work", { library: scope, work: work.id }],
    ["pc_open", { scope, work: work.id, view: { kind: "snapshot", id: "newest" } }],
    ["pc_page", { key: "newest", page: 0 }],
  ]);
  expect(calls.filter(([command]) => command === "reading_local_put")).toEqual([
    ["reading_local_put", { reading: expect.objectContaining({ snapshotId: "newest", editId: null }) }],
  ]);
});
