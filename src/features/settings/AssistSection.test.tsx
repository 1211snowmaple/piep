import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { DEFAULT_ASSIST_SETTINGS, assistTarget, type AssistSettings } from "@/services/assistApi";
import { theme } from "@/theme";
import { AssistSection } from "./AssistSection";

const saved: AssistSettings = {
  ...DEFAULT_ASSIST_SETTINGS,
  enabled: true,
  baseUrl: "http://127.0.0.1:8080/v1",
  model: "qwen3.6-35b-a3b-ud-q4_k_xl",
};
saved.verifiedTarget = assistTarget(saved);

vi.mock("@/services/dbApi", async (original) => ({
  ...(await original<typeof import("@/services/dbApi")>()),
  isTauriRuntime: () => true,
  getSearchIndexStatus: vi.fn(() => new Promise(() => {})),
}));

vi.mock("@/services/assistApi", async (original) => ({
  ...(await original<typeof import("@/services/assistApi")>()),
  loadAssistSettings: vi.fn(async () => saved),
  getAssistRuntimeProfile: vi.fn(() => new Promise(() => {})),
  tryAssistEngine: vi.fn(async () => ({
    name: "シャニマス",
    subtitle: "このタグを持つ6/6作品",
    elapsedMs: 52_899,
    sentTitles: ["放課後の約束", "雨の日の283"],
    candidates: [
      { name: "シャニマス", subtitle: "このタグを持つ6/6作品" },
      { name: "放課後", subtitle: "共通する題名の語幹・2作品" },
    ],
  })),
}));

function renderSection() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <MantineProvider theme={theme}>
      <QueryClientProvider client={client}>
        <AssistSection />
      </QueryClientProvider>
    </MantineProvider>,
  );
}

describe("AssistSection", () => {
  it("shows what the trial sent and which candidate the model picked", async () => {
    const user = userEvent.setup();
    renderSection();
    await screen.findByDisplayValue("http://127.0.0.1:8080/v1");
    await user.click(screen.getByRole("button", { name: "試し書き" }));

    expect(await screen.findByText("答えが返ってきました（53秒）")).toBeInTheDocument();
    expect(screen.getByText("放課後の約束／雨の日の283")).toBeInTheDocument();
    expect(screen.getByText("共通する題名の語幹・2作品")).toBeInTheDocument();
    expect(screen.getByText("「シャニマス」")).toBeInTheDocument();
  });

  it("says why saving is blocked instead of only that something changed", async () => {
    renderSection();
    const url = await screen.findByDisplayValue("http://127.0.0.1:8080/v1");
    fireEvent.change(url, { target: { value: "http://127.0.0.1:1234/v1" } });

    expect(screen.getByText("このつなぎ先とモデルで試し書きをしてから有効にしてください")).toBeInTheDocument();
    expect(screen.queryByText("保存していない変更があります")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "保存" })).toBeDisabled();
  });
});
