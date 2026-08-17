import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import type { BackupInspection } from "@/services/archiveApi";
import { theme } from "@/theme";
import SettingsPage, { RestoreWizard } from "./SettingsPage";

const validInspection: BackupInspection = {
  valid: true,
  error: null,
  backupVersion: "3.0",
  entryCount: 140,
  compressedBytes: 10_000_000,
  expandedBytes: 32_000_000,
  requiredFreeBytes: 64_000_000,
  availableFreeBytes: 1_000_000_000,
  workCount: 12,
  personCount: 3,
  seriesCount: 2,
  versionCount: 14,
  assetCount: 108,
  warnings: [],
};

function renderWizard(inspection: BackupInspection, onConfirm = vi.fn(), format: "multipart" | "zip" = "zip") {
  const path = format === "multipart" ? "C:\\backups\\piep-backup.json" : "C:\\backup.zip";
  render(<MantineProvider theme={theme}><ModalsProvider><RestoreWizard review={{ path, format, inspection }} loading={false} onClose={vi.fn()} onConfirm={onConfirm} /></ModalsProvider></MantineProvider>);
  return onConfirm;
}

describe("RestoreWizard", () => {
  it("previews validated contents and confirms only after inspection", () => {
    const onConfirm = renderWizard(validInspection);
    expect(screen.getByRole("button", { name: "閉じる" })).toBeInTheDocument();
    expect(screen.getByText("12")).toBeInTheDocument();
    expect(screen.getByText("単一ZIP · v3.0")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "検証済みバックアップを復元" }));
    expect(onConfirm).toHaveBeenCalledOnce();
  });

  it("blocks an invalid or space-starved archive", () => {
    renderWizard({ ...validInspection, valid: false, error: "不正なパスを検出しました" });
    expect(screen.getByText("不正なパスを検出しました")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "検証済みバックアップを復元" })).toBeDisabled();
  });

  it("explains multipart manifests and exposes a native confirmation button", async () => {
    const user = userEvent.setup();
    const onConfirm = renderWizard(validInspection, vi.fn(), "multipart");
    const dialog = screen.getByRole("dialog", { name: "バックアップ復元ウィザード" });

    expect(screen.getByText("選択したJSONマニフェスト")).toBeInTheDocument();
    expect(screen.getByText("ZIPパート合計")).toBeInTheDocument();
    expect(screen.getByText("分割 · v3.0")).toBeInTheDocument();
    expect(screen.getByText(/同じJSONマニフェストをもう一度選ぶと続きから再開/)).toBeInTheDocument();
    expect(screen.getByRole("list", { name: "復元手順" })).toBeInTheDocument();

    const confirm = screen.getByRole("button", { name: "検証済みバックアップを復元" });
    expect(confirm.tagName).toBe("BUTTON");
    expect(confirm).toBeEnabled();
    await user.click(confirm);
    expect(onConfirm).toHaveBeenCalledOnce();
    expect(dialog).toBeInTheDocument();
  });

  it("reports insufficient space with the exact required and available sizes", () => {
    renderWizard({ ...validInspection, requiredFreeBytes: 64_000_000, availableFreeBytes: 8_000_000 }, vi.fn(), "multipart");
    expect(screen.getByRole("alert")).toHaveTextContent("復元には一時領域を含めて 61 MB 必要ですが、利用可能なのは 7.6 MB です");
    expect(screen.getByRole("button", { name: "検証済みバックアップを復元" })).toBeDisabled();
  });
});

describe("SettingsPage navigation", () => {
  it("uses keyboard-operable buttons for settings sections", async () => {
    const user = userEvent.setup();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    window.location.hash = "#/settings";
    render(<MantineProvider theme={theme}><QueryClientProvider client={client}><ModalsProvider><AppRouter><SettingsPage /></AppRouter></ModalsProvider></QueryClientProvider></MantineProvider>);

    const about = await screen.findByRole("button", { name: /piepについて/ });
    expect(about).toHaveAttribute("type", "button");
    about.focus();
    await user.keyboard("{Enter}");
    await waitFor(() => expect(window.location.hash).toContain("section=about"));
    expect(about).toHaveAttribute("aria-current", "page");
  });
});
