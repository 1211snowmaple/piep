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
import LibraryPage, { resolveSortBy, rollbackWorkFlag, searchSuggestionAction, updateWorkFlag } from "./LibraryPage";

describe("LibraryPage search", () => {
  beforeAll(() => {
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
    expect(sort).toHaveValue("関連度が高い順");
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
    expect(sort).toHaveValue("タイトル昇順（あ→ん）");

    const input = screen.getByLabelText("ライブラリを検索");
    fireEvent.change(input, { target: { value: "" } });
    await waitFor(() => expect(window.location.hash).not.toContain("q="), { timeout: 1500 });
    // Relevance has no meaning without a query, but an explicit column sort does.
    expect(screen.getByRole("combobox", { name: "並び順" })).toHaveValue("タイトル昇順（あ→ん）");
  });

  it("falls back from relevance to the saved-date order when the query is cleared", async () => {
    window.location.hash = "#/library?q=%E6%97%A5%E6%9C%AC";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    expect(await screen.findByRole("combobox", { name: "並び順" })).toHaveValue("関連度が高い順");
    fireEvent.change(screen.getByLabelText("ライブラリを検索"), { target: { value: "" } });
    await waitFor(() => expect(window.location.hash).not.toContain("q="), { timeout: 1500 });
    expect(screen.getByRole("combobox", { name: "並び順" })).toHaveValue("保存が新しい順");
  });

  it("normalizes unsupported URL and persisted values instead of entering an invalid view", async () => {
    window.localStorage.setItem("piep.library-view", JSON.stringify("future-view"));
    window.localStorage.setItem("piep.saved-searches.v2", JSON.stringify({ malformed: true }));
    window.location.hash = "#/library?tab=unknown&watch=yes&sort=not-a-sort";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    expect(await screen.findByRole("tab", { name: "作品" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("combobox", { name: "並び順" })).toHaveValue("保存が新しい順");
    expect(screen.getByRole("radiogroup", { name: "表示形式" })).toBeInTheDocument();
    expect(screen.getByLabelText("ギャラリー表示")).toBeInTheDocument();
  });

  it("stages expensive filters until Apply and validates the URL-owned flags once", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    fireEvent.click(await screen.findByRole("button", { name: "絞り込み" }));
    fireEvent.click(await screen.findByRole("checkbox", { name: "お気に入りのみ" }));
    expect(window.location.hash).not.toContain("favorite=1");
    fireEvent.click(screen.getByRole("button", { name: "適用" }));
    await waitFor(() => expect(window.location.hash).toContain("favorite=1"));
  });

  it("names the page and every button in the filter drawer", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider theme={theme}><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    expect(await screen.findByRole("heading", { level: 1, name: "ライブラリ" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "絞り込み" }));
    const drawer = await screen.findByRole("dialog", { name: "詳細フィルター" });
    expect(within(drawer).getByRole("button", { name: "閉じる" })).toBeInTheDocument();
    within(drawer).getAllByRole("button").forEach((button) => expect(button).toHaveAccessibleName());
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
    fireEvent.click(await screen.findByRole("checkbox", { name: "pixiv" }));
    fireEvent.change(screen.getByRole("textbox", { name: "最小文字数" }), { target: { value: "5000" } });
    fireEvent.click(screen.getByRole("button", { name: "適用" }));

    await waitFor(() => expect(window.location.hash).toContain("source=pixiv"));
    expect(window.location.hash).toContain("minchars=5000");
  });

  it("restores the drawer's filters when history returns to them", async () => {
    window.location.hash = "#/library?source=pixiv&tag=%E5%89%B5%E4%BD%9C&minchars=5000";
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><AppRouter><WorkspaceProvider><LibraryPage /></WorkspaceProvider></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    expect(await screen.findByText("適用中")).toBeInTheDocument();
    expect(screen.getByText("#創作")).toBeInTheDocument();
    // And the drawer opens onto the conditions actually in force, rather than
    // an empty form that disagrees with the results behind it.
    fireEvent.click(screen.getByRole("button", { name: "絞り込み" }));
    expect(await screen.findByRole("checkbox", { name: "pixiv" })).toBeChecked();
    expect(screen.getByRole("textbox", { name: "最小文字数" })).toHaveValue("5,000");
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
