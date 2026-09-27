import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it } from "vitest";
import { AppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import { theme } from "@/theme";
import { searchDemoWorks } from "@/mocks/demoData";
import type { SearchV2Params } from "@/types/library";
import LibraryPage, { parseSavedParams, resolveSortBy, rollbackWorkFlag, searchSuggestionAction, updateWorkFlag } from "./LibraryPage";

describe("LibraryPage search", () => {
  beforeAll(() => {
    Element.prototype.scrollIntoView = () => {};
    const values = new Map<string, string>();
    Object.defineProperty(window, "localStorage", {
      configurable: true,
      value: {
        get length() { return values.size; },
        clear: () => values.clear(),
        getItem: (key: string) => values.get(key) ?? null,
        key: (index: number) => [...values.keys()][index] ?? null,
        removeItem: (key: string) => { values.delete(key); },
        setItem: (key: string, value: string) => { values.set(key, value); },
      } satisfies Storage,
    });
  });

  beforeEach(() => {
    window.localStorage.clear();
    window.location.hash = "#/library";
  });

  it("keeps the input mounted through Japanese IME composition and URL sync", async () => {
    window.location.hash = "#/library";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);
    const input = await screen.findByLabelText("ライブラリを検索");
    fireEvent.compositionStart(input);
    fireEvent.change(input, { target: { value: "にほ" } });
    fireEvent.change(input, { target: { value: "日本" } });
    fireEvent.compositionEnd(input, { data: "日本" });
    expect(input).toHaveValue("日本");
    await waitFor(() => expect(window.location.hash).toContain("q=%E6%97%A5%E6%9C%AC"), { timeout: 1500 });
    expect(screen.getByLabelText("ライブラリを検索")).toBe(input);
    // Searching defaults to relevance, but the control stays usable so the
    // results can be reordered by any library column.
    const sort = screen.getByRole("combobox", { name: "並び順" });
    expect(sort).toBeEnabled();
    expect(sort).toHaveValue("関連度：高い順");
  });

  /**
   * 検索欄の Enter が無反応だった。候補が無いときの案内は「Enterで全文検索」と
   * 言っていたのに、**受ける処理が一行も無かった**。しかも押さなくても既に
   * 全文検索は走っている（入力は即座に反映される）ので、案内も二重に嘘だった。
   *
   * 変換の確定で飛んでくる Enter は数えない。数えると、日本語を打っている
   * 途中で候補が閉じる。
   */
  it("answers Enter by closing the suggestions instead of doing nothing", async () => {
    window.location.hash = "#/library";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);
    const input = await screen.findByLabelText("ライブラリを検索");
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: "モテモテのハーレム" } });
    // 開いているかどうかは `aria-expanded` に出る。候補の中身は非同期で
    // 変わるし、閉じても節点は DOM に残るので、そこでは判定できない。
    await waitFor(() => expect(input).toHaveAttribute("aria-expanded", "true"));

    // 変換の確定で飛ぶ Enter は数えない。数えると打っている途中で閉じる。
    fireEvent.keyDown(input, { key: "Enter", isComposing: true });
    expect(input).toHaveAttribute("aria-expanded", "true");

    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(input).toHaveAttribute("aria-expanded", "false"));
    expect(input).toHaveValue("モテモテのハーレム");
  });

  it("keeps a column sort selectable during a search and drops it when the query is cleared", async () => {
    window.location.hash = "#/library?q=%E6%97%A5%E6%9C%AC&sort=title";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    const sort = await screen.findByRole("combobox", { name: "並び順" });
    expect(sort).toHaveValue("タイトル：昇順（あ→ん）");

    const input = screen.getByLabelText("ライブラリを検索");
    fireEvent.change(input, { target: { value: "" } });
    await waitFor(() => expect(window.location.hash).not.toContain("q="), { timeout: 1500 });
    // Relevance has no meaning without a query, but an explicit column sort does.
    expect(screen.getByRole("combobox", { name: "並び順" })).toHaveValue("タイトル：昇順（あ→ん）");
  });

  it("falls back from relevance to the saved-date order when the query is cleared", async () => {
    window.location.hash = "#/library?q=%E6%97%A5%E6%9C%AC";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    expect(await screen.findByRole("combobox", { name: "並び順" })).toHaveValue("関連度：高い順");
    fireEvent.change(screen.getByLabelText("ライブラリを検索"), { target: { value: "" } });
    await waitFor(() => expect(window.location.hash).not.toContain("q="), { timeout: 1500 });
    expect(screen.getByRole("combobox", { name: "並び順" })).toHaveValue("保存日：新しい順");
  });

  it("offers every useful work sort in both directions and sends the chosen order to search", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    const sort = await screen.findByRole("combobox", { name: "並び順" });
    fireEvent.click(sort);
    expect(await screen.findByRole("option", { name: "公開日：古い順", hidden: true })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "版番号：大きい順", hidden: true })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "容量：小さい順", hidden: true })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: "添付数：少ない順", hidden: true }));

    await waitFor(() => {
      const query = new URLSearchParams(window.location.hash.split("?")[1] ?? "");
      expect(query.get("sort")).toBe("asset_count");
      expect(query.get("order")).toBe("asc");
    });
    await waitFor(() => {
      const query = client.getQueryCache().findAll({ queryKey: ["library"] })
        .find((item) => {
          const params = item.queryKey[1] as SearchV2Params | undefined;
          return params?.sortBy === "asset_count" && params.sortOrder === "asc";
        });
      expect(query).toBeDefined();
    });
  });

  it("lets an explicit shared sort keep its natural direction over this device's preference", async () => {
    window.localStorage.setItem("piep.library-sort", JSON.stringify("title"));
    window.localStorage.setItem("piep.library-sort-order", JSON.stringify("desc"));
    window.location.hash = "#/library?sort=title";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    expect(await screen.findByRole("combobox", { name: "並び順" })).toHaveValue("タイトル：昇順（あ→ん）");
  });

  it("offers both directions for collection sorting and records the direction in the URL", async () => {
    window.location.hash = "#/library?tab=collections";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    const sort = await screen.findByRole("combobox", { name: "並び順" });
    fireEvent.click(sort);
    expect(await screen.findByRole("option", { name: "作成日：古い順", hidden: true })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "名前：降順（ん→あ）", hidden: true })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "更新回数：多い順", hidden: true })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: "登録作品数：少ない順", hidden: true }));

    await waitFor(() => {
      const query = new URLSearchParams(window.location.hash.split("?")[1] ?? "");
      expect(query.get("csort")).toBe("member_count");
      expect(query.get("corder")).toBe("asc");
    });
    expect(screen.getByRole("button", { name: "保存した検索" })).toBeVisible();
  });

  it("restores a saved collection ordering and clears hidden sorts from other tabs", async () => {
    window.location.hash = "#/library?tab=people&saved=41&ewatch=watched&emin=8&edone=1&esort=name&eorder=asc&csort=name&corder=asc&sort=title&order=desc";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(["saved-searches"], [{
      id: 41,
      name: "更新の多いコレクション",
      query: null,
      paramsJson: JSON.stringify({
        tab: "collections",
        filters: {},
        collectionSortBy: "revision",
        collectionSortOrder: "asc",
      }),
      createdAt: "2026-01-01T00:00:00Z",
      updatedAt: "2026-01-01T00:00:00Z",
    }]);
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    await waitFor(() => {
      const query = new URLSearchParams(window.location.hash.split("?")[1] ?? "");
      expect(query.get("tab")).toBe("collections");
      expect(query.get("csort")).toBe("revision");
      expect(query.get("corder")).toBe("asc");
      expect(query.has("esort")).toBe(false);
      expect(query.has("eorder")).toBe(false);
      expect(query.has("ewatch")).toBe(false);
      expect(query.has("emin")).toBe(false);
      expect(query.has("edone")).toBe(false);
      expect(query.has("sort")).toBe(false);
      expect(query.has("order")).toBe(false);
    });
    expect(screen.getByRole("combobox", { name: "並び順" })).toHaveValue("更新回数：少ない順");
  });

  it("restores a saved membership shelf exactly and clears the shelf it replaces", async () => {
    window.location.hash = "#/library?saved=43&revised=1";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(["saved-searches"], [{
      id: 43,
      name: "読みかけの長編",
      query: null,
      paramsJson: JSON.stringify({
        tab: "works",
        filters: { minChars: 10_000 },
        membershipShelf: "reading",
      }),
      createdAt: "2026-01-01T00:00:00Z",
      updatedAt: "2026-01-01T00:00:00Z",
    }]);
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    await waitFor(() => {
      const query = new URLSearchParams(window.location.hash.split("?")[1] ?? "");
      expect(query.get("shelf")).toBe("reading");
      expect(query.has("revised")).toBe(false);
      expect(query.get("minchars")).toBe("10000");
      expect(query.get("saved")).toBe("43");
    });
  });

  it("does not advertise retained work or series-only filters on collections and people", async () => {
    window.location.hash = "#/library?tab=collections&favorite=1&edone=1";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const view = render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);
    await screen.findByRole("combobox", { name: "並び順" });
    expect(screen.queryByText("適用中")).not.toBeInTheDocument();

    view.unmount();
    window.location.hash = "#/library?tab=people&edone=1";
    render(<MantineProvider><QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);
    await screen.findByRole("combobox", { name: "並び順" });
    expect(screen.queryByText("適用中")).not.toBeInTheDocument();
  });

  it("restores saved entity scope and ordering without carrying a collection sort", async () => {
    window.location.hash = "#/library?tab=series&saved=42&csort=name&corder=desc&esort=name&eorder=desc&ewatch=watched&emin=9&edone=1";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    client.setQueryData(["saved-searches"], [{
      id: 42,
      name: "停止中の短いシリーズ",
      query: null,
      paramsJson: JSON.stringify({
        tab: "series",
        filters: {},
        entityScope: { watch: "paused", minWorkCount: 3, concluded: false },
        entitySortBy: "asset_count",
        entitySortOrder: "asc",
      }),
      createdAt: "2026-01-01T00:00:00Z",
      updatedAt: "2026-01-01T00:00:00Z",
    }]);
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    await waitFor(() => {
      const query = new URLSearchParams(window.location.hash.split("?")[1] ?? "");
      expect(query.get("tab")).toBe("series");
      expect(query.get("ewatch")).toBe("paused");
      expect(query.get("emin")).toBe("3");
      expect(query.get("edone")).toBe("0");
      expect(query.get("esort")).toBe("asset_count");
      expect(query.get("eorder")).toBe("asc");
      expect(query.has("csort")).toBe(false);
      expect(query.has("corder")).toBe(false);
    });
    expect(screen.getByRole("combobox", { name: "並び順" })).toHaveValue("合計添付数：少ない順");
  });

  it("does not fetch a hidden series listing while the collections tab is open", async () => {
    window.location.hash = "#/library?tab=collections&q=%E5%A4%9C";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    await screen.findByRole("tab", { name: "コレクション" });
    const hiddenQueries = [
      ...client.getQueryCache().findAll({ queryKey: ["library-entities"] }),
      ...client.getQueryCache().findAll({ queryKey: ["library-entity-count"] }),
      ...client.getQueryCache().findAll({ queryKey: ["update-targets", "library"] }),
    ];
    expect(hiddenQueries.every((query) => query.state.fetchStatus === "idle" && query.state.data === undefined)).toBe(true);
  });

  it("keeps the revised shelf in a loading state until its membership is known", async () => {
    window.location.hash = "#/library?revised=1";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    let finishMembership!: (value: []) => void;
    const membership = new Promise<[]>((resolve) => { finishMembership = resolve; });
    void client.fetchQuery({ queryKey: ["pending-revisions"], queryFn: () => membership });
    const view = render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);
    try {
      expect(await screen.findByText("改稿のある作品を確認しています")).toBeInTheDocument();
      expect(screen.queryByText("一致する作品がありません")).toBeNull();
    } finally {
      view.unmount();
      finishMembership([]);
    }
  });

  it("makes saved revision search an explicit URL-owned scope", async () => {
    window.location.hash = "#/library?q=%E6%94%B9%E7%A8%BF";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    expect(await screen.findByRole("button", { name: "絞り込み" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "検索対象: 現在版だけ" })).toBeNull();
    expect(screen.getByLabelText("ライブラリを検索")).toHaveAttribute("placeholder", "タイトル、作者、タグ、本文を検索");
    fireEvent.click(screen.getByRole("button", { name: "絞り込み" }));
    const spotlight = await screen.findByRole("dialog", { name: "詳細フィルター" });
    fireEvent.click(within(spotlight).getByRole("tab", { name: /^状態/ }));
    fireEvent.click(within(spotlight).getByRole("radio", { name: "過去版も含む" }));
    fireEvent.click(within(spotlight).getByRole("button", { name: "適用" }));

    await waitFor(() => expect(window.location.hash).toContain("versions=all"));
    expect(screen.getByText("検索対象: 過去版も")).toBeInTheDocument();
    expect(screen.getByLabelText("ライブラリを検索")).toHaveAttribute("placeholder", "現在版と過去版を検索");
    await waitFor(() => {
      const query = client.getQueryCache().findAll({ queryKey: ["library"] })
        .find((item) => (item.queryKey[1] as SearchV2Params | undefined)?.versionScope === "all");
      expect(query).toBeDefined();
    });
  });

  it("normalizes unsupported URL and persisted values instead of entering an invalid view", async () => {
    window.localStorage.setItem("piep.library-view", JSON.stringify("future-view"));
    window.localStorage.setItem("piep.saved-searches.v2", JSON.stringify({ malformed: true }));
    window.location.hash = "#/library?tab=unknown&watch=yes&sort=not-a-sort";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    expect(await screen.findByRole("tab", { name: "作品" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("combobox", { name: "並び順" })).toHaveValue("保存日：新しい順");
    expect(screen.getByRole("radiogroup", { name: "表示形式" })).toBeInTheDocument();
    expect(screen.getByLabelText("ギャラリー表示")).toBeInTheDocument();
  });

  it("does not create a second library query when only the presentation changes", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    await screen.findAllByText("雨上がりの図書室で");
    const before = client.getQueryCache().findAll({ queryKey: ["library"] })[0];
    expect(before).toBeDefined();
    const updatedAt = before?.state.dataUpdatedAt;

    fireEvent.click(screen.getByLabelText("リスト表示"));

    // 押した一覧の上書きとして残る。全体の既定はここでは動かさない -
    // 棚を行で見たいことと、束の中身まで行にしたいことは別である。
    await waitFor(() => expect(window.localStorage.getItem("piep.library-view.library-works")).toBe(JSON.stringify("compact")));
    expect(window.localStorage.getItem("piep.library-view")).toBe(JSON.stringify("gallery"));
    const libraryQueries = client.getQueryCache().findAll({ queryKey: ["library"] });
    expect(libraryQueries).toEqual([before]);
    expect(libraryQueries[0]?.state.dataUpdatedAt).toBe(updatedAt);
  });

  it("stages expensive filters until Apply and validates the URL-owned flags once", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    fireEvent.click(await screen.findByRole("button", { name: "絞り込み" }));
    fireEvent.change(await screen.findByRole("textbox", { name: "条件を検索" }), { target: { value: "お気に入り" } });
    fireEvent.click(await screen.findByRole("option", { name: /お気に入りのみ/, hidden: true }));
    expect(window.location.hash).not.toContain("favorite=1");
    fireEvent.click(screen.getByRole("button", { name: "適用" }));
    await waitFor(() => expect(window.location.hash).toContain("favorite=1"));
  });

  it("filters by an author chosen in the detail spotlight", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    fireEvent.click(await screen.findByRole("button", { name: "絞り込み" }));
    const spotlight = await screen.findByRole("dialog", { name: "詳細フィルター" });
    await waitFor(() => expect(client.getQueryData(["library-facets"])).toBeDefined());
    fireEvent.change(within(spotlight).getByRole("textbox", { name: "条件を検索" }), { target: { value: "青葉" } });
    // jsdom cannot position the non-portaled dropdown, so Mantine keeps its
    // options hidden even though the same store is open in a real window.
    const authorOptions = await screen.findAllByRole("option", { name: /青葉しおり/, hidden: true });
    expect(within(authorOptions[0]).getByLabelText("pixiv")).toBeInTheDocument();
    expect(within(authorOptions[0]).getByLabelText("FANBOX")).toBeInTheDocument();
    expect(within(authorOptions[0]).getByText("青葉しおり")).toHaveClass("filter-spotlight__facet-name");
    expect(authorOptions[0].querySelector(".line-clamp-1")).toBeNull();
    fireEvent.click(authorOptions[0]);

    fireEvent.click(within(spotlight).getByRole("button", { name: "適用" }));

    await waitFor(() => {
      const query = window.location.hash.split("?")[1] ?? "";
      expect(new URLSearchParams(query).getAll("author")).toEqual(["青葉しおり"]);
    });
    expect((await screen.findAllByText("作者: 青葉しおり")).length).toBeGreaterThan(0);
  });

  it("names the page and every button in the filter spotlight", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider theme={theme}><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    expect(await screen.findByRole("heading", { level: 1, name: "ライブラリ" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "絞り込み" }));
    const spotlight = await screen.findByRole("dialog", { name: "詳細フィルター" });
    expect(within(spotlight).getByRole("button", { name: "閉じる" })).toBeInTheDocument();
    within(spotlight).getAllByRole("button").forEach((button) => expect(button).toHaveAccessibleName());
  });

  it("keeps every filter group reachable from the spotlight summary", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    fireEvent.click(await screen.findByRole("button", { name: "絞り込み" }));
    const spotlight = await screen.findByRole("dialog", { name: "詳細フィルター" });
    expect(within(spotlight).getByRole("textbox", { name: "条件を検索" })).toBeInTheDocument();
    expect(within(spotlight).queryByText("作者・タグは名前の一致で検索")).toBeNull();
    expect(within(spotlight).queryByText(/選択済みの条件は下部に表示/)).toBeNull();
    const sectionTabs = within(spotlight).getByRole("tablist", { name: "フィルターの種類" });
    expect(within(sectionTabs).getByRole("tab", { name: /^基本/ })).toBeInTheDocument();
    expect(within(sectionTabs).getByRole("tab", { name: /^状態/ })).toBeInTheDocument();
    expect(within(sectionTabs).getByRole("tab", { name: /^作者/ })).toHaveAttribute("aria-selected", "true");
    expect(within(sectionTabs).getByRole("tab", { name: /^タグ/ })).toBeInTheDocument();
    expect(within(sectionTabs).getByRole("tab", { name: /^日付/ })).toBeInTheDocument();
    expect(within(sectionTabs).getByRole("tab", { name: /^内容・ファイル/ })).toBeInTheDocument();

    fireEvent.click(within(sectionTabs).getByRole("tab", { name: /^基本/ }));
    expect(within(spotlight).getByRole("checkbox", { name: "pixiv" })).toBeInTheDocument();
    fireEvent.click(within(sectionTabs).getByRole("tab", { name: /^状態/ }));
    expect(within(spotlight).getByRole("radiogroup", { name: "全文検索の対象" })).toBeInTheDocument();
    expect(within(spotlight).getByRole("combobox", { name: "改稿状態" })).toBeInTheDocument();
    expect(within(spotlight).getByRole("combobox", { name: "ローカル編集" })).toBeInTheDocument();
    expect(within(spotlight).getByRole("combobox", { name: "表紙" })).toBeInTheDocument();
    fireEvent.click(within(sectionTabs).getByRole("tab", { name: /^タグ/ }));
    expect(within(spotlight).getAllByRole("button", { name: /を含めるタグへ追加/ }).length).toBeGreaterThan(0);
    expect(within(spotlight).getByRole("radiogroup", { name: "複数タグの条件" })).toBeInTheDocument();
    expect(within(spotlight).queryByRole("radiogroup", { name: "検索結果の追加先" })).toBeNull();
    fireEvent.click(within(sectionTabs).getByRole("tab", { name: /^日付/ }));
    expect(within(spotlight).getByLabelText("開始日")).toBeInTheDocument();
    fireEvent.click(within(sectionTabs).getByRole("tab", { name: /^内容・ファイル/ }));
    expect(within(spotlight).getByRole("textbox", { name: "最小文字数" })).toBeInTheDocument();
    expect(within(spotlight).getByRole("combobox", { name: "添付ファイル" })).toBeInTheDocument();
    expect(within(spotlight).getByRole("textbox", { name: "最小添付数" })).toBeInTheDocument();
    expect(within(spotlight).getByRole("textbox", { name: "最小容量" })).toBeInTheDocument();
    expect(within(spotlight).queryByText(/本文の意味が似ている/)).toBeNull();
  });

  it("keeps included and excluded facets visible while moving between filter sections", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    fireEvent.click(await screen.findByRole("button", { name: "絞り込み" }));
    const spotlight = await screen.findByRole("dialog", { name: "詳細フィルター" });
    fireEvent.click(await within(spotlight).findByRole("button", { name: "青葉しおりを含める作者へ追加" }));
    fireEvent.click(within(spotlight).getByRole("button", { name: "遠野つむぎを除外する作者へ追加" }));
    fireEvent.click(within(spotlight).getByRole("tab", { name: /^内容・ファイル/ }));

    const included = within(spotlight).getByRole("region", { name: "含める条件" });
    const excluded = within(spotlight).getByRole("region", { name: "除外する条件" });
    expect(within(included).getByText("作者: 青葉しおり")).toBeInTheDocument();
    expect(within(excluded).getByText("作者: 遠野つむぎ")).toBeInTheDocument();

    fireEvent.click(within(included).getByRole("button", { name: "作者: 青葉しおりを解除" }));
    expect(within(included).queryByText("作者: 青葉しおり")).toBeNull();
    expect(within(excluded).getByText("作者: 遠野つむぎ")).toBeInTheDocument();
  });

  it("offers the paging switch beside the count, not only past the end", async () => {
    // With scrolling turned on and a few thousand works there is no end of the
    // list to reach, so controls that live only under it cannot be got back to.
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    // Both states are visible at once and the one in force is marked, so the
    // way back out of a mode never depends on reaching the end of the list.
    const toggle = await screen.findByRole("radiogroup", { name: "一覧の読み込み方" });
    expect(within(toggle).getByRole("radio", { name: /自動/ })).toBeChecked();
    expect(within(toggle).getByRole("radio", { name: /ページ番号/ })).toBeInTheDocument();
  });

  it("keeps every filter in the address so it survives leaving the page", async () => {
    // Opening a work unmounts the library. Anything the drawer had put in
    // component state was gone by the time the back button brought it back,
    // which read as the app having silently dropped the narrowing.
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    fireEvent.click(await screen.findByRole("button", { name: "絞り込み" }));
    const spotlight = await screen.findByRole("dialog", { name: "詳細フィルター" });
    fireEvent.click(within(spotlight).getByRole("tab", { name: /^基本/ }));
    fireEvent.click(await screen.findByRole("checkbox", { name: "pixiv" }));
    fireEvent.click(within(spotlight).getByRole("tab", { name: /^内容・ファイル/ }));
    fireEvent.change(screen.getByRole("textbox", { name: "最小文字数" }), { target: { value: "5000" } });
    fireEvent.click(screen.getByRole("button", { name: "適用" }));

    await waitFor(() => expect(window.location.hash).toContain("source=pixiv"));
    expect(window.location.hash).toContain("minchars=5000");
  });

  it("restores the drawer's filters when history returns to them", async () => {
    window.location.hash = "#/library?source=pixiv&tag=%E5%89%B5%E4%BD%9C&minchars=5000&minassets=2&maxassets=20&minsizemb=1.5&maxsizemb=50&assets=has_images&seriesmode=in_series&revision=revised&localedit=edited&cover=has_cover&versions=all&datefield=source_updated_at&datefrom=2026-01-01&dateto=2026-09-01";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    expect(await screen.findByText("適用中")).toBeInTheDocument();
    expect(screen.getByText("#創作").closest(".filter-token")).toBeInTheDocument();
    expect(screen.getByText("検索対象: 過去版も")).toBeInTheDocument();
    expect(screen.getByText("改稿あり")).toBeInTheDocument();
    expect(screen.getByText("ローカル編集あり")).toBeInTheDocument();
    expect(screen.getByText("表紙あり")).toBeInTheDocument();
    await waitFor(() => {
      const query = client.getQueryCache().findAll({ queryKey: ["library"] })
        .find((item) => {
          const params = item.queryKey[1] as SearchV2Params | undefined;
          return params?.revisionFilter === "revised"
            && params.editFilter === "edited"
            && params.coverFilter === "has_cover"
            && params.minAssetCount === 2
            && params.maxAssetCount === 20
            && params.minFileSizeBytes === Math.round(1.5 * 1024 * 1024)
            && params.maxFileSizeBytes === 50 * 1024 * 1024;
        });
      expect(query).toBeDefined();
    });
    // And the drawer opens onto the conditions actually in force, rather than
    // an empty form that disagrees with the results behind it.
    fireEvent.click(screen.getByRole("button", { name: "絞り込み" }));
    const spotlight = await screen.findByRole("dialog", { name: "詳細フィルター" });
    fireEvent.click(within(spotlight).getByRole("tab", { name: /^基本/ }));
    expect(within(spotlight).getByRole("checkbox", { name: "pixiv" })).toBeChecked();
    fireEvent.click(within(spotlight).getByRole("tab", { name: /^状態/ }));
    expect(within(spotlight).getByRole("radio", { name: "過去版も含む" })).toBeChecked();
    expect(within(spotlight).getByRole("combobox", { name: "改稿状態" })).toHaveValue("改稿あり");
    expect(within(spotlight).getByRole("combobox", { name: "ローカル編集" })).toHaveValue("編集あり");
    expect(within(spotlight).getByRole("combobox", { name: "表紙" })).toHaveValue("表紙あり");
    fireEvent.click(within(spotlight).getByRole("tab", { name: /^タグ/ }));
    expect(within(within(spotlight).getByRole("region", { name: "含める条件" })).getByText("#創作")).toBeInTheDocument();
    fireEvent.click(within(spotlight).getByRole("tab", { name: /^日付/ }));
    expect(within(spotlight).getByRole("radio", { name: "更新日" })).toBeChecked();
    expect(within(spotlight).getByLabelText("開始日")).toHaveValue("2026-01-01");
    expect(within(spotlight).getByLabelText("終了日")).toHaveValue("2026-09-01");
    fireEvent.click(within(spotlight).getByRole("tab", { name: /^内容・ファイル/ }));
    expect(within(spotlight).getByRole("textbox", { name: "最小文字数" })).toHaveValue("5,000");
    expect(within(spotlight).getByRole("textbox", { name: "最小添付数" })).toHaveValue("2");
    expect(within(spotlight).getByRole("textbox", { name: "最大添付数" })).toHaveValue("20");
    expect(within(spotlight).getByRole("textbox", { name: "最小容量" })).toHaveValue("1.5 MB");
    expect(within(spotlight).getByRole("textbox", { name: "最大容量" })).toHaveValue("50 MB");
    expect(within(spotlight).getByRole("combobox", { name: "シリーズ" })).toHaveValue("シリーズ作品");
    expect(within(spotlight).getByRole("combobox", { name: "添付ファイル" })).toHaveValue("画像あり");
  });

  it("clears URL-owned watch filters during history navigation", async () => {
    window.location.hash = "#/library?watch=watched";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);
    expect(await screen.findByText("適用中")).toBeInTheDocument();

    window.location.hash = "#/library";
    window.dispatchEvent(new HashChangeEvent("hashchange"));
    await waitFor(() => expect(screen.queryByText("適用中")).toBeNull());
  });
});

