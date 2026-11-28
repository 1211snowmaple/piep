import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
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

beforeEach(() => window.localStorage.clear());

const renderPage = () => render(<MantineProvider><QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><AppRouter><UpdatesPage /></AppRouter></QueryClientProvider></MantineProvider>);

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

it("選んだ内容と処理方法は、画面を開き直しても残る", async () => {
  Element.prototype.scrollIntoView = vi.fn();
  window.location.hash = "#/updates";
  const first = renderPage();
  fireEvent.click(screen.getByRole("radio", { name: "改稿も" }));
  fireEvent.click(screen.getByRole("radio", { name: "自動保存" }));
  first.unmount();

  renderPage();
  expect(screen.getByRole("radio", { name: "改稿も" })).toBeChecked();
  expect(screen.getByRole("radio", { name: "自動保存" })).toBeChecked();
  fireEvent.click(screen.getByRole("button", { name: "確認を開始" }));
  await waitFor(() => expect(mocks.start).toHaveBeenCalledWith(expect.objectContaining({ scope: "all", mode: "auto_save", newWorksOnly: false })));
});

it("覚えていた監視グループが消えていたら、すべての監視対象に戻す", async () => {
  Element.prototype.scrollIntoView = vi.fn();
  window.location.hash = "#/updates";
  window.localStorage.setItem("piep.updates.check-choices", JSON.stringify({ scope: "group:gone", mode: "check_only", depth: "new" }));
  renderPage();
  await waitFor(() => expect(screen.getByRole("combobox", { name: "確認する対象" })).toHaveValue("すべての監視対象"));
});
