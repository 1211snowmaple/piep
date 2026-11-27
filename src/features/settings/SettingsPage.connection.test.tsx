import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { theme } from "@/theme";

const authApi = vi.hoisted(() => ({
  getConnectionStatus: vi.fn(),
  disconnectAccount: vi.fn(),
  clearUnreadableAccounts: vi.fn(),
  activateAccount: vi.fn(),
  loginPixivWebview: vi.fn(),
  connectPixivToken: vi.fn(),
  loginFanboxWebview: vi.fn(),
  connectFanboxSession: vi.fn(),
}));

vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getSearchIndexStatus: vi.fn().mockResolvedValue({ totalWorks: 12, indexedWorks: 12, pendingWorks: 0, isComplete: true, phase: "ready", semanticIndexedChunks: 0, semanticIndexedWorks: 0, semanticPendingWorks: 12, semanticModelReady: false, embeddingProvider: "CPU", gpuEnabled: false, throughputPerSec: null }),
}));
vi.mock("@/services/shelfApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/shelfApi")>()),
  shelfStatus: vi.fn().mockResolvedValue({ state: "ready", path: "C:/piep/library" }),
}));
vi.mock("@/services/shelfHealthApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/shelfHealthApi")>()),
  shelfDashboard: vi.fn().mockResolvedValue({ stats: { totalWorks: 12, pixivCount: 12, fanboxCount: 0, totalAssets: 108, totalSizeBytes: 32_000_000 } }),
}));
vi.mock("@/services/authApi", () => authApi);
vi.mock("@/features/search/searchIndexProgress", () => ({ useSearchIndexProgress: () => null }));

import SettingsPage from "./SettingsPage";

function renderConnectionSettings() {
  window.location.hash = "#/settings";
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return { client, ...render(
    <MantineProvider theme={theme}>
      <QueryClientProvider client={client}>
        <ModalsProvider><AppRouter><SettingsPage /></AppRouter></ModalsProvider>
      </QueryClientProvider>
    </MantineProvider>,
  ) };
}

