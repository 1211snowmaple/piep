import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { theme } from "@/theme";
import type { DesktopServerStatus } from "@/services/serverApi";

const api = vi.hoisted(() => ({
  serverStatus: vi.fn(),
  serverInterfaces: vi.fn(),
  serverEmbeddedStart: vi.fn(),
  serverEmbeddedStop: vi.fn(),
  serverCreateReaderPairing: vi.fn(),
  serverRevokeDevice: vi.fn(),
  serverRemoteDevices: vi.fn(),
  serverRemoteStatus: vi.fn(),
  serverConnectRemote: vi.fn(),
  serverDisconnectRemote: vi.fn(),
  serverSetAutoSend: vi.fn(),
  serverSendNow: vi.fn(),
  serverSendStop: vi.fn(),
}));
vi.mock("@/services/serverApi", () => api);

import { ServerSection } from "@/features/settings/ServerSection";

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
    fireEvent.click(screen.getByRole("button", { name: "つなぐ" }));
    await waitFor(() => expect(api.serverConnectRemote).toHaveBeenCalledWith("  {\"version\":2}  "));
    await waitFor(() => expect((input as HTMLTextAreaElement).value).toBe(""));
  });

  it("starts the embedded server on the chosen address", async () => {
    api.serverStatus.mockResolvedValue(status());
    api.serverEmbeddedStart.mockResolvedValue(status({ embeddedRunning: true, embeddedUrl: "https://192.168.1.10:38777/" }));
    renderSection();
    const start = await screen.findByRole("button", { name: "始める" });
    await waitFor(() => expect((start as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(start);
    await waitFor(() => expect(api.serverEmbeddedStart).toHaveBeenCalledWith({ enabled: true, ip: "192.168.1.10", behindProxy: false, publicUrl: null }));
  });

  it("shows sending progress and lets it be stopped", async () => {
    api.serverStatus.mockResolvedValue(status({
      remote: { url: "https://server.example/", fingerprint: null },
      send: { running: true, phase: "upload", filesTotal: 10, filesDone: 4, bytesTotal: 100, bytesDone: 50, message: null, outsidePaths: 0, skippedNames: 0 },
    }));
    api.serverSendStop.mockResolvedValue(status());
    renderSection();
    expect(await screen.findByText("ファイルを送っています")).toBeTruthy();
    expect(await screen.findByText(/12 作品/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "止める" }));
    await waitFor(() => expect(api.serverSendStop).toHaveBeenCalled());
  });

  it("offers a QR for a phone and does not offer to revoke the admin", async () => {
    api.serverStatus.mockResolvedValue(status({ remote: { url: "https://server.example/", fingerprint: null } }));
    api.serverRemoteDevices.mockResolvedValue([
      { id: "a", name: "PC", role: "admin", createdAt: "2026-11-01T00:00:00Z", lastSeenAt: null, lastUser: null },
      { id: "b", name: "Pixel", role: "reader", createdAt: "2026-11-01T00:00:00Z", lastSeenAt: null, lastUser: null },
    ]);
    api.serverCreateReaderPairing.mockResolvedValue({ payload: "{\"version\":2}", expiresInSeconds: 600 });
    renderSection();
    expect(await screen.findByText("Pixel")).toBeTruthy();
    expect(screen.getAllByRole("button", { name: "登録を解除" })).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "携帯を追加" }));
    expect(await screen.findByRole("button", { name: "登録文字列をコピー" })).toBeTruthy();
    expect(screen.getByText(/10 分だけ有効/)).toBeTruthy();
  });

  it("reports a failed action", async () => {
    api.serverStatus.mockResolvedValue(status({ remote: { url: "https://server.example/", fingerprint: null } }));
    api.serverSendNow.mockRejectedValue("サーバーに接続できません");
    renderSection();
    fireEvent.click(await screen.findByRole("button", { name: "今すぐ送る" }));
    expect(await screen.findByText("サーバーに接続できません")).toBeTruthy();
  });
});