describe("search suggestion actions", () => {
  it("navigates entity suggestions instead of searching their internal ids", () => {
    expect(searchSuggestionAction({ kind: "series", label: "季節の栞", value: "pixiv:12552619", source: "pixiv", sourceKey: "12552619" }))
      .toEqual({ kind: "navigate", target: "/series/pixiv/12552619" });
    expect(searchSuggestionAction({ kind: "author", label: "作者名", value: "作者名", source: "pixiv", sourceKey: "creator/42" }))
      .toEqual({ kind: "navigate", target: "/people/pixiv/creator%2F42" });
  });

  it("turns non-entity suggestions into explicit filters", () => {
    expect(searchSuggestionAction({ kind: "tag", label: "長編 小説", value: "長編 小説" }))
      .toEqual({ kind: "query", query: "tag:\"長編 小説\"" });
  });
});

describe("library optimistic flag rollback", () => {
  it("reverts only the failed work and preserves a later successful mutation", () => {
    const result = searchDemoWorks();
    const first = { ...result.items[0], favorite: true, watchUpdates: true };
    const second = { ...result.items[1], favorite: true, watchUpdates: true };
    const initial = {
      pages: [{ ...result, items: [first, second], totalEstimate: 2, searchMeta: { ...result.searchMeta, totalEstimate: 2 } }],
      pageParams: [null],
    };
    const params: SearchV2Params = { favorite: true };

    // The first operation removes its row from this filtered listing. A second
    // work then succeeds before the first request reports failure.
    const afterFirst = updateWorkFlag(initial, first.id, { favorite: false }, params);
    const afterSecond = updateWorkFlag(afterFirst, second.id, { watchUpdates: false }, params);
    const rolledBack = rollbackWorkFlag(afterSecond, initial, first.id, "favorite", params);

    expect(rolledBack?.pages[0].items.map((item) => item.id)).toEqual([first.id, second.id]);
    expect(rolledBack?.pages[0].items.find((item) => item.id === first.id)?.favorite).toBe(true);
    expect(rolledBack?.pages[0].items.find((item) => item.id === second.id)?.watchUpdates).toBe(false);
    expect(rolledBack?.pages[0].totalEstimate).toBe(2);
  });

  it("normalizes a pasted deep page and explains how to continue without a deep OFFSET", async () => {
    window.localStorage.setItem("piep.paging-mode", JSON.stringify("pages"));
    window.location.hash = "#/library?page=999999";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    await waitFor(() => expect(window.location.hash).toContain("page=251"));
    const notice = await screen.findByRole("status");
    expect(notice).toHaveTextContent("直接開けるのは251ページ目まで");
    expect(notice).toHaveTextContent("自動");
  });
});