describe("SettingsPage connection flow", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    authApi.getConnectionStatus.mockResolvedValue({ pixiv: null, fanbox: { name: "FANBOX利用者" }, accounts: { pixiv: [], fanbox: [{ key: "f1", name: "FANBOX利用者", active: true }] } });
    authApi.disconnectAccount.mockResolvedValue({ pixiv: null, fanbox: null, accounts: { pixiv: [], fanbox: [] } });
  });

  it("asks the backend to forget the FANBOX connection", async () => {
    renderConnectionSettings();

    fireEvent.click(await screen.findByRole("button", { name: "FANBOX利用者の接続を解除" }));
    expect(await screen.findByRole("dialog", { name: "FANBOX利用者の接続を解除しますか？" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "接続を解除" }));

    await waitFor(() => expect(authApi.disconnectAccount).toHaveBeenCalledWith("fanbox", "f1"));
    await waitFor(() => expect(screen.queryByRole("button", { name: "FANBOX利用者の接続を解除" })).not.toBeInTheDocument());
  });

  it("switches the account used for saving without asking", async () => {
    const before = {
      pixiv: { name: "表", webSession: true },
      fanbox: null,
      accounts: { pixiv: [{ key: "p1", name: "表", active: true }, { key: "p2", name: "裏", active: false }], fanbox: [] },
    };
    authApi.getConnectionStatus.mockResolvedValue(before);
    authApi.activateAccount.mockResolvedValue({
      pixiv: { name: "裏", webSession: true },
      fanbox: null,
      accounts: { pixiv: [{ key: "p1", name: "表", active: false }, { key: "p2", name: "裏", active: true }], fanbox: [] },
    });
    const { client } = renderConnectionSettings();

    expect(await screen.findByText("使用中")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "アカウントを追加" })).toBeInTheDocument();
    let finishStatus!: (value: typeof before) => void;
    authApi.getConnectionStatus.mockImplementationOnce(() => new Promise((resolve) => { finishStatus = resolve; }));
    const staleRead = client.refetchQueries({ queryKey: ["settings-auth"] });
    await waitFor(() => expect(finishStatus).toBeTypeOf("function"));
    fireEvent.click(screen.getByRole("button", { name: "裏を使う" }));

    await waitFor(() => expect(authApi.activateAccount).toHaveBeenCalledWith("pixiv", "p2"));
    expect(await screen.findByRole("button", { name: "表を使う" })).toBeInTheDocument();
    await act(async () => {
      finishStatus(before);
      await staleRead;
    });
    expect(screen.getByRole("button", { name: "表を使う" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("読み取れない取得元の一覧を確認後に削除して登録し直せる", async () => {
    authApi.getConnectionStatus.mockResolvedValue({ pixiv: { name: "pixiv利用者", webSession: true }, fanbox: null, unreadable: ["fanbox"] });
    authApi.clearUnreadableAccounts.mockResolvedValue({ pixiv: { name: "pixiv利用者", webSession: true }, fanbox: null });
    renderConnectionSettings();

    expect(await screen.findByText("pixiv利用者")).toBeInTheDocument();
    expect(screen.getByText("保存された接続を読み取れませんでした。一覧を削除してから登録し直してください")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "接続一覧を削除" }));
    const dialog = await screen.findByRole("dialog", { name: "保存された接続一覧を削除しますか？" });
    expect(authApi.clearUnreadableAccounts).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "一覧を削除" }));
    await waitFor(() => expect(authApi.clearUnreadableAccounts).toHaveBeenCalledWith("fanbox"));
    expect(await screen.findByRole("button", { name: "ブラウザで接続" })).toBeEnabled();
    expect(screen.getByText("pixiv利用者")).toBeInTheDocument();
  });

  it("接続済みの取得元にも手動でアカウントを追加できる", async () => {
    authApi.connectFanboxSession.mockResolvedValue({
      pixiv: null, fanbox: { name: "二人目" }, accounts: {
        pixiv: [], fanbox: [{ key: "f1", name: "FANBOX利用者", active: false }, { key: "f2", name: "二人目", active: true }],
      },
    });
    renderConnectionSettings();
    expect(await screen.findByText("使用中")).toBeInTheDocument();
    const session = screen.getByLabelText("FANBOXSESSID");
    fireEvent.change(session, { target: { value: "試験用の入力" } });
    fireEvent.click(within(session.closest("form")!).getByRole("button", { name: "検証して保存" }));
    await waitFor(() => expect(authApi.connectFanboxSession).toHaveBeenCalledWith("試験用の入力", "Mozilla/5.0"));
    expect(await screen.findByRole("button", { name: "二人目の接続を解除" })).toBeInTheDocument();
  });

  it("切り替えが終わるまで追加・解除・復旧の操作を押せない", async () => {
    const status = {
      pixiv: { name: "表", webSession: true }, fanbox: null, unreadable: ["fanbox"],
      accounts: { pixiv: [{ key: "p1", name: "表", active: true }, { key: "p2", name: "裏", active: false }], fanbox: [] },
    };
    authApi.getConnectionStatus.mockResolvedValue(status);
    let finish!: (value: typeof status) => void;
    authApi.activateAccount.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    renderConnectionSettings();
    const switchButton = await screen.findByRole("button", { name: "裏を使う" });
    fireEvent.click(switchButton);
    await waitFor(() => expect(switchButton).toBeDisabled());
    expect(screen.getByRole("button", { name: "アカウントを追加" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "表の接続を解除" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "接続一覧を削除" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "検証して保存" })).toBeDisabled();
    finish(status);
    await waitFor(() => expect(switchButton).toBeEnabled());
  });

  it("offers a browser reconnect when the pixiv connection lacks a web session", async () => {
    authApi.getConnectionStatus.mockResolvedValue({ pixiv: { name: "pixiv利用者", webSession: false }, fanbox: null });
    renderConnectionSettings();

    expect(await screen.findByText("pixiv利用者")).toBeInTheDocument();
    expect(screen.getByText("一覧を使った高速確認には、ブラウザでの再接続が必要です")).toBeInTheDocument();
  });
});
