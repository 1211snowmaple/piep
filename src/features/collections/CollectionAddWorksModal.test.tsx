import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { demoWorks, getDemoCollection } from "@/mocks/demoData";
import type { DownloadEntry, SearchV2Params } from "@/types/library";
import { CollectionAddWorksModal } from "./CollectionAddWorksModal";

const dbApi = vi.hoisted(() => ({ searchDownloadsV2: vi.fn() }));

vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  searchDownloadsV2: dbApi.searchDownloadsV2,
}));

vi.mock("@/components/WorkCard", () => ({
  WorkCard: ({ work, selected, onSelect }: { work: DownloadEntry; selected?: boolean; onSelect?: (id: number, selected: boolean) => void }) => (
    <button type="button" aria-label={work.title} aria-pressed={selected} onClick={() => onSelect?.(work.id, !selected)}>{work.title}</button>
  ),
}));

function result(items: DownloadEntry[]) {
  return {
    items,
    nextCursor: null,
    totalEstimate: items.length,
    searchMeta: { engine: "test", query: null, totalEstimate: items.length, indexComplete: true, semanticIndexComplete: true, semanticModelReady: true },
    facetsVersion: 1,
  };
}

describe("CollectionAddWorksModal", () => {
  beforeEach(() => {
    dbApi.searchDownloadsV2.mockReset();
    dbApi.searchDownloadsV2.mockImplementation((params: SearchV2Params) => Promise.resolve(result([params.favorite ? demoWorks[1] : demoWorks[0]])));
  });

  it("keeps selected works when a scope change replaces the visible results", async () => {
    const onAdd = vi.fn();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <MantineProvider>
        <QueryClientProvider client={client}>
          <CollectionAddWorksModal opened onClose={() => undefined} collection={getDemoCollection("demo-empty")} busy={false} onAdd={onAdd} />
        </QueryClientProvider>
      </MantineProvider>,
    );

    fireEvent.click(await screen.findByRole("button", { name: demoWorks[0].title }));
    fireEvent.click(screen.getByText("お気に入り"));
    fireEvent.click(await screen.findByRole("button", { name: demoWorks[1].title }));

    const add = await screen.findByRole("button", { name: "2作品を追加" });
    fireEvent.click(add);
    await waitFor(() => expect(onAdd).toHaveBeenCalledWith([demoWorks[0], demoWorks[1]]));
  });
});