describe("remembered library order", () => {
  // 並び順は「この人の見かた」。アプリを閉じても続くべき設定として扱う。
  it("uses the remembered order when the address says nothing", () => {
    expect(resolveSortBy(null, false, "title")).toBe("title");
    expect(resolveSortBy(null, false)).toBe("downloaded_at");
  });

  it("lets the address win, so a link opens what it says", () => {
    expect(resolveSortBy("text_length", false, "title")).toBe("text_length");
  });

  // 検索したときの既定は関連度。覚えた順序で上書きしない。
  it("keeps relevance as the default for a search", () => {
    expect(resolveSortBy(null, true, "title")).toBe("relevance");
    expect(resolveSortBy("title", true, "downloaded_at")).toBe("title");
  });

  // 関連度は検索の外では意味を持たないので、覚えていても使わない。
  it("never falls back to relevance without a query", () => {
    expect(resolveSortBy(null, false, "relevance")).toBe("downloaded_at");
    expect(resolveSortBy("relevance", false, "title")).toBe("title");
  });

  // 保存先は手で書き換えられるし、並び順の名前は版で変わりうる。
  it("falls back when the stored value is not an order any more", () => {
    expect(resolveSortBy(null, false, "nonsense" as never)).toBe("downloaded_at");
  });
});

