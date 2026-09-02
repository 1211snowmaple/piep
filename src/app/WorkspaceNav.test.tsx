import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppFrame } from "@/app/AppFrame";
import { AppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import { writeReadingPosition } from "@/features/library/readingShelf";
import type { LibraryShelfCounts, SavedSearchRecord } from "@/types/library";
import { usePageAssist } from "@/app/PageAssistContext";

const shelfApi = vi.hoisted(() => ({
  getLibraryShelfCounts: vi.fn(),
  listSavedSearches: vi.fn(),
  upsertSavedSearch: vi.fn(),
  deleteSavedSearch: vi.fn(),
}));
const updateJobApi = vi.hoisted(() => ({
  listUpdateJobsCommand: vi.fn(),
}));

vi.mock("@/services/shelfApi", () => shelfApi);
vi.mock("@/services/updateJobApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/updateJobApi")>()),
  listUpdateJobsCommand: updateJobApi.listUpdateJobsCommand,
}));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
}));
vi.mock("@/features/search/searchIndexProgress", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/features/search/searchIndexProgress")>()),
  useSearchIndexProgress: () => null,
}));

const counts: LibraryShelfCounts = { total: 2237, favorite: 84, watched: 7, reading: 3, revised: 5 };

/** The shell rails only on a wide window, which jsdom never reports. */
let matchMediaMatches = false;
Object.defineProperty(window, "matchMedia", {
  writable: true,
  configurable: true,
  value: (query: string) => ({
    matches: matchMediaMatches,
    media: query,
    onchange: null,
    addListener: () => undefined,
    removeListener: () => undefined,
    addEventListener: () => undefined,
    removeEventListener: () => undefined,
    dispatchEvent: () => false,
  }),
});

function saved(overrides: Partial<SavedSearchRecord> = {}): SavedSearchRecord {
  return {
    id: 1,
    name: "長編ファンタジー",
    query: "tag:ファンタジー",
    paramsJson: "{}",
    createdAt: "2026-08-01T00:00:00Z",
    updatedAt: "2026-08-01T00:00:00Z",
    ...overrides,
  };
}

function renderApp(hash: string, child = <div />) {
  window.location.hash = hash;
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MantineProvider>
      <QueryClientProvider client={client}>
        <AppRouter><WorkspaceProvider><AppFrame>{child}</AppFrame></WorkspaceProvider></AppRouter>
      </QueryClientProvider>
    </MantineProvider>,
  );
}

function PageAssistFixture({ onSelect }: { onSelect: () => void }) {
  usePageAssist("test-page-assist", "この画面のAI", [{
    id: "test-action",
    label: "画面の内容をまとめる",
    description: "現在の画面だけを対象にします",
    enabled: true,
    onSelect,
  }]);
  return null;
}

function nav() {
  return within(document.getElementById("app-navigation") as HTMLElement);
}

/** A sidebar destination by its visible label. */
async function row(label: string | RegExp): Promise<HTMLElement> {
  const text = await nav().findByText(label);
  const element = text.closest(".app-nav__link");
  if (!element) throw new Error(`no sidebar row for ${String(label)}`);
  return element as HTMLElement;
}

/** The persisted fold state, which is what survives a restart. */
function foldState(): Record<string, boolean> {
  try {
    return JSON.parse(window.localStorage.getItem("piep.nav-groups.v1") ?? "{}") as Record<string, boolean>;
  } catch {
    return {};
  }
}

function rows(): HTMLElement[] {
  const root = document.getElementById("app-navigation") as HTMLElement;
  return [...root.querySelectorAll(".app-nav__link")] as HTMLElement[];
}

