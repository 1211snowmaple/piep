import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeAll, beforeEach, describe, expect, it } from "vitest";
import { AppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import LibraryPage, { searchSuggestionAction } from "./LibraryPage";

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
