import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { theme } from "@/theme";
import type { DesktopServerStatus, RemoteServerStatus } from "@/services/serverApi";

const api = vi.hoisted(() => ({
  serverStatus: vi.fn(),
  serverRemoteStatus: vi.fn(),
  serverAdminLink: vi.fn(),
  serverConnectRemote: vi.fn(),
  serverDisconnectRemote: vi.fn(),
  serverSetAutoSend: vi.fn(),
  serverSendNow: vi.fn(),
  serverSendStop: vi.fn(),
}));
vi.mock("@/services/serverApi", () => api);
const opener = vi.hoisted(() => ({ openExternalUrl: vi.fn() }));
vi.mock("@/services/openerApi", () => opener);

import { remoteView, ServerSection } from "@/features/settings/ServerSection";

function status(overrides: Partial<DesktopServerStatus> = {}): DesktopServerStatus {
  return {
    remote: null,
    autoSend: false,
    send: { running: false, phase: "", filesTotal: 0, filesDone: 0, bytesTotal: 0, bytesDone: 0, message: null },
    lastSend: { finishedAt: null, works: null, filesAdded: null, error: null },
    ...overrides,
  };
}


function remote(overrides: Partial<RemoteServerStatus> = {}): RemoteServerStatus {
  return {
    version: "0.36.0",
    apiVersion: 2,
    shelf: { libraryId: "0190", syncEpoch: "0191", works: "12" },
    update: { auto: true, window: "03:00-06:00", phase: { state: "idle" }, latest: "0.36.0", lastCheckedAt: "2026-11-01T00:00:00Z", skipped: [] },
    serverName: "Maple",
    adminWeb: "available",
    ...overrides,
  };
}

const connected = () => status({ remote: { url: "https://server.example/", fingerprint: null } });

function renderSection(runtime = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <MantineProvider theme={theme}>
      <ModalsProvider>
        <QueryClientProvider client={client}>
          <ServerSection runtime={runtime} />
        </QueryClientProvider>
      </ModalsProvider>
    </MantineProvider>,
  );
}

