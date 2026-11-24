import { MantineProvider } from "@mantine/core";
import { Notifications } from "@mantine/notifications";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { theme } from "@/theme";
import { PREVIEW_ADVANCED_SPECS, defaultAdvancedValues } from "@/services/advancedSettingsApi";
import { formatReadingTime } from "@/features/reader/ReaderNextUp";
import { applyAdvancedSettings, operationJobsKept } from "@/lib/advancedSettings";
import { ADVANCED_DESCRIPTIONS, AdvancedSection } from "./AdvancedSection";

const api = vi.hoisted(() => ({ get: vi.fn(), save: vi.fn() }));

vi.mock("@/services/advancedSettingsApi", async (original) => ({
  ...(await original<typeof import("@/services/advancedSettingsApi")>()),
  getAdvancedSettings: api.get,
  saveAdvancedSettings: api.save,
}));

function renderSection() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><MantineProvider theme={theme}><Notifications /><AdvancedSection /></MantineProvider></QueryClientProvider>);
}

const defaults = () => ({ specs: PREVIEW_ADVANCED_SPECS, values: defaultAdvancedValues(PREVIEW_ADVANCED_SPECS) });

beforeEach(() => {
  api.get.mockResolvedValue(defaults());
  api.save.mockImplementation(async (values: Record<string, number>) => ({ specs: PREVIEW_ADVANCED_SPECS, values }));
  applyAdvancedSettings(defaults());
});

describe("advanced settings", () => {
  it("explains every item Rust offers", () => {
    for (const spec of PREVIEW_ADVANCED_SPECS) expect(ADVANCED_DESCRIPTIONS[spec.key], spec.key).toBeTruthy();
  });

  it("shows intervals in seconds and saves them back in milliseconds", async () => {
    renderSection();
    const gap = await screen.findByRole("textbox", { name: "取得元へ問い合わせる間隔" });
    expect(gap).toHaveValue("0.8 秒");
    expect(screen.getByText("既定 0.8 秒・0.4 秒〜60 秒")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "取得制限に当たった作品をやり直す回数" })).toHaveValue("3 回");
    expect(screen.getByRole("textbox", { name: "画像を同時に取る数" })).toHaveValue("2 本");
    fireEvent.change(gap, { target: { value: "1.5" } });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(api.save).toHaveBeenCalled());
    expect(api.save.mock.calls[0][0]).toMatchObject({ requestGapMs: 1_500, updateCheckDelayMs: 2_000 });
  });

  it("does not let a value faster than the floor be saved", async () => {
    renderSection();
    const gap = await screen.findByRole("textbox", { name: "取得元へ問い合わせる間隔" });
    fireEvent.change(gap, { target: { value: "0.3" } });
    expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
    expect(gap).toHaveAttribute("aria-invalid", "true");
    expect(api.save).not.toHaveBeenCalled();
  });

  it("tells the reason when Rust refuses", async () => {
    api.save.mockRejectedValueOnce("「AI の応答を待つ時間」は 60 秒〜1800 秒 の範囲で決めてください");
    renderSection();
    const timeout = await screen.findByRole("textbox", { name: "AI の応答を待つ時間" });
    fireEvent.change(timeout, { target: { value: "600" } });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    expect(await screen.findByText(/60 秒〜1800 秒 の範囲で/)).toBeInTheDocument();
  });

  it("puts the screen-side values into effect when saved", async () => {
    expect(formatReadingTime(550 * 60)).toBe("約1時間");
    renderSection();
    fireEvent.change(await screen.findByRole("textbox", { name: "読む速さ" }), { target: { value: "1100" } });
    fireEvent.change(screen.getByRole("textbox", { name: "処理の記録を残す件数" }), { target: { value: "300" } });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(operationJobsKept()).toBe(300));
    expect(formatReadingTime(550 * 60)).toBe("約30分");
  });

  it("returns every item to its default", async () => {
    api.get.mockResolvedValue({ specs: PREVIEW_ADVANCED_SPECS, values: { ...defaultAdvancedValues(PREVIEW_ADVANCED_SPECS), keepUpdateJobs: 200 } });
    renderSection();
    expect(await screen.findByRole("textbox", { name: "更新の確認の履歴を残す件数" })).toHaveValue("200 件");
    fireEvent.click(screen.getByRole("button", { name: "既定に戻す" }));
    expect(screen.getByRole("textbox", { name: "更新の確認の履歴を残す件数" })).toHaveValue("50 件");
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    await waitFor(() => expect(api.save).toHaveBeenCalledWith(defaultAdvancedValues(PREVIEW_ADVANCED_SPECS)));
  });
});
