import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { AppFrame } from "@/app/AppFrame";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import { isRebuildRunning, rebuildPercent } from "@/features/search/searchIndexProgress";
import type { SearchRebuildProgress } from "@/types/library";

const progressStore = vi.hoisted(() => ({ current: null as SearchRebuildProgress | null }));

vi.mock("@/features/search/searchIndexProgress", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/features/search/searchIndexProgress")>()),
  useSearchIndexProgress: () => progressStore.current,
}));

function running(overrides: Partial<SearchRebuildProgress> = {}): SearchRebuildProgress {
  return {
    jobId: "job-1",
    origin: "automatic",
    status: "running",
    totalDownloads: 1313,
    indexedDownloads: 0,
    pendingDownloads: 1313,
    isComplete: false,
    phase: "indexing",
    processed: 400,
    processedTotal: 1000,
    failed: 0,
    ...overrides,
  };
}

function renderFrame() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MantineProvider>
      <QueryClientProvider client={client}>
        <AppRouter><WorkspaceProvider><AppFrame><div /></AppFrame></WorkspaceProvider></AppRouter>
      </QueryClientProvider>
    </MantineProvider>,
  );
}

describe("search index progress helpers", () => {
  it("reports a percentage only once the size of the run is known", () => {
    expect(rebuildPercent(null)).toBeNull();
    expect(rebuildPercent(running({ processedTotal: 0 }))).toBeNull();
    expect(rebuildPercent(running())).toBe(40);
  });

  it("treats only a running job as in progress", () => {
    expect(isRebuildRunning(null)).toBe(false);
    expect(isRebuildRunning(running({ status: "completed" }))).toBe(false);
    expect(isRebuildRunning(running())).toBe(true);
  });
});

describe("background indexing indicator", () => {
  it("says so in the header while the app catches its index up", () => {
    progressStore.current = running();
    renderFrame();
    // Background work nobody can see is indistinguishable from a slow app.
    expect(screen.getByLabelText("検索インデックスを更新しています（40%）")).toBeInTheDocument();
  });

  it("stays out of the way when there is nothing to report", () => {
    progressStore.current = null;
    renderFrame();
    expect(screen.queryByText(/索引更新中/)).toBeNull();
  });

  it("disappears once the run finishes", () => {
    progressStore.current = running({ status: "completed", processed: 1000 });
    renderFrame();
    expect(screen.queryByText(/索引更新中/)).toBeNull();
  });
});
