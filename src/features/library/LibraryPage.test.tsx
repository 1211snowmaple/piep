import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { AppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import LibraryPage from "./LibraryPage";

describe("LibraryPage search", () => {
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
  });
});
