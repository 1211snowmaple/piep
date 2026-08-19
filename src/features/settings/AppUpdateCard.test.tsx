import { StrictMode } from "react";
import { MantineProvider } from "@mantine/core";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi, beforeEach } from "vitest";
import { theme } from "@/theme";
import { APP_UPDATE_CHECK_KEY } from "@/features/settings/appUpdate";

const checkForAppUpdate = vi.fn();
const downloadAndInstallAppUpdate = vi.fn();
vi.mock("@/services/appUpdateApi", () => ({
  checkForAppUpdate: () => checkForAppUpdate(),
  downloadAndInstallAppUpdate: (...args: unknown[]) => downloadAndInstallAppUpdate(...args),
  restartForAppUpdate: vi.fn(),
}));

import { AppUpdateCard } from "@/features/settings/AppUpdateCard";

function renderCard(runtime = true) {
  render(<MantineProvider theme={theme}><AppUpdateCard runtime={runtime} /></MantineProvider>);
}

describe("AppUpdateCard", () => {
  beforeEach(() => {
    window.localStorage.removeItem(APP_UPDATE_CHECK_KEY);
    checkForAppUpdate.mockReset();
    downloadAndInstallAppUpdate.mockReset();
  });

  it("says the app is current when nothing newer is published", async () => {
    checkForAppUpdate.mockResolvedValue(null);
    renderCard();
    expect(await screen.findByText("最新です")).toBeInTheDocument();
  });

  // 見つけただけでは何も起きない。入れ替えは押したときだけ。
  it("offers the new version without installing it until asked", async () => {
    checkForAppUpdate.mockResolvedValue({ version: "0.8.0", date: null, body: "変更点いろいろ" });
    let finish = () => {};
    downloadAndInstallAppUpdate.mockImplementation(() => new Promise<void>((resolve) => { finish = resolve; }));
    renderCard();

    expect(await screen.findByText("v0.8.0")).toBeInTheDocument();
    expect(screen.getByText("変更点いろいろ")).toBeInTheDocument();
    expect(downloadAndInstallAppUpdate).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "ダウンロードして更新" }));
    await waitFor(() => expect(downloadAndInstallAppUpdate).toHaveBeenCalledTimes(1));
    finish();
    expect(await screen.findByRole("button", { name: "いま再起動する" })).toBeInTheDocument();
  });

  // 署名鍵が未設定のときは、原因の分からない失敗にしない。
  it("explains a failed check instead of showing the raw error alone", async () => {
    checkForAppUpdate.mockRejectedValue(new Error("failed to parse pubkey"));
    renderCard();
    expect(await screen.findByText(/署名鍵/)).toBeInTheDocument();
  });

  it("remembers that the launch check was turned off", async () => {
    checkForAppUpdate.mockResolvedValue(null);
    renderCard();
    fireEvent.click(await screen.findByRole("switch", { name: /起動時に新しい版を確認する/ }));
    expect(window.localStorage.getItem(APP_UPDATE_CHECK_KEY)).toBe("0");
  });

  // StrictMode は effect を一度剥がしてから付け直す。剥がれたことだけを
  // 覚えて立て直さないと、二度目のマウントが自分を「もう居ない」と見なし、
  // 確認結果がどこにも届かないまま回り続ける。
  it("still reports the result after StrictMode remounts it", async () => {
    checkForAppUpdate.mockResolvedValue(null);
    render(<StrictMode><MantineProvider theme={theme}><AppUpdateCard runtime /></MantineProvider></StrictMode>);
    expect(await screen.findByText("最新です")).toBeInTheDocument();
  });

  // プレビューには入れ替える実行ファイルがないので、確認そのものを試みない。
  it("does not check in the browser preview", async () => {
    renderCard(false);
    await waitFor(() => expect(screen.getByText(/ブラウザプレビュー/)).toBeInTheDocument());
    expect(checkForAppUpdate).not.toHaveBeenCalled();
  });
});