describe("ServerSection", () => {
  beforeEach(() => {
    for (const mock of Object.values(api)) mock.mockReset();
    api.serverRemoteStatus.mockResolvedValue({ version: "0.31.0", apiVersion: 2, shelf: { libraryId: "0190", syncEpoch: "0191", works: "12" }, update: null });
  });

  it("does not call the server outside the desktop app", () => {
    renderSection(false);
    expect(screen.getByText("サーバーへの接続と送信はデスクトップアプリで利用できます。")).toBeTruthy();
    expect(api.serverStatus).not.toHaveBeenCalled();
  });

  it("connects to a standalone server with the pasted string and clears it", async () => {
    api.serverStatus.mockResolvedValue(status());
    api.serverConnectRemote.mockResolvedValue(status());
    renderSection();
    const input = await screen.findByLabelText("管理者用の登録文字列");
    fireEvent.change(input, { target: { value: "  {\"version\":2}  " } });
    fireEvent.click(screen.getByRole("button", { name: "接続" }));
    await waitFor(() => expect(api.serverConnectRemote).toHaveBeenCalledWith("  {\"version\":2}  "));
    await waitFor(() => expect((input as HTMLTextAreaElement).value).toBe(""));
  });

  it("shows sending progress and lets it be stopped", async () => {
    api.serverStatus.mockResolvedValue(status({
      remote: { url: "https://server.example/", fingerprint: null },
      send: { running: true, phase: "upload", filesTotal: 10, filesDone: 4, bytesTotal: 100, bytesDone: 50, message: null },
    }));
    api.serverRemoteStatus.mockResolvedValue(remote());
    api.serverSendStop.mockResolvedValue(status());
    renderSection();
    expect(await screen.findByText("送っています 50%")).toBeTruthy();
    expect(await screen.findByText("12 作品")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "中止" }));
    await waitFor(() => expect(api.serverSendStop).toHaveBeenCalled());
  });

  it("keeps disconnecting and revoking out of the everyday screen", async () => {
    api.serverStatus.mockResolvedValue(connected());
    api.serverRemoteStatus.mockResolvedValue(remote());
    renderSection();
    expect(await screen.findByText("Maple に接続しています")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /解除/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "携帯を追加" })).toBeNull();
    expect(screen.queryByRole("switch", { name: /閲覧/ })).toBeNull();
    expect(screen.getByRole("button", { name: "URL をコピー" })).toBeTruthy();
  });

  it("opens the admin screen with a one-time link", async () => {
    api.serverStatus.mockResolvedValue(connected());
    api.serverRemoteStatus.mockResolvedValue(remote());
    api.serverAdminLink.mockResolvedValue("https://server.example/admin/#ticket=abc");
    renderSection();
    const open = await screen.findByRole("button", { name: "管理画面を開く" });
    await waitFor(() => expect((open as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(open);
    await waitFor(() => expect(opener.openExternalUrl).toHaveBeenCalledWith("https://server.example/admin/#ticket=abc"));
  });

  it("guides the operator to the management page when the connected address only serves readers", async () => {
    api.serverStatus.mockResolvedValue(connected());
    api.serverRemoteStatus.mockResolvedValue(remote({ adminWeb: "public-only", shelfPending: true }));
    renderSection();
    expect(await screen.findByText("現在の接続先では、サーバーの PC で管理画面を開けます。")).toBeTruthy();
    expect(screen.getByText("送ったライブラリは、サーバーを再起動すると読めるようになります。")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "開き方を見る" }));
    expect(opener.openExternalUrl).toHaveBeenCalledWith(expect.stringContaining("#管理画面"));
    expect(api.serverAdminLink).not.toHaveBeenCalled();
  });

  it("reports available server updates and routes manual updates to administration", async () => {
    api.serverStatus.mockResolvedValue(connected());
    api.serverRemoteStatus.mockResolvedValue(remote({
      version: "0.35.0",
      update: { auto: false, phase: { state: "idle" }, latest: "0.36.0", lastCheckedAt: null },
    }));
    renderSection();
    expect(await screen.findByText("新しいバージョンがあります")).toBeTruthy();
    expect(screen.getByText("管理画面の「更新」から更新できます。")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /更新/ })).toBeNull();
  });

  it("offers to retry and blocks server actions when the server is out of reach", async () => {
    api.serverStatus.mockResolvedValue(connected());
    api.serverRemoteStatus.mockRejectedValue("サーバーに接続できません");
    renderSection();
    expect(await screen.findByText("server.example に接続できません")).toBeTruthy();
    expect((screen.getByRole("button", { name: /今すぐ送る/ }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("switch", { name: /自動で送る/ }) as HTMLInputElement).disabled).toBe(false);
    expect(screen.getByRole("button", { name: "もう一度確認" })).toBeTruthy();
  });

  it("lets a lost registration be redone in place", async () => {
    api.serverStatus.mockResolvedValue(connected());
    api.serverRemoteStatus.mockRejectedValue("登録が残っていません。登録し直してください");
    renderSection();
    expect(await screen.findByText("server.example の登録が残っていません")).toBeTruthy();
    expect(screen.getByLabelText("新しい登録文字列")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "もう一度確認" })).toBeNull();
  });

  it("reports a failed action", async () => {
    api.serverStatus.mockResolvedValue(connected());
    api.serverRemoteStatus.mockResolvedValue(remote());
    api.serverSendNow.mockRejectedValue("送れませんでした");
    renderSection();
    await screen.findByText("Maple に接続しています");
    fireEvent.click(screen.getByRole("button", { name: /今すぐ送る/ }));
    expect(await screen.findByText("送れませんでした")).toBeTruthy();
  });
});

describe("remoteView", () => {
  const base = { name: "Maple", offline: false, registrationLost: false, sending: false, checkedAt: Date.now() };

  it("only says what it has confirmed", () => {
    expect(remoteView({ ...base, details: undefined }).headline).toBe("Maple を確認しています");
    expect(remoteView({ ...base, details: remote() })).toEqual({ tone: "ok", headline: "Maple に接続しています", summary: null });
  });

  it("does not call an update in progress a failure", () => {
    const applying = remote({ update: { auto: true, window: "03:00-06:00", phase: { state: "applying", version: "0.36.1" }, latest: "0.36.1", lastCheckedAt: null } });
    expect(remoteView({ ...base, details: applying, offline: true }).headline).toBe("0.36.1 に更新しています");
  });

  it("does not offer a version that failed to start", () => {
    const skipped = remote({ version: "0.36.0", update: { auto: true, window: "03:00-06:00", phase: { state: "idle" }, latest: "0.36.1", lastCheckedAt: null, skipped: ["0.36.1"] } });
    expect(remoteView({ ...base, details: skipped }).headline).toBe("Maple に接続しています");
  });
});
