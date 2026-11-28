import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import type { AdminStatus } from "../api";
import { OverviewPage } from "./OverviewPage";

const expiresAt = new Date(2027, 0, 31, 12).toISOString();
const dayMs = 24 * 60 * 60 * 1000;
const filesWarning = "証明書の期限が近づいています。ファイルを入れ替えたらサーバーを立ち上げ直してください";
const selfSignedWarning = "証明書の期限が切れています。証明書と秘密鍵を両方削除して立ち上げ直し、端末を登録し直してください";

afterEach(() => vi.restoreAllMocks());

function renderOverview(certificate: AdminStatus["certificate"], remainingMs: number) {
  vi.spyOn(Date, "now").mockReturnValue(Date.parse(expiresAt) - remainingMs);
  const client = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity } } });
  client.setQueryData<AdminStatus>(["status"], {
    version: "0.43.0",
    apiVersion: 2,
    shelf: null,
    devices: [],
    publicUrl: null,
    certificate,
    update: null,
    serverName: "試験用サーバー",
    startedAt: "2026-12-01T00:00:00Z",
    adminWeb: "available",
  });
  client.setQueryData(["storage"], { freeBytes: null });
  client.setQueryData(["audit", "recent"], []);
  render(
    <MantineProvider>
      <QueryClientProvider client={client}>
        <OverviewPage go={vi.fn()} />
      </QueryClientProvider>
    </MantineProvider>,
  );
}

it.each([
  { source: "files", remainingMs: 30 * dayMs - 1, warning: filesWarning },
  { source: "self-signed", remainingMs: -dayMs, warning: selfSignedWarning },
] as const)("期限まで30日未満の $source の証明書は有効期限と更新方法を表示する", ({ source, remainingMs, warning }) => {
  renderOverview({ expiresAt, source }, remainingMs);

  expect(screen.getByText("証明書の有効期限")).toBeInTheDocument();
  expect(screen.getByText(/2027-01-31/)).toHaveAttribute("datetime", expiresAt);
  expect(screen.getByText(warning)).toBeInTheDocument();
  expect(screen.queryByText("問題はありません")).toBeNull();
});

it.each([30, 31])("期限まで %i 日あれば警告を表示しない", (days) => {
  renderOverview({ expiresAt, source: "files" }, days * dayMs);

  expect(screen.getByText("証明書の有効期限")).toBeInTheDocument();
  expect(screen.getByText(/2027-01-31/)).toHaveAttribute("datetime", expiresAt);
  expect(screen.getByText("問題はありません")).toBeInTheDocument();
});

it("TLS が無効なら証明書の欄を表示しない", () => {
  renderOverview(null, 0);

  expect(screen.queryByText("証明書の有効期限")).toBeNull();
  expect(screen.getByText("問題はありません")).toBeInTheDocument();
});