describe("library entity paging", () => {
  beforeEach(() => {
    window.localStorage.clear();
    // Small enough that the six demo authors do not fit on one page.
    window.localStorage.setItem("piep.page-size", JSON.stringify(5));
    window.location.hash = "#/library?tab=people";
  });

  // 作者とシリーズは一つの鍵を分け合っていたので、作者を名前順にすると
  // シリーズまで名前順になった。並びを選ぶのは「何を探しているか」であって、
  // 二つのタブで同じとは限らない。
  it("remembers the creator and series orders apart from each other", async () => {
    window.localStorage.setItem("piep.library-entity-sort.person", JSON.stringify("name"));
    window.localStorage.setItem("piep.library-entity-sort.series", JSON.stringify("work_count"));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    expect(await screen.findByDisplayValue("名前：昇順（あ→ん）")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: /シリーズ/ }));
    // 住所に残った esort を連れて行かない - 覚えてある方の並びで開く。
    expect(await screen.findByDisplayValue("作品数：多い順")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: /作者/ }));
    expect(await screen.findByDisplayValue("名前：昇順（あ→ん）")).toBeInTheDocument();
  });

  // 古い鍵で選んであった並びは、そのまま引き継ぐ。黙って既定へ戻さない。
  it("carries the order chosen before the two tabs were split", async () => {
    window.localStorage.setItem("piep.library-entity-sort", JSON.stringify("downloaded_at"));
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    expect(await screen.findByDisplayValue("保存日：新しい順")).toBeInTheDocument();
  });

  it("applies ascending and descending directions to grouped entity lists", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    const sort = await screen.findByRole("combobox", { name: "並び順" });
    fireEvent.click(sort);
    expect(await screen.findByRole("option", { name: "合計文字数：多い順", hidden: true })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "改稿回数：少ない順", hidden: true })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("option", { name: "作品数：少ない順", hidden: true }));
    await waitFor(() => {
      const query = new URLSearchParams(window.location.hash.split("?")[1] ?? "");
      expect(query.get("esort")).toBe("work_count");
      expect(query.get("eorder")).toBe("asc");
    });
    await waitFor(() => expect(client.getQueryCache().findAll({ queryKey: ["library-entities"] })
      .some((query) => query.queryKey.includes("asc"))).toBe(true));
  });

  // The drawer is on screen for every tab, so it has to mean something on
  // every tab. It used to narrow the works and leave the creators untouched.
  it("narrows the creator tab by the library filter", async () => {
    window.location.hash = "#/library?tab=people&source=fanbox";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    expect(await screen.findByLabelText("mizu atelierを開く")).toBeInTheDocument();
    expect(screen.getByLabelText("こはるデザイン室を開く")).toBeInTheDocument();
    expect(screen.queryByLabelText("青葉しおりを開く")).not.toBeInTheDocument();
  });

  // Scrolling and page one both start at offset zero, so the two modes used to
  // share a cache entry: turning on page numbers after a long scroll kept every
  // row that had been loaded and called it page one.
  it("starts numbered paging at the first page rather than keeping the scrolled rows", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    expect(await screen.findByLabelText("青葉しおりを開く")).toBeInTheDocument();
    expect(screen.queryByLabelText("七瀬あかりを開く")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /さらに読み込む/ }));
    expect(await screen.findByLabelText("七瀬あかりを開く")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("radio", { name: "ページ番号" }));
    await waitFor(() => expect(screen.queryByLabelText("七瀬あかりを開く")).not.toBeInTheDocument());
    expect(screen.getByLabelText("青葉しおりを開く")).toBeInTheDocument();
  });
});

