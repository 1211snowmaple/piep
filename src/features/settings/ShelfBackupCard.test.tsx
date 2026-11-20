import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { ShelfBackupCard } from "./ShelfBackupCard";

const api = vi.hoisted(() => ({ backup: vi.fn(), inspect: vi.fn(), schedule: vi.fn(), state: vi.fn(), cancel: vi.fn() }));
const dialog = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock("@/services/shelfBackupApi", () => ({
  libraryV2Backup: api.backup,
  libraryV2InspectBackup: api.inspect,
  libraryV2ScheduleRestore: api.schedule,
  libraryV2RestoreState: api.state,
  libraryV2CancelRestore: api.cancel,
}));
vi.mock("@/services/dialogApi", () => ({ openSingleDialog: dialog.open }));

beforeEach(() => {
  vi.clearAllMocks();
  api.state.mockResolvedValue({ pending: null, last: null });
});

const renderCard = () => render(<MantineProvider><QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><AppRouter><ShelfBackupCard runtime busy={false} /></AppRouter></QueryClientProvider></MantineProvider>);

it("makes a backup as a new folder inside the chosen one", async () => {
  dialog.open.mockResolvedValue("C:\\backups");
  api.backup.mockImplementation(async (path: string) => path);
  renderCard();
  fireEvent.click(await screen.findByRole("button", { name: "棚のバックアップを作る" }));
  await waitFor(() => expect(api.backup).toHaveBeenCalledOnce());
  expect(api.backup.mock.calls[0][0]).toMatch(/^C:\\backups\\piep-library-backup-\d{4}-\d{2}-\d{2}-/);
});

it("inspects a backup, says what it holds, and schedules the swap only on confirm", async () => {
  dialog.open.mockResolvedValue("D:/old-shelf");
  api.inspect.mockResolvedValue({ path: "D:/old-shelf", works: 1200, files: 30000, bytes: 1024, sameLibrary: false, switched: true });
  api.schedule.mockResolvedValue({});
  renderCard();
  fireEvent.click(await screen.findByRole("button", { name: "バックアップから復元" }));
  expect(await screen.findByText("作品 1,200件")).toBeInTheDocument();
  expect(screen.getByText(/今の棚とは別の棚の写しです/)).toBeInTheDocument();
  expect(api.schedule).not.toHaveBeenCalled();
  api.state.mockResolvedValue({ pending: { backup: "D:/old-shelf", requestedAt: 1 }, last: null });
  fireEvent.click(screen.getByRole("button", { name: "次の起動で入れ替える" }));
  await waitFor(() => expect(api.schedule).toHaveBeenCalledWith("D:/old-shelf"));
  expect(await screen.findByText("復元を予約しています")).toBeInTheDocument();
  // 予約中は、もう一つ予約しない。
  expect(screen.getByRole("button", { name: "バックアップから復元" })).toBeDisabled();
  fireEvent.click(screen.getByRole("button", { name: "予約を取り消す" }));
  await waitFor(() => expect(api.cancel).toHaveBeenCalledOnce());
});

it("tells where the previous shelf was kept after a restore", async () => {
  api.state.mockResolvedValue({ pending: null, last: { backup: "D:/old-shelf", previous: "C:/data/shelf.before-restore-1", error: null, finishedAt: 1_700_000_000 } });
  renderCard();
  expect(await screen.findByText("C:/data/shelf.before-restore-1")).toBeInTheDocument();
});
