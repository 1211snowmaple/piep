import { MantineProvider } from "@mantine/core";
import { notifications } from "@mantine/notifications";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { ShelfBackupCard } from "./ShelfBackupCard";

const api = vi.hoisted(() => ({ backup: vi.fn(), inspect: vi.fn(), schedule: vi.fn(), state: vi.fn(), cancel: vi.fn(), importArchive: vi.fn() }));
const dialog = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock("@/services/shelfBackupApi", () => ({
  shelfBackup: api.backup,
  shelfInspectBackup: api.inspect,
  shelfScheduleRestore: api.schedule,
  shelfRestoreState: api.state,
  shelfCancelRestore: api.cancel,
  shelfImportArchive: api.importArchive,
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
  api.inspect.mockResolvedValue({ path: "D:/old-shelf", works: 1200, files: 30000, bytes: 1024, sameLibrary: false });
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

it("takes in an author archive instead of swapping the shelf, and tells what it added", async () => {
  dialog.open.mockResolvedValue("D:/archives/作者");
  api.inspect.mockResolvedValue({ path: "D:/archives/作者", works: 12, files: 40, bytes: 2048, sameLibrary: true, partial: true });
  api.importArchive.mockResolvedValue({ added: 2, restored: 9, already: 1, conflicts: 0 });
  const show = vi.spyOn(notifications, "show");
  renderCard();
  fireEvent.click(await screen.findByRole("button", { name: "バックアップから復元" }));
  expect(await screen.findByText("アーカイブの取り込み")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "次の起動で入れ替える" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "この棚に取り込む" }));
  await waitFor(() => expect(api.importArchive).toHaveBeenCalledWith("D:/archives/作者"));
  expect(api.schedule).not.toHaveBeenCalled();
  await waitFor(() => expect(show).toHaveBeenCalledWith(expect.objectContaining({ title: "アーカイブを取り込みました", message: "2作品を足しました。9作品を棚に戻しました。1作品はすでに棚にあります。" })));
});

it("does not take in an archive made from another shelf", async () => {
  dialog.open.mockResolvedValue("D:/archives/作者");
  api.inspect.mockResolvedValue({ path: "D:/archives/作者", works: 12, files: 40, bytes: 2048, sameLibrary: false, partial: true });
  renderCard();
  fireEvent.click(await screen.findByRole("button", { name: "バックアップから復元" }));
  expect(await screen.findByText(/別の棚から作ったアーカイブなので、取り込めません/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "この棚に取り込む" })).toBeDisabled();
  expect(api.importArchive).not.toHaveBeenCalled();
});