describe("library sidebar", () => {
  beforeEach(() => {
    window.localStorage.clear();
    shelfApi.getLibraryShelfCounts.mockResolvedValue(counts);
    shelfApi.listSavedSearches.mockResolvedValue([saved()]);
    updateJobApi.listUpdateJobsCommand.mockResolvedValue([]);
  });

  it("lists the shelves with their counts instead of a menu of screens", async () => {
    renderApp("#/library");
    expect(await nav().findByText("すべて")).toBeInTheDocument();
    await waitFor(() => expect(nav().getByText("2,237")).toBeInTheDocument());
    expect(nav().getByText("お気に入り")).toBeInTheDocument();
    expect(nav().getByText("84")).toBeInTheDocument();
    expect(nav().getByText("読みかけ")).toBeInTheDocument();
    expect(nav().getByText("改稿あり")).toBeInTheDocument();
  });

  it("marks the shelf that is actually being shown", async () => {
    renderApp("#/library?favorite=1");
    expect(await row("お気に入り")).toHaveAttribute("aria-current", "page");
    expect(await row("すべて")).not.toHaveAttribute("aria-current", "page");
  });

  it("marks no shelf once a search narrows the view", async () => {
    // The listing is no longer "all works", so saying it is would be wrong.
    renderApp("#/library?q=%E7%89%A9%E8%AA%9E");
    await row("すべて");
    rows().forEach((item) => expect(item).not.toHaveAttribute("aria-current", "page"));
  });

  it("shows saved searches", async () => {
    renderApp("#/library");
    expect(await nav().findByText("長編ファンタジー")).toBeInTheDocument();
  });

  it("says how to get one when there are none", async () => {
    shelfApi.listSavedSearches.mockResolvedValue([]);
    renderApp("#/library");
    expect(await nav().findByText("検索条件を保存すると、ここから開けます。")).toBeInTheDocument();
  });

  it("keeps an overlong saved-search name from stretching the sidebar", async () => {
    shelfApi.listSavedSearches.mockResolvedValue([saved({ name: "あ".repeat(80) })]);
    renderApp("#/library");
    // Truncation is the sidebar's job, not the name's: the row still exists and
    // is still clickable at any length.
    expect(await row("あ".repeat(80))).toBeInTheDocument();
  });

  it("carries every section at once rather than swapping them", async () => {
    // Nothing the app can do is hidden behind a control that has to be found
    // first: the groups only decide how much is unfolded.
    renderApp("#/library");
    expect(await nav().findByText("すべて")).toBeInTheDocument();
    expect(nav().getByText("ライブラリ")).toBeInTheDocument();
    expect(nav().getByText("保存")).toBeInTheDocument();
    expect(nav().getByText("書き出し")).toBeInTheDocument();
  });

  it("uses native buttons for destinations and folding controls", async () => {
    const user = userEvent.setup();
    renderApp("#/library");

    const home = await nav().findByRole("button", { name: "ホーム" });
    expect(home).toHaveAttribute("type", "button");
    home.focus();
    await user.keyboard("{Enter}");
    await waitFor(() => expect(window.location.hash).toBe("#/"));

    const collect = nav().getByRole("button", { name: "保存" });
    expect(collect).toHaveAttribute("aria-expanded", "false");
    collect.focus();
    await user.keyboard("{Enter}");
    expect(collect).toHaveAttribute("aria-expanded", "true");
  });

  it("unfolds a folded group without navigating anywhere", async () => {
    const user = userEvent.setup();
    renderApp("#/library");
    const heading = (await nav().findByText("保存")).closest(".app-nav__group") as HTMLElement;
    expect(foldState().collect).toBe(false);

    await user.click(heading);

    await waitFor(() => expect(foldState().collect).toBe(true));
    // Folding is tidying, not travelling.
    expect(window.location.hash).toBe("#/library");
  });

  it("remembers which groups were left folded", async () => {
    const user = userEvent.setup();
    const first = renderApp("#/library");
    const heading = (await nav().findByText("書き出し")).closest(".app-nav__group") as HTMLElement;
    await user.click(heading);
    await waitFor(() => expect(foldState().export).toBe(true));
    first.unmount();

    // A sidebar that refolds itself on every visit is a sidebar being fought.
    renderApp("#/library");
    await nav().findByText("書き出し");
    expect(foldState().export).toBe(true);
  });

  it("uses each service's own mark rather than a generic icon", async () => {
    renderApp("#/save/pixiv");
    expect((await row("pixivから保存")).querySelector(".provider-mark__icon")).toBeTruthy();
    expect((await row("FANBOXから保存")).querySelector(".provider-mark__icon")).toBeTruthy();
  });

  it("keeps activity and settings reachable from every screen", async () => {
    for (const hash of ["#/library", "#/save/pixiv", "#/epub", "#/settings"]) {
      const view = renderApp(hash);
      expect(await nav().findByText("アクティビティ")).toBeInTheDocument();
      expect(nav().getByText("設定")).toBeInTheDocument();
      view.unmount();
    }
  });

  it("hosts one page-scoped AI entry in the persistent header", async () => {
    const onSelect = vi.fn();
    renderApp("#/library", <PageAssistFixture onSelect={onSelect} />);

    const launcher = await screen.findByRole("button", { name: "この画面のAI" });
    const search = screen.getByRole("button", { name: "検索または移動" });
    expect(search.compareDocumentPosition(launcher) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(launcher).toHaveAttribute("data-placement", "header");
    expect(launcher).toHaveAttribute("data-assist-ready", "true");
    expect(launcher.querySelector("svg")).toHaveAttribute("fill", "none");
    fireEvent.click(launcher);
    fireEvent.click(await screen.findByText("画面の内容をまとめる"));
    expect(onSelect).toHaveBeenCalledOnce();
  });

  it("fills the header star only when no page AI action is available", async () => {
    renderApp("#/library");
    const launcher = await screen.findByRole("button", { name: "この画面で使えるAIの手伝い" });
    expect(launcher).not.toHaveAttribute("data-assist-ready");
    expect(launcher.querySelector("svg")).toHaveAttribute("fill", "currentColor");
  });

  it("counts an automatic backend update in the activity badge", async () => {
    updateJobApi.listUpdateJobsCommand.mockResolvedValue([
      {
        jobId: "automatic-1",
        status: "running",
        scope: "all",
        mode: "check_only",
        totals: 12,
        processed: 3,
        candidateCount: 0,
        savedCount: 0,
        errorCount: 0,
        activeLabel: "更新を確認しています",
        startedAt: "2026-08-29T00:00:00Z",
        updatedAt: "2026-08-29T00:01:00Z",
        finishedAt: null,
      },
    ]);
    renderApp("#/library");

    const activity = await row("アクティビティ");
    await waitFor(() =>
      expect(within(activity).getByText("1")).toBeInTheDocument(),
    );
  });

  it("navigates to a shelf when it is chosen", async () => {
    renderApp("#/library");
    fireEvent.click(await row("改稿あり"));
    await waitFor(() => expect(window.location.hash).toContain("revised=1"));
  });

  it("opens a saved search by id rather than by copying its conditions into the link", async () => {
    renderApp("#/library");
    fireEvent.click(await row("長編ファンタジー"));
    await waitFor(() => expect(window.location.hash).toContain("saved=1"));
  });

  it("still renders when the library cannot be reached", async () => {
    shelfApi.getLibraryShelfCounts.mockRejectedValue(new Error("database is busy"));
    shelfApi.listSavedSearches.mockRejectedValue(new Error("database is busy"));
    renderApp("#/library");
    // The shelves are still navigable; only their counts are unknown.
    expect(await nav().findByText("すべて")).toBeInTheDocument();
    expect(nav().queryByText("2,237")).toBeNull();
  });

  it("keeps working with an empty library", async () => {
    shelfApi.getLibraryShelfCounts.mockResolvedValue({ total: 0, favorite: 0, watched: 0, reading: 0 });
    shelfApi.listSavedSearches.mockResolvedValue([]);
    renderApp("#/library");
    await waitFor(() => expect(nav().getAllByText("0").length).toBeGreaterThan(0));
    // Zero is a fact worth stating; a blank where a number goes is not.
    expect(await row("すべて")).toHaveAttribute("aria-current", "page");
  });

  it("refreshes the persistent reading shelf when the reader records progress", async () => {
    renderApp("#/library");
    await waitFor(() => expect(shelfApi.getLibraryShelfCounts).toHaveBeenCalledWith([]));

    writeReadingPosition(42, null, { page: 2, top: 0 });

    await waitFor(() => expect(shelfApi.getLibraryShelfCounts).toHaveBeenCalledWith([42]));
  });
});

/**
 * The collapsed rail is 62px wide: no labels fit, and a list of saved searches
 * certainly does not.
 *
 * jsdom reports every media query as unmatched, so the shell would never
 * consider the window wide enough to rail. The width query is answered here
 * instead, which keeps the rail inside the real shell rather than testing a
 * detached copy of the sidebar.
 */
describe("collapsed rail", () => {
  beforeEach(() => {
    window.localStorage.clear();
    window.localStorage.setItem("piep.nav-railed", "true");
    matchMediaMatches = true;
    shelfApi.getLibraryShelfCounts.mockResolvedValue(counts);
    shelfApi.listSavedSearches.mockResolvedValue([saved(), saved({ id: 2, name: "青葉しおり 全作" })]);
  });
  afterEach(() => { matchMediaMatches = false; });

  const renderRail = renderApp;

  it("drops the labels but keeps every shelf reachable and named", async () => {
    renderRail("#/library");
    await waitFor(() => expect(nav().queryByText("すべて")).toBeNull());
    // The name survives for assistive technology and for the tooltip.
    expect(nav().getByLabelText("すべて")).toBeInTheDocument();
    expect(nav().getByLabelText("お気に入り")).toBeInTheDocument();
    expect(nav().getByLabelText("読みかけ")).toBeInTheDocument();
    expect(nav().getByLabelText("改稿あり")).toBeInTheDocument();
  });

  it("keeps every saved search one click away, told apart by its initial", async () => {
    renderRail("#/library");
    // Each keeps its name for assistive technology and the tooltip; the visible
    // mark is its own first character, so they are not four identical icons.
    const fantasy = await nav().findByLabelText("長編ファンタジー");
    expect(nav().getByLabelText("青葉しおり 全作")).toBeInTheDocument();
    expect(fantasy.textContent).toBe("長");
    expect(nav().getByLabelText("青葉しおり 全作").textContent).toBe("青");

    fireEvent.click(fantasy);
    await waitFor(() => expect(window.location.hash).toContain("saved=1"));
  });

  it("shows nothing where the saved searches would be when there are none", async () => {
    shelfApi.listSavedSearches.mockResolvedValue([]);
    renderRail("#/library");
    await waitFor(() => expect(nav().getByLabelText("すべて")).toBeInTheDocument());
    // The rail has no room for an explanation, so it simply says nothing.
    expect(nav().queryByText("検索条件を保存すると、ここから開けます。")).toBeNull();
  });

});

