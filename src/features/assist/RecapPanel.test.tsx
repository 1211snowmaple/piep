import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ loadNote: vi.fn(), recapPrevious: vi.fn() }));
vi.mock("@/features/assist/useAssist", () => ({ useAssist: () => ({ engine: null, body: false, settings: undefined }) }));
vi.mock("@/app/PageAssistContext", () => ({ usePageAssist: () => false }));
vi.mock("@/app/router", () => ({ useAppNavigate: () => vi.fn() }));
vi.mock("@/services/dbApi", () => ({ isTauriRuntime: () => true }));
vi.mock("@/services/assistApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/assistApi")>()),
  loadNote: mocks.loadNote,
  recapPrevious: mocks.recapPrevious,
}));
import { RecapPanel } from "@/features/assist/RecapPanel";
afterEach(() => vi.clearAllMocks());

it("reads the saved recap context without generating a new answer", async () => {
  const text = "前話の主欄。\n\n［本文の末尾・確認用の原文］\n\n原文にある欄名。";
  mocks.loadNote.mockResolvedValue({ text, sourceTail: "前話の末尾原文。 \t", modelId: "test-model", promptVersion: "reader_recap/v11" });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<MantineProvider><QueryClientProvider client={client}><RecapPanel currentId={2} previous={{ id: 1, title: "前話" }} /></QueryClientProvider></MantineProvider>);
  expect(await screen.findByText((_, element) => element?.textContent === text)).toBeVisible();
  expect(mocks.loadNote).toHaveBeenCalledWith("work", "2:1", "recap", null);
  expect(screen.getByText("本文末尾の原文を確認").closest("details")).not.toHaveAttribute("open");
  expect(screen.getByText((_, element) => element?.textContent === "前話の末尾原文。 \t")).not.toBeVisible();
  expect(mocks.recapPrevious).not.toHaveBeenCalled();
});
