import { MantineProvider } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import EntityPage from "@/features/library/EntityPage";
import { SHELF_V2_PREFERENCE_KEY } from "@/features/library/shelfV2";
import type { ShelfCard, ShelfEntityHead, ShelfSeriesFacet } from "@/services/libraryV2Api";

vi.mock("@tauri-apps/api/core", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@tauri-apps/api/core")>()),
  convertFileSrc: (path: string, protocol: string) => `http://${protocol}.localhost/${path}`,
}));

const api = vi.hoisted(() => ({
  libraryV2Status: vi.fn(),
  libraryV2Entity: vi.fn(),
  libraryV2EntityTags: vi.fn(),
  libraryV2PersonSeries: vi.fn(),
  libraryV2EntityOriginal: vi.fn(),
  libraryV2EntityCollections: vi.fn(),
  libraryV2Search: vi.fn(),
  libraryV2Decorations: vi.fn(),
}));
const refresh = vi.hoisted(() => vi.fn());
const archive = vi.hoisted(() => ({ exportShelf: vi.fn(), exportLegacy: vi.fn(), choose: vi.fn() }));
vi.mock("@/services/shelfBackupApi", () => ({ libraryV2ExportEntityArchive: archive.exportShelf }));
vi.mock("@/services/archiveApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/archiveApi")>()),
  exportEntityZip: archive.exportLegacy,
}));
vi.mock("@/services/dialogApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dialogApi")>()),
  openSingleDialog: archive.choose,
}));
const watching = vi.hoisted(() => ({ get: vi.fn(), upsert: vi.fn(), check: vi.fn() }));
vi.mock("@/features/updates/startSingleCheck", () => ({ runSingleCheck: watching.check }));
vi.mock("@/store", () => ({ store: { get: vi.fn().mockResolvedValue("test-connection") } }));
vi.mock("@/services/libraryV2Api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/libraryV2Api")>()),
  ...api,
}));
// 旧棚の命令は一つも呼ばれないこと。
const legacy = vi.hoisted(() => ({
  getPerson: vi.fn(),
  getSeries: vi.fn(),
  searchDownloadsV2: vi.fn(),
  listEntityVersions: vi.fn(),
  getLatestEntityProfileJson: vi.fn(),
  listEntitySeriesPage: vi.fn(),
  listEntityTags: vi.fn(),
  listCollectionsForPerson: vi.fn(),
  listCollectionsForSeries: vi.fn(),
}));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getPerson: legacy.getPerson,
  getSeries: legacy.getSeries,
  searchDownloadsV2: legacy.searchDownloadsV2,
  listEntityVersions: legacy.listEntityVersions,
  getLatestEntityProfileJson: legacy.getLatestEntityProfileJson,
  getUpdateTarget: watching.get,
  upsertUpdateTarget: watching.upsert,
  refreshEntityProfile: refresh,
}));
vi.mock("@/services/shelfApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/shelfApi")>()),
  listEntitySeriesPage: legacy.listEntitySeriesPage,
  listEntityTags: legacy.listEntityTags,
}));
vi.mock("@/services/collectionApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/collectionApi")>()),
  listCollectionsForPerson: legacy.listCollectionsForPerson,
  listCollectionsForSeries: legacy.listCollectionsForSeries,
}));

const PERSON = "0195f1c2-0000-7000-8000-0000000000a1";
const SERIES = "0195f1c2-0000-7000-8000-0000000000b1";
const SNAPSHOT = "0195f1c2-0000-7000-8000-0000000000c1";
const ICON = "cd".repeat(32);

function head(overrides: Partial<ShelfEntityHead> = {}): ShelfEntityHead {
  return {
    id: PERSON,
    kind: "person",
    provider: "pixiv",
    external_key: "77",
    name: "作者一",
    description: "紹介",
    links: [],
    icon: ICON,
    banner: null,
    cover: null,
    is_concluded: null,
    published_content_count: null,
    work_count: 3,
    snapshot: SNAPSHOT,
    version_number: 2,
    last_checked_at: null,
    last_fetched_at: Date.UTC(2026, 0, 2),
    versions: [
      { snapshot: "0195f1c2-0000-7000-8000-0000000000c0", version: 1, saved_at: Date.UTC(2026, 0, 1), text_length: null },
      { snapshot: SNAPSHOT, version: 2, saved_at: Date.UTC(2026, 0, 2), text_length: null },
    ],
    ...overrides,
  };
}

