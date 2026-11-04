import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";

const mocks = vi.hoisted(() => ({ start: vi.fn().mockResolvedValue(undefined), loadJobs: vi.fn() }));
vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  listUpdateTargets: async () => [],
  listWatchGroups: async () => [],
}));
vi.mock("@/services/updateJobApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/updateJobApi")>()),
  countDismissedUpdateCandidatesCommand: async () => 0,
}));
vi.mock("./updateJobs", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./updateJobs")>()),
  useUpdateJobs: () => ({ jobs: [], activeSnapshot: null, start: mocks.start, loadJobs: mocks.loadJobs }),
}));
vi.mock("./updateSchedule", async (importOriginal) => {
  const original = await importOriginal<typeof import("./updateSchedule")>();
  return { ...original, loadSchedule: async () => original.updateScheduleDefaults };
});
vi.mock("./useUpdateScheduler", () => ({ useUpdateJobNotifications: () => {} }));
vi.mock("./DeferredCandidatesPanel", () => ({ DeferredCandidatesPanel: () => null }));
import UpdatesPage from "./UpdatesPage";

it("監視中の作品では、表示どおり改稿を確認する依頼を送る", async () => {
  Element.prototype.scrollIntoView = vi.fn();
  window.location.hash = "#/updates";
  render(<MantineProvider><QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><AppRouter><UpdatesPage /></AppRouter></QueryClientProvider></MantineProvider>);
  fireEvent.click(screen.getByRole("combobox", { name: "確認する対象" }));
  fireEvent.click(await screen.findByRole("option", { name: "監視中の作品" }));
  expect(screen.getByRole("radio", { name: "改稿も" })).toBeChecked();
  fireEvent.click(screen.getByRole("button", { name: "確認を開始" }));
  await waitFor(() => expect(mocks.start).toHaveBeenCalledWith(expect.objectContaining({ scope: "work", newWorksOnly: false })));
});
