import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { adminApi, type AdminStatus, type Device } from "../api";
import { ReauthProvider } from "../shared";
import { DevicesPage } from "./DevicesPage";

const computer: Device = { id: "pc", name: "書斎の PC", role: "admin", createdAt: "2026-10-01T00:00:00Z", lastSeenAt: null };
const phone: Device = { ...computer, id: "phone", name: "携帯", role: "reader", readsShelf: false };

function status(overrides: Partial<AdminStatus> = {}): AdminStatus {
  return {
    version: "0.42.2",
    apiVersion: 2,
    shelf: { libraryId: "shelf", syncEpoch: "epoch", works: "12" },
    devices: [computer, phone],
    publicUrl: "https://reader.example/",
    update: null,
    serverName: "piep",
    startedAt: computer.createdAt,
    ...overrides,
  };
}

function openDevices(data: AdminStatus) {
  const serverStatus = vi.spyOn(adminApi, "status").mockResolvedValue(data);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <MantineProvider>
      <ModalsProvider>
        <QueryClientProvider client={client}>
          <ReauthProvider><DevicesPage /></ReauthProvider>
        </QueryClientProvider>
      </ModalsProvider>
    </MantineProvider>,
  );
  return { client, serverStatus };
}

describe("サーバー管理画面の端末", () => {
  afterEach(() => vi.restoreAllMocks());

  it("携帯の閲覧許可を切り替え、サーバーから状態を読み直す", async () => {
    const { serverStatus } = openDevices(status());
    const setAccess = vi.spyOn(adminApi, "setReaderAccess").mockResolvedValue(undefined);
    const toggle = await screen.findByRole<HTMLInputElement>("switch", { name: "「携帯」のライブラリ閲覧を許可" });
    expect(toggle.checked).toBe(false);
    expect(screen.queryByRole("switch", { name: /書斎の PC/ })).toBeNull();

    serverStatus.mockResolvedValue(status({ devices: [computer, { ...phone, readsShelf: true }] }));
    fireEvent.click(toggle);
    await waitFor(() => expect(toggle.checked).toBe(true));
    expect(setAccess).toHaveBeenLastCalledWith("phone", true);

    serverStatus.mockResolvedValue(status());
    fireEvent.click(toggle);
    await waitFor(() => expect(toggle.checked).toBe(false));
    expect(setAccess).toHaveBeenLastCalledWith("phone", false);
  });

  it("配信前は送信する操作を案内する", async () => {
    openDevices(status({ shelf: null, devices: [computer, { ...phone, readsShelf: undefined }] }));
    expect(await screen.findByText("配信待ち")).toBeTruthy();
    expect(screen.getByText(/デスクトップ版.*今すぐ送る/)).toBeTruthy();
    expect(screen.queryByRole("switch", { name: /閲覧/ })).toBeNull();
  });

  it("インストールと登録で QR を切り替え、登録後は閲覧許可を案内する", async () => {
    const { client } = openDevices(status({ devices: [computer] }));
    const addReader = vi.spyOn(adminApi, "addReader").mockResolvedValue({ payload: "試験用", expiresInSeconds: 600 });
    await screen.findByText("登録した携帯はありません。");
    fireEvent.click(screen.getByRole("button", { name: "携帯を追加" }));
    expect(await screen.findByRole("img", { name: "Android 版の配布ページ" })).toBeTruthy();
    expect(addReader).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "登録に進む" }));
    expect(await screen.findByRole("img", { name: "登録用の QR" })).toBeTruthy();
    expect(screen.queryByRole("img", { name: "Android 版の配布ページ" })).toBeNull();
    expect(addReader).toHaveBeenCalledWith("https://reader.example/");

    client.setQueryData(["status"], status());
    expect(await screen.findByText(/「携帯」が端末一覧に表示されました.*閲覧を許可/)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "端末一覧へ" }));
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("公開 URL が未設定なら携帯用の HTTPS アドレスを入力できる", async () => {
    openDevices(status({ publicUrl: null }));
    const addReader = vi.spyOn(adminApi, "addReader").mockResolvedValue({ payload: "試験用", expiresInSeconds: 600 });
    await screen.findByRole("switch", { name: /閲覧/ });
    fireEvent.click(screen.getByRole("button", { name: "携帯を追加" }));
    fireEvent.click(await screen.findByRole("button", { name: "登録に進む" }));
    const address = screen.getByLabelText("携帯が接続する HTTPS アドレス");
    fireEvent.change(address, { target: { value: "http://reader.example/" } });
    fireEvent.click(screen.getByRole("button", { name: "登録文字列を作る" }));
    expect(screen.getByRole("alert")).toBeTruthy();
    expect(addReader).not.toHaveBeenCalled();

    fireEvent.change(address, { target: { value: "https://reader.example/piep/" } });
    fireEvent.click(screen.getByRole("button", { name: "登録文字列を作り直す" }));
    await waitFor(() => expect(addReader).toHaveBeenCalledWith("https://reader.example/piep/"));
  });
});