const facet: ShelfSeriesFacet = {
  id: SERIES,
  provider: "pixiv",
  external_key: "s1",
  name: "続き物",
  work_count: 2,
  cover: null,
  description: null,
  is_concluded: true,
  sample_title: null,
  latest_saved_at: Date.UTC(2026, 0, 2),
  latest_published_at: Date.UTC(2026, 0, 1),
  latest_updated_at: Date.UTC(2026, 0, 1),
  total_text_length: 10,
  total_size: 100,
  total_media: 0,
  total_revisions: 0,
};

const card: ShelfCard = {
  id: "0195f1c2-0000-7000-8000-000000000001",
  snapshot_id: SNAPSHOT,
  edit_id: null,
  provider: "pixiv",
  external_key: "1",
  content_type: "novel",
  title: "題",
  author_name: "作者一",
  saved_at: Date.UTC(2026, 0, 2),
  published_at: null,
  updated_at: null,
  text_length: 5,
  logical_size: 10,
  media_count: 0,
  version_number: 1,
  favorite: false,
  watching: false,
  revised: false,
  image_count: 0,
  file_count: 0,
  excerpt: null,
  tags: [],
  cover: null,
  series_title: null,
  series_position: null,
  coverFile: null,
  legacyId: null,
};

function renderEntity(kind: "person" | "series", hash: string, client = new QueryClient({ defaultOptions: { queries: { retry: false } } })) {
  window.location.hash = hash;
  return render(
    <MantineProvider>
      <QueryClientProvider client={client}>
        <AppRouter><WorkspaceProvider><div><EntityPage kind={kind} /></div></WorkspaceProvider></AppRouter>
      </QueryClientProvider>
    </MantineProvider>,
  );
}