/**
 * 保存した検索は、条件だけでなく**検索の種類**も覚えていなければならない。
 *
 * 「言葉で探す」で作った検索を保存し、サイドバーから開き直すと、意味検索が
 * 字面検索に落ちていた。しかも検索欄に残っているのは利用者が打った言葉では
 * なくモデルの言い換えなので、開き直した結果は元の検索とも、素直な字面検索とも
 * 違うものになる。いちばん静かな壊れ方だった。
 */
describe("saved search parameters", () => {
  it("remembers that a search was a semantic one", () => {
    const saved = JSON.stringify({ tab: "works", filters: {}, sortBy: "downloaded_at", searchMode: "semantic" });
    expect(parseSavedParams(saved).searchMode).toBe("semantic");
  });

  it("treats an ordinary search as an ordinary search", () => {
    const saved = JSON.stringify({ tab: "works", filters: {}, sortBy: "downloaded_at" });
    expect(parseSavedParams(saved).searchMode).toBeNull();
    expect(parseSavedParams(saved).versionScope).toBe("current");
    expect(parseSavedParams(saved).sortOrder).toBe("desc");
  });

  it("remembers the selected sort direction", () => {
    const saved = JSON.stringify({ tab: "works", filters: {}, sortBy: "title", sortOrder: "desc" });
    expect(parseSavedParams(saved).sortBy).toBe("title");
    expect(parseSavedParams(saved).sortOrder).toBe("desc");
    expect(parseSavedParams(JSON.stringify({ sortBy: "title" })).sortOrder).toBe("asc");
  });

  it("remembers when saved revisions are part of the search", () => {
    const saved = JSON.stringify({ tab: "works", filters: {}, sortBy: "relevance", versionScope: "all" });
    expect(parseSavedParams(saved).versionScope).toBe("all");
  });

  it("remembers membership shelves and rejects unknown shelf values", () => {
    expect(parseSavedParams(JSON.stringify({ membershipShelf: "reading" })).membershipShelf).toBe("reading");
    expect(parseSavedParams(JSON.stringify({ membershipShelf: "revised" })).membershipShelf).toBe("revised");
    expect(parseSavedParams(JSON.stringify({ membershipShelf: "watched" })).membershipShelf).toBeNull();
    expect(parseSavedParams("{ not json").membershipShelf).toBeNull();
  });

  it("remembers entity-only filters and both halves of the entity ordering", () => {
    const saved = JSON.stringify({
      tab: "series",
      filters: {},
      entityScope: { watch: "paused", minWorkCount: 4, concluded: false },
      entitySortBy: "asset_count",
      entitySortOrder: "asc",
    });
    expect(parseSavedParams(saved)).toEqual(expect.objectContaining({
      entityScope: { watch: "paused", minWorkCount: 4, concluded: false },
      entitySortBy: "asset_count",
      entitySortOrder: "asc",
    }));
  });

  it("remembers both halves of collection ordering", () => {
    const saved = JSON.stringify({
      tab: "collections",
      filters: {},
      collectionSortBy: "available_count",
      collectionSortOrder: "asc",
    });
    expect(parseSavedParams(saved)).toEqual(expect.objectContaining({
      collectionSortBy: "available_count",
      collectionSortOrder: "asc",
    }));
  });

  it("gives old saved searches neutral entity conditions", () => {
    const parsed = parseSavedParams(JSON.stringify({ tab: "people", filters: {} }));
    expect(parsed.entityScope).toEqual({ watch: null, minWorkCount: "", concluded: null });
    expect(parsed.entitySortBy).toBe("work_count");
    expect(parsed.entitySortOrder).toBe("desc");
    expect(parsed.collectionSortBy).toBe("created_at");
    expect(parsed.collectionSortOrder).toBe("desc");
  });

  it("remembers the work-state filters", () => {
    const saved = JSON.stringify({
      tab: "works",
      filters: { revisionFilter: "revised", editFilter: "edited", coverFilter: "no_cover" },
    });
    expect(parseSavedParams(saved).filters).toEqual(expect.objectContaining({
      revisionFilter: "revised",
      editFilter: "edited",
      coverFilter: "no_cover",
    }));
  });

  /** 知らない値を意味検索として扱わない。壊れた保存が検索の種類を変えない。 */
  it("does not promote an unknown mode to semantic", () => {
    const saved = JSON.stringify({ tab: "works", filters: {}, searchMode: "hybrid" });
    expect(parseSavedParams(saved).searchMode).toBeNull();
    expect(parseSavedParams("{ not json").searchMode).toBeNull();
  });
});
