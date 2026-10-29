import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { PageRefreshControl } from "./PageRefreshControl";

it("refreshes visible data without replacing the page or discarding input", async () => {
  const read = vi.fn().mockResolvedValueOnce("保存前").mockResolvedValue("保存後");
  const expensive = vi.fn();
  function Page() {
    const query = useQuery({ queryKey: ["work"], queryFn: read, staleTime: Infinity });
    useQuery({ queryKey: ["library-diagnostics"], queryFn: expensive, enabled: false });
    return <><PageRefreshControl /><input aria-label="編集中" defaultValue="下書き" /><span>{query.data}</span></>;
  }
  render(<MantineProvider><QueryClientProvider client={new QueryClient()}><Page /></QueryClientProvider></MantineProvider>);
  expect(await screen.findByText("保存前")).toBeInTheDocument();
  const input = screen.getByRole("textbox");
  fireEvent.change(input, { target: { value: "保存していない文章" } });
  fireEvent.click(screen.getByRole("button", { name: "画面を更新" }));
  expect(await screen.findByText("保存後")).toBeInTheDocument();
  expect(input).toHaveValue("保存していない文章");
  input.focus();
  const event = new KeyboardEvent("keydown", { key: "F5", bubbles: true, cancelable: true });
  input.dispatchEvent(event);
  expect(event.defaultPrevented).toBe(true);
  await waitFor(() => expect(read).toHaveBeenCalledTimes(3));
  expect(expensive).not.toHaveBeenCalled();
});