describe("author and series pages on the new shelf", () => {
  beforeAll(() => {
    Element.prototype.scrollIntoView = () => {};
  });

  beforeEach(() => {
    window.localStorage.clear();
    window.localStorage.setItem(SHELF_V2_PREFERENCE_KEY, "true");
    vi.clearAllMocks();
    watching.get.mockReset().mockResolvedValue(null);
    watching.upsert.mockReset().mockResolvedValue(undefined);
    watching.check.mockReset().mockResolvedValue(undefined);
    api.libraryV2Status.mockResolvedValue({ state: "ready", path: "x", unresolved: [] });
    api.libraryV2Entity.mockResolvedValue(head());
    api.libraryV2EntityTags.mockResolvedValue([{ name: "雨", count: 2 }]);
    api.libraryV2PersonSeries.mockResolvedValue({ items: [facet], next: null, total: 1 });
    api.libraryV2EntityOriginal.mockResolvedValue(JSON.stringify({ displayName: "作者一", sampleNovelCount: 4 }));
    api.libraryV2Search.mockResolvedValue({ page: { items: [], next: null, total: 1 }, intent: null, matches: [], cards: [card] });
    api.libraryV2Decorations.mockResolvedValue([]);
    api.libraryV2EntityCollections.mockResolvedValue([]);
  });

  it("reads an author from the new shelf and never asks the old one", async () => {
    renderEntity("person", "#/people/pixiv/77");
    expect(await screen.findByRole("heading", { name: "作者一" })).toBeInTheDocument();
    expect(api.libraryV2Entity).toHaveBeenCalledWith("person", "pixiv", "77");
    expect(screen.getByText("新しい棚（試験中）", { selector: ".mantine-Badge-label" })).toBeInTheDocument();
    // 作品はこの作者の棚の ID で絞り、公開日の新しい順。
    await waitFor(() => expect(api.libraryV2Search).toHaveBeenCalled());
    const [, query] = api.libraryV2Search.mock.calls[0];
    expect(query).toMatchObject({ sort: "published", descending: true, filter: { person: PERSON, series: null } });
    expect(await screen.findByText("雨")).toBeInTheDocument();
    const seriesTab = await screen.findByRole("tab", { name: /シリーズ/ });
    await waitFor(() => expect(within(seriesTab).getByText("1")).toBeInTheDocument());
    expect(api.libraryV2PersonSeries).toHaveBeenCalledWith(PERSON, expect.objectContaining({ sort: "work_count", offset: 0 }));
    // 画像は棚のファイルの識別子から。
    expect(document.querySelector(`img[src="http://piep-library.localhost/${ICON}"]`)).not.toBeNull();
    for (const call of Object.values(legacy)) expect(call).not.toHaveBeenCalled();
  });

  it("writes the author's archive from the shelf, even before switching", async () => {
    const show = vi.spyOn(notifications, "show");
    archive.choose.mockResolvedValue("D:/archives");
    archive.exportShelf.mockResolvedValue({ path: "D:/archives/作者一", works: 3 });
    renderEntity("person", "#/people/pixiv/77");
    await screen.findByRole("heading", { name: "作者一" });
    fireEvent.click(screen.getByRole("button", { name: "アーカイブ" }));
    await waitFor(() => expect(archive.exportShelf).toHaveBeenCalledWith("person", "pixiv", "77", "D:/archives", "作者一"));
    await waitFor(() => expect(show).toHaveBeenCalledWith(expect.objectContaining({ title: "3作品を書き出しました", message: "D:/archives/作者一" })));
    expect(archive.exportLegacy).not.toHaveBeenCalled();
  });

  it("keeps the controls that write to the old shelf in place but unpressable", async () => {
    renderEntity("person", "#/people/pixiv/77");
    await screen.findByRole("heading", { name: "作者一" });
    expect(screen.getByRole("button", { name: "情報を更新（新しい棚では変えられません）" })).toBeDisabled();
    expect(screen.getByRole("switch", { name: "新作と改稿の監視（新しい棚では変えられません）" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "いま新作・改稿を確認（新しい棚では変えられません）" })).toBeDisabled();
    expect(watching.get).not.toHaveBeenCalled();
    expect(watching.upsert).not.toHaveBeenCalled();
    expect(watching.check).not.toHaveBeenCalled();
    // 開く操作は取得元へ行くだけなので残す。
    expect(screen.getByRole("button", { name: "ブラウザで開く" })).toBeEnabled();
    // コレクションに入っていなければ、旧棚と同じくタブを出さない。
    await waitFor(() => expect(api.libraryV2EntityCollections).toHaveBeenCalledWith("person", PERSON));
    expect(screen.queryByRole("tab", { name: /コレクション/ })).toBeNull();
  });

  it.each(["person", "series"] as const)("changes watching and starts a one-off %s check on the switched shelf", async (kind) => {
    const sourceKey = kind === "person" ? "77" : "55";
    const name = kind === "person" ? "作者一" : "シリーズ";
    const targetType = kind === "person" ? "author" : "series";
    api.libraryV2Status.mockResolvedValue({ state: "ready", path: "x", unresolved: [], switched: true });
    api.libraryV2Entity.mockResolvedValue(head({ id: kind === "person" ? PERSON : SERIES, kind, name, external_key: sourceKey }));
    watching.get.mockResolvedValue({ id: PERSON, enabled: true, watchGeneration: 1 });
    watching.upsert.mockImplementation(async () => { watching.get.mockResolvedValue({ id: PERSON, enabled: false, watchGeneration: 2 }); });
    renderEntity(kind, kind === "person" ? "#/people/pixiv/77" : "#/series/pixiv/55");
    const toggle = await screen.findByRole("switch", { name: "新作と改稿の監視" });
    await waitFor(() => expect(toggle).toBeChecked());
    expect(watching.get).toHaveBeenCalledWith(targetType, "pixiv", sourceKey);
    fireEvent.click(toggle);
    await waitFor(() => expect(watching.upsert).toHaveBeenCalledWith({ targetType, source: "pixiv", sourceKey, displayName: name, enabled: false, metadataJson: null }));
    await waitFor(() => { expect(toggle).toBeEnabled(); expect(toggle).not.toBeChecked(); });
    fireEvent.click(screen.getByRole("button", { name: kind === "person" ? "いま新作・改稿を確認" : "いま新作を確認" }));
    await waitFor(() => expect(watching.check).toHaveBeenCalledWith({ kind: targetType, source: "pixiv", sourceKey, label: name }, expect.any(Function)));
    for (const call of Object.values(legacy)) expect(call).not.toHaveBeenCalled();
  });

  it("reads the switched shelf's watch state even if the old shelf's state is cached", async () => {
    api.libraryV2Status.mockResolvedValue({ state: "ready", path: "x", unresolved: [], switched: true });
    watching.get.mockResolvedValue({ id: PERSON, enabled: true, watchGeneration: 1 });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
    client.setQueryData(["update-targets", "person", "pixiv", "77", "legacy"], { id: 7, enabled: false });
    renderEntity("person", "#/people/pixiv/77", client);
    await waitFor(() => expect(screen.getByRole("switch", { name: "新作と改稿の監視" })).toBeChecked());
    expect(watching.get).toHaveBeenCalledWith("author", "pixiv", "77");
  });

  it("keeps the saved watch state when the switched shelf refuses a change", async () => {
    api.libraryV2Status.mockResolvedValue({ state: "ready", path: "x", unresolved: [], switched: true });
    watching.get.mockResolvedValue({ id: PERSON, enabled: true, watchGeneration: 1 });
    watching.upsert.mockRejectedValue(new Error("競合"));
    renderEntity("person", "#/people/pixiv/77");
    const toggle = await screen.findByRole("switch", { name: "新作と改稿の監視" });
    await waitFor(() => expect(toggle).toBeChecked());
    fireEvent.click(toggle);
    await waitFor(() => expect(watching.upsert).toHaveBeenCalled());
    await waitFor(() => { expect(toggle).toBeEnabled(); expect(toggle).toBeChecked(); });
  });

  it("refreshes the switched shelf and reloads its head and saved response", async () => {
    api.libraryV2Status.mockResolvedValue({ state: "ready", path: "x", unresolved: [], switched: true });
    refresh.mockImplementation(async () => {
      api.libraryV2Entity.mockResolvedValue(head({ name: "更新した作者", last_checked_at: Date.UTC(2026, 0, 3) }));
      api.libraryV2EntityOriginal.mockResolvedValue(JSON.stringify({ displayName: "更新した作者" }));
      return {};
    });
    renderEntity("person", "#/people/pixiv/77");
    await screen.findByRole("heading", { name: "作者一" });
    const button = screen.getByRole("button", { name: "情報を更新" });
    expect(button).toBeEnabled();
    fireEvent.click(button);
    await waitFor(() => expect(refresh).toHaveBeenCalledWith(expect.objectContaining({ entityType: "person", source: "pixiv", sourceKey: "77", force: true })));
    expect(await screen.findByRole("heading", { name: "更新した作者" })).toBeInTheDocument();
    await waitFor(() => expect(api.libraryV2EntityOriginal.mock.calls.length).toBeGreaterThan(1));
    for (const call of Object.values(legacy)) expect(call).not.toHaveBeenCalled();
  });

  it("lists the collections holding the author's works and opens them by their old id", async () => {
    api.libraryV2EntityCollections.mockResolvedValue([{
      id: "0195f1c2-0000-7000-8000-0000000000d1",
      legacy_id: "c-old",
      name: "雨の束",
      description: null,
      ordered: true,
      cover: null,
      cover_image: null,
      cover_mode: "mosaic",
      name_source: "manual",
      track: "manual",
      tiles: [{ provider: "pixiv", external_key: "1", title: "題", author_name: "作者一", cover: ICON }],
      revision: 1,
      member_count: 3,
      available_count: 2,
      total_text_length: 1200,
      created_at: Date.UTC(2026, 0, 1),
      updated_at: Date.UTC(2026, 0, 4),
    }]);
    renderEntity("person", "#/people/pixiv/77?tab=collections");
    const tab = await screen.findByRole("tab", { name: /コレクション/ });
    expect(within(tab).getByText("1")).toBeInTheDocument();
    expect(await screen.findByText("雨の束")).toBeInTheDocument();
    expect(screen.getByText("未保存 1")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /雨の束/ })).toHaveAttribute("href", "#/collections/c-old");
    // 表紙のマスは棚のファイルの識別子から。
    expect(document.querySelector(`img[src="http://piep-library.localhost/${ICON}"]`)).not.toBeNull();
    expect(screen.getByText(/コレクションの中身は旧棚で開きます/)).toBeInTheDocument();
    for (const call of Object.values(legacy)) expect(call).not.toHaveBeenCalled();
  });

  it("shows the profile's versions and the response that made the current one", async () => {
    renderEntity("person", "#/people/pixiv/77?tab=history");
    expect(await screen.findByText("バージョン 2")).toBeInTheDocument();
    expect(screen.getByText("バージョン 1")).toBeInTheDocument();
    // 版ごとの大きさは新しい棚に無いので出さない。
    expect(screen.queryByText(/ B$|KB|MB/)).toBeNull();
    expect(await screen.findByText("4件")).toBeInTheDocument();
    expect(api.libraryV2EntityOriginal).toHaveBeenCalledWith(PERSON, SNAPSHOT);
    fireEvent.click(screen.getByRole("tab", { name: "保存データ" }));
    expect(await screen.findByText(/sampleNovelCount/)).toBeInTheDocument();
  });

  it("orders a series in its own order and shows what is missing", async () => {
    api.libraryV2Entity.mockResolvedValue(head({
      id: SERIES, kind: "series", external_key: "s1", name: "続き物", icon: null,
      cover: ICON, is_concluded: true, published_content_count: 5, work_count: 3,
    }));
    renderEntity("series", "#/series/pixiv/s1");
    expect(await screen.findByRole("heading", { name: "続き物" })).toBeInTheDocument();
    expect(screen.getByText("2話 未取得")).toBeInTheDocument();
    await waitFor(() => expect(api.libraryV2Search).toHaveBeenCalled());
    const [, query] = api.libraryV2Search.mock.calls[0];
    expect(query).toMatchObject({ sort: "series_order", descending: false, filter: { series: SERIES, person: null } });
    expect(api.libraryV2PersonSeries).not.toHaveBeenCalled();
    expect(document.querySelector(`img[src="http://piep-library.localhost/${ICON}"]`)).not.toBeNull();
  });

  it("says so when the new shelf does not have the author", async () => {
    api.libraryV2Entity.mockResolvedValue(null);
    renderEntity("person", "#/people/pixiv/missing");
    expect(await screen.findByText("新しい棚にこの作者はいません")).toBeInTheDocument();
    expect(legacy.getPerson).not.toHaveBeenCalled();
  });

  it("stays on the old shelf when the preference is off", async () => {
    window.localStorage.setItem(SHELF_V2_PREFERENCE_KEY, "false");
    legacy.getPerson.mockResolvedValue({
      id: 1, source: "pixiv", sourceKey: "77", displayName: "旧棚の作者", iconPath: null, linksJson: null,
      coverPath: null, description: null, contentHash: null, currentVersion: 1,
      lastCheckedAt: null, lastFetchedAt: null, createdAt: "", updatedAt: "", workCount: 0,
    });
    legacy.searchDownloadsV2.mockResolvedValue({ items: [], nextCursor: null, totalEstimate: 0, searchMeta: null, facetsVersion: 0 });
    legacy.listEntitySeriesPage.mockResolvedValue({ items: [], nextCursor: null, total: 0 });
    legacy.listEntityTags.mockResolvedValue([]);
    legacy.listCollectionsForPerson.mockResolvedValue([]);
    watching.get.mockResolvedValue(null);
    legacy.getLatestEntityProfileJson.mockResolvedValue({});
    renderEntity("person", "#/people/pixiv/77");
    expect(await screen.findByRole("heading", { name: "旧棚の作者" })).toBeInTheDocument();
    expect(api.libraryV2Entity).not.toHaveBeenCalled();
    expect(screen.queryByText("新しい棚（試験中）")).toBeNull();
  });
});
