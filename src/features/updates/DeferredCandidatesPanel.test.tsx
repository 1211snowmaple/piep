import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
const mocks = vi.hoisted(() => ({ list: vi.fn(), recheck: vi.fn() }));
vi.mock("@/services/updateJobApi", () => ({ listDeferredUpdateCandidatesCommand: mocks.list, recheckDeferredUpdateCandidatesCommand: mocks.recheck }));
import { DeferredCandidatesPanel } from "./DeferredCandidatesPanel";

it("keeps held works reversible and checks only the explicit selection", async () => {
  mocks.list.mockResolvedValue([
    { source: "fanbox", sourceId: "11", title: "支援プラン待ち", status: "held", payloadJson: '{"holdReason":"月額1000円以上"}' },
    { source: "fanbox", sourceId: "12", title: "今は保存しない", status: "dismissed", payloadJson: '{}' },
  ]);
  mocks.recheck.mockResolvedValue({ jobId: "permission-check" });
  const onRechecked = vi.fn();
  render(<MantineProvider><QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><DeferredCandidatesPanel runtime onRechecked={onRechecked} /></QueryClientProvider></MantineProvider>);
  await screen.findByText("支援プラン待ち");
  expect(screen.getByRole("button", { name: "権限変更後に再確認（0件）" })).toBeDisabled();
  expect(mocks.recheck).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("checkbox", { name: "支援プラン待ちを再確認" }));
  fireEvent.click(screen.getByRole("button", { name: "権限変更後に再確認（1件）" }));
  await waitFor(() => expect(mocks.recheck).toHaveBeenCalledWith(["fanbox:11"], false));
  expect(onRechecked).toHaveBeenCalledWith("permission-check");
});
