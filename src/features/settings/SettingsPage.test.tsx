import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { AppRouter } from "@/app/router";
import { theme } from "@/theme";
import SettingsPage from "./SettingsPage";

describe("SettingsPage navigation", () => {
  it("uses keyboard-operable buttons for settings sections", async () => {
    const user = userEvent.setup();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    window.location.hash = "#/settings";
    render(<MantineProvider theme={theme}><QueryClientProvider client={client}><ModalsProvider><AppRouter><SettingsPage /></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    const about = await screen.findByRole("button", { name: /piepについて/ });
    expect(about).toHaveAttribute("type", "button");
    about.focus();
    await user.keyboard("{Enter}");
    await waitFor(() => expect(window.location.hash).toContain("section=about"));
    expect(about).toHaveAttribute("aria-current", "page");
  });

  /**
   * 区画は URL に載る。一覧に無い値は弾かれて既定へ戻るので、脇の項目を
   * 足しただけで一覧を直し忘れると、**押しても何も起きない項目**ができる。
   * 実際に「コレクションの名前」がそうなった。全部の項目を押して確かめる。
   */
  it("opens every section in the sidebar, not only the ones that shipped first", async () => {
    const user = userEvent.setup();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    window.location.hash = "#/settings";
    render(<MantineProvider theme={theme}><QueryClientProvider client={client}><ModalsProvider><AppRouter><SettingsPage /></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    const nav = await screen.findByRole("button", { name: /サービス接続/ });
    expect(nav).toHaveAttribute("aria-current", "page");
    const sidebar = screen.getByRole("navigation", { name: "設定の項目" });
    const items = [...sidebar.querySelectorAll<HTMLButtonElement>("button")];
    expect(items.length).toBeGreaterThan(4);

    for (const item of items) {
      await user.click(item);
      await waitFor(() => expect(item).toHaveAttribute("aria-current", "page"));
    }
  });
});
