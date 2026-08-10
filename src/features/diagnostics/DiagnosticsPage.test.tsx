import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import DiagnosticsPage from "./DiagnosticsPage";

describe("DiagnosticsPage", () => {
  it("explains measured performance and storage health in preview mode", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><ModalsProvider><DiagnosticsPage /></ModalsProvider></QueryClientProvider></MantineProvider>);
    expect(await screen.findByRole("heading", { name: "ライブラリ診断" })).toBeInTheDocument();
    expect(await screen.findByText("実データ性能")).toBeInTheDocument();
    expect(screen.getByText("索引と保存領域")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "DBを最適化" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "検索索引を最適化" })).toBeDisabled();
  });
});
