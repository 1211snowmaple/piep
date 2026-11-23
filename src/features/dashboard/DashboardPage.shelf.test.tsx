import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import type { ShelfCard } from "@/services/shelfApi";
import type { DashboardSummary } from "@/types/library";
import DashboardPage from "./DashboardPage";

const shelfApi = vi.hoisted(() => ({ shelfStatus: vi.fn(), shelfSearch: vi.fn() }));
vi.mock("@/services/shelfApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/shelfApi")>()),
  ...shelfApi,
}));
const health = vi.hoisted(() => ({ shelfDashboard: vi.fn(), shelfSearchIndexStatus: vi.fn() }));
vi.mock("@/services/shelfHealthApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/shelfHealthApi")>()),
  ...health,
}));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
}));
vi.mock("@/services/authApi", () => ({ getConnectionStatus: vi.fn().mockResolvedValue({ pixiv: { name: "利用者", webSession: true }, fanbox: { name: "利用者" } }) }));

const card: ShelfCard = {
  id: "0195f1c2-0000-7000-8000-000000000001",
  snapshot_id: "0195f1c2-0000-7000-8000-000000000011",
  edit_id: null,
  provider: "pixiv",
  external_key: "1",
  content_type: "novel",
  title: "棚の作品",
  author_name: "作者",
  saved_at: Date.UTC(2026, 10, 1),
  published_at: null,
  updated_at: null,
  text_length: 10,
  logical_size: 100,
  media_count: 0,
  version_number: 1,
  favorite: false,
  watching: false,
  revised: false,
  image_count: 0,
  file_count: 0,
  excerpt: null,
  tags: [],
  cover: null,
  series_title: null,
  series_position: null,
  coverFile: null,
} as unknown as ShelfCard;

const summary: DashboardSummary = {
  stats: { totalWorks: 27, pixivCount: 20, fanboxCount: 7, totalAssets: 3, totalSizeBytes: 1024 },
  favoriteCount: 4,
  watchedCount: 0,
  updateTargetCount: 5,
  indexedCount: 27,
  pendingIndexCount: 0,
  topTags: [{ name: "雨", count: 9, sources: [] }],
  topAuthors: [],
  recentWorks: [],
  sourceBreakdown: [{ source: "pixiv", count: 20, totalSizeBytes: 1000 }],
  monthlySaves: [],
};

describe("棚のホーム", () => {
  beforeEach(() => {
    shelfApi.shelfStatus.mockResolvedValue({ state: "ready", path: "C:/shelf" });
    shelfApi.shelfSearch.mockResolvedValue({ page: { items: [], next: null, total: 1 }, intent: null, matches: [], cards: [card] });
    health.shelfDashboard.mockResolvedValue(summary);
    health.shelfSearchIndexStatus.mockResolvedValue({ totalWorks: 27, indexedWorks: 27, pendingWorks: 0, isComplete: true, phase: "ready", semanticIndexedChunks: 0, semanticIndexedWorks: 0, semanticPendingWorks: 0, semanticEnabled: false, semanticModelReady: false, embeddingProvider: "cpu", gpuEnabled: false });
  });
  afterEach(() => {
    localStorage.clear();
    vi.clearAllMocks();
  });

  it("数も最近の保存も棚から出す", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <MantineProvider>
        <QueryClientProvider client={client}>
          <AppRouter><WorkspaceProvider><DashboardPage /></WorkspaceProvider></AppRouter>
        </QueryClientProvider>
      </MantineProvider>,
    );
    expect((await screen.findAllByText("棚の作品")).length).toBeGreaterThan(0);
    expect(screen.getByText("27")).toBeTruthy();
    expect(screen.getByText("100%")).toBeTruthy();
    const [, query] = shelfApi.shelfSearch.mock.calls[0];
    expect(query).toMatchObject({ sort: "saved", descending: true, limit: 4 });
  });
});
