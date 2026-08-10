import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { BackupInspection } from "@/services/archiveApi";
import { RestoreWizard } from "./SettingsPage";

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

function renderWizard(inspection: BackupInspection, onConfirm = vi.fn()) {
  render(<MantineProvider><ModalsProvider><RestoreWizard review={{ path: "C:\\backup.zip", inspection }} loading={false} onClose={vi.fn()} onConfirm={onConfirm} /></ModalsProvider></MantineProvider>);
  return onConfirm;
}

describe("RestoreWizard", () => {
  it("previews validated contents and confirms only after inspection", () => {
    const onConfirm = renderWizard(validInspection);
    expect(screen.getByText("12")).toBeInTheDocument();
    expect(screen.getByText("v3.0")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "検証済みバックアップを復元" }));
    expect(onConfirm).toHaveBeenCalledOnce();
  });

  it("blocks an invalid or space-starved archive", () => {
    renderWizard({ ...validInspection, valid: false, error: "不正なパスを検出しました" });
    expect(screen.getByText("不正なパスを検出しました")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "検証済みバックアップを復元" })).toBeDisabled();
  });
});
