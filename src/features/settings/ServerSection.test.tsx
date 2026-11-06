import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { theme } from "@/theme";
import type { DesktopServerStatus, RemoteServerStatus } from "@/services/serverApi";

const api = vi.hoisted(() => ({
  serverStatus: vi.fn(),
  serverInterfaces: vi.fn(),
  serverEmbeddedStart: vi.fn(),
  serverEmbeddedStop: vi.fn(),
  serverCreateReaderPairing: vi.fn(),
  serverRevokeDevice: vi.fn(),
  serverRemoteDevices: vi.fn(),
  serverRemoteStatus: vi.fn(),
  serverUpdateRemote: vi.fn(),
  serverCheckRemoteUpdate: vi.fn(),
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

import { nextAction, remoteView, ServerSection } from "@/features/settings/ServerSection";

function status(overrides: Partial<DesktopServerStatus> = {}): DesktopServerStatus {
  return {
    embeddedRunning: false,
    embeddedUrl: null,
    embeddedFingerprint: null,
    embedded: { enabled: false, ip: null, behindProxy: false, publicUrl: null },
    devices: [],
    remote: null,
    autoSend: false,
    send: { running: false, phase: "", filesTotal: 0, filesDone: 0, bytesTotal: 0, bytesDone: 0, message: null, outsidePaths: 0, skippedNames: 0 },
    lastSend: { finishedAt: null, generation: null, works: null, filesAdded: null, filesRemoved: null, missingReferenced: null, unchanged: false, error: null },
    ...overrides,
  };
}

const phone = { id: "b", name: "Pixel", role: "reader" as const, createdAt: "2026-11-01T00:00:00Z", lastSeenAt: null };
const admin = { id: "a", name: "PC", role: "admin" as const, createdAt: "2026-11-01T00:00:00Z", lastSeenAt: null };

function remote(overrides: Partial<RemoteServerStatus> = {}): RemoteServerStatus {
  return {
    version: "0.36.0",
    apiVersion: 2,
    works: 12,
    publicUrl: null,
    sync: { generation: 1, committedAt: null, indexPending: 0, indexError: null },
    update: { auto: true, window: "03:00-06:00", phase: { state: "idle" }, latest: "0.36.0", lastCheckedAt: "2026-11-01T00:00:00Z", lastError: null, lastAppliedAt: null, skipped: [] },
    serverName: "Maple",
    adminWeb: "available",
    devices: [admin, phone],
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
    api.serverInterfaces.mockResolvedValue([{ name: "Wi-Fi", ip: "192.168.1.10" }]);
    api.serverRemoteDevices.mockResolvedValue([]);
    api.serverRemoteStatus.mockResolvedValue({ version: "0.31.0", apiVersion: 2, works: 12, publicUrl: null, sync: { generation: 1, committedAt: null, indexPending: 3, indexError: null } });
  });

  it("does not call the server outside the desktop app", () => {
    renderSection(false);
    expect(screen.getByText("サーバーの操作はデスクトップアプリで利用できます。")).toBeTruthy();
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

  it("starts the embedded server on the chosen address", async () => {
    api.serverStatus.mockResolvedValue(status());
    api.serverEmbeddedStart.mockResolvedValue(status({ embeddedRunning: true, embeddedUrl: "https://192.168.1.10:38777/" }));
    renderSection();
    fireEvent.click(await screen.findByRole("radio", { name: "この PC" }));
    const start = await screen.findByRole("button", { name: "開始" });
    await waitFor(() => expect((start as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(start);
    await waitFor(() => expect(api.serverEmbeddedStart).toHaveBeenCalledWith({ enabled: true, ip: "192.168.1.10", behindProxy: false, publicUrl: null }));
  });

  it("shows sending progress and lets it be stopped", async () => {
    api.serverStatus.mockResolvedValue(status({
      remote: { url: "https://server.example/", fingerprint: null },
      send: { running: true, phase: "upload", filesTotal: 10, filesDone: 4, bytesTotal: 100, bytesDone: 50, message: null, outsidePaths: 0, skippedNames: 0 },
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
    expect(screen.queryByText("Pixel")).toBeNull();
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

  it("checks for an update without applying it", async () => {
    api.serverStatus.mockResolvedValue(connected());
    api.serverRemoteStatus.mockResolvedValue(remote());
    api.serverCheckRemoteUpdate.mockResolvedValue({});
    renderSection();
    await screen.findByText("Maple に接続しています");
    fireEvent.click(screen.getByRole("button", { name: /更新を確認/ }));
    await waitFor(() => expect(api.serverCheckRemoteUpdate).toHaveBeenCalled());
    expect(api.serverUpdateRemote).not.toHaveBeenCalled();
  });

  it("applies a new version when one is offered", async () => {
    api.serverStatus.mockResolvedValue(connected());
    api.serverRemoteStatus.mockResolvedValue(remote({
      version: "0.35.0",
      update: { auto: true, window: "03:00-06:00", phase: { state: "idle" }, latest: "0.36.0", lastCheckedAt: null, lastError: null, lastAppliedAt: null, skipped: [] },
    }));
    api.serverUpdateRemote.mockResolvedValue(undefined);
    renderSection();
    expect(await screen.findByText("新しいバージョンがあります")).toBeTruthy();
    expect(screen.getByText("次の 3:00〜6:00 に自動で更新します。")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /0\.36\.0 に更新/ }));
    await waitFor(() => expect(api.serverUpdateRemote).toHaveBeenCalled());
  });

  it("offers to retry and blocks server actions when the server is out of reach", async () => {
    api.serverStatus.mockResolvedValue(connected());
    api.serverRemoteStatus.mockRejectedValue("サーバーに接続できません");
    renderSection();
    expect(await screen.findByText("server.example に接続できません")).toBeTruthy();
    expect((screen.getByRole("button", { name: /今すぐ送る/ }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: /携帯を追加/ }) as HTMLButtonElement).disabled).toBe(true);
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

  it("does not issue a pairing until the second step", async () => {
    api.serverStatus.mockResolvedValue(connected());
    api.serverRemoteStatus.mockResolvedValue(remote());
    api.serverRemoteDevices.mockResolvedValue([admin, phone]);
    api.serverCreateReaderPairing.mockResolvedValue({ payload: "{\"version\":2}", expiresInSeconds: 600 });
    renderSection();
    await screen.findByText("Maple に接続しています");
    fireEvent.click(screen.getByRole("button", { name: /携帯を追加/ }));

    // アプリの QR だけが出ていて、次へ進むまで登録の鍵を発行しない。
    expect(await screen.findByRole("img", { name: "Android 版の配布ページ" })).toBeTruthy();
    expect(document.querySelectorAll("svg[aria-label]")).toHaveLength(1);
    expect(api.serverCreateReaderPairing).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "次へ" }));
    expect(await screen.findByRole("img", { name: "登録用の QR" })).toBeTruthy();
    expect(document.querySelectorAll("svg[aria-label]")).toHaveLength(1);
    expect(screen.getByRole("button", { name: "登録文字列をコピー" })).toBeTruthy();
    expect(screen.getByText(/10 分間有効/)).toBeTruthy();

    // 戻って進んでも、鍵を発行し直さない。
    fireEvent.click(screen.getByRole("button", { name: "戻る" }));
    fireEvent.click(await screen.findByRole("button", { name: "次へ" }));
    expect(await screen.findByRole("img", { name: "登録用の QR" })).toBeTruthy();
    expect(api.serverCreateReaderPairing).toHaveBeenCalledTimes(1);
  });

  it("lists phones and keeps the revoke behind a menu when this PC is the server", async () => {
    api.serverStatus.mockResolvedValue(status({ embeddedRunning: true, embeddedUrl: "https://192.168.1.10:38777/", devices: [admin, phone] }));
    renderSection();
    expect(await screen.findByText("この PC で動いています")).toBeTruthy();
    expect(screen.getByText("Pixel")).toBeTruthy();
    expect(screen.queryByText("PC")).toBeNull();
    expect(screen.queryByRole("button", { name: "登録を解除" })).toBeNull();
    expect(screen.getByRole("button", { name: "Pixelの操作" })).toBeTruthy();
  });

  it("says so when the phone has been registered", async () => {
    api.serverStatus.mockResolvedValue(connected());
    api.serverRemoteStatus.mockResolvedValue(remote());
    api.serverRemoteDevices.mockResolvedValue([admin]);
    api.serverCreateReaderPairing.mockResolvedValue({ payload: "{\"version\":2}", expiresInSeconds: 600 });
    renderSection();
    await screen.findByText("Maple に接続しています");
    fireEvent.click(screen.getByRole("button", { name: /携帯を追加/ }));
    await waitFor(() => expect(api.serverRemoteDevices).toHaveBeenCalled());
    await waitFor(() => expect(screen.getByRole("button", { name: "次へ" })).toBeTruthy());
    fireEvent.click(screen.getByRole("button", { name: "次へ" }));
    expect(await screen.findByRole("img", { name: "登録用の QR" })).toBeTruthy();
    api.serverRemoteDevices.mockResolvedValue([admin, phone]);
    expect(await screen.findByText("「Pixel」を登録しました。", undefined, { timeout: 5_000 })).toBeTruthy();
    expect(screen.queryByRole("img", { name: "登録用の QR" })).toBeNull();
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
    const applying = remote({ update: { auto: true, window: "03:00-06:00", phase: { state: "applying", version: "0.36.1" }, latest: "0.36.1", lastCheckedAt: null, lastError: null, lastAppliedAt: null } });
    expect(remoteView({ ...base, details: applying, offline: true }).headline).toBe("0.36.1 に更新しています");
  });

  it("does not offer a version that failed to start", () => {
    const skipped = remote({ version: "0.36.0", update: { auto: true, window: "03:00-06:00", phase: { state: "idle" }, latest: "0.36.1", lastCheckedAt: null, lastError: null, lastAppliedAt: null, skipped: ["0.36.1"] } });
    expect(remoteView({ ...base, details: skipped }).headline).toBe("Maple に接続しています");
  });
});

describe("nextAction", () => {
  it("points at one thing at a time", () => {
    expect(nextAction(undefined, 0)).toBeNull();
    expect(nextAction(remote({ works: null }), 0)).toBe("send");
    expect(nextAction(remote(), 0)).toBe("phone");
    expect(nextAction(remote(), 1)).toBeNull();
    expect(nextAction(remote({ version: "0.35.0" }), 1)).toBe("update");
  });
});
