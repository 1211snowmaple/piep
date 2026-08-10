import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import OperationsPage from "./OperationsPage";
import { startOperation } from "./operationJobs";

describe("OperationsPage", () => {
  it("renders a shared operation with progress and logs", () => {
    const operation = startOperation({ kind: "backup", label: "ライブラリをバックアップ", total: 2 });
    operation.progress(1, 2, "データを検証中");
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<MantineProvider><QueryClientProvider client={client}><OperationsPage /></QueryClientProvider></MantineProvider>);
    expect(screen.getByRole("heading", { name: "操作履歴" })).toBeInTheDocument();
    expect(screen.getByText("ライブラリをバックアップ")).toBeInTheDocument();
    expect(screen.getByText("1 / 2")).toBeInTheDocument();
  });
});
