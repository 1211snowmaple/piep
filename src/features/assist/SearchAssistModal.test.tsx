import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { SearchAssistModal } from "@/features/assist/SearchAssistModal";

const interpretSearch = vi.hoisted(() => vi.fn());
const assist = vi.hoisted(() => ({ engine: null as object | null }));
vi.mock("@/features/assist/useAssist", () => ({
  useAssist: () => assist,
}));

afterEach(() => { cleanup(); assist.engine = null; vi.clearAllMocks(); });
vi.mock("@/services/assistApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/assistApi")>()),
  interpretSearch,
}));

it("searches directly without an LLM and keeps the entered meaning", () => {
  const onApply = vi.fn();
  const onClose = vi.fn();
  const client = new QueryClient();
  render(
    <MantineProvider>
      <QueryClientProvider client={client}>
        <SearchAssistModal opened onClose={onClose} onApply={onApply} />
      </QueryClientProvider>
    </MantineProvider>,
  );
  fireEvent.change(screen.getByRole("textbox", { name: "どんなものが読みたいか" }), {
    target: { value: "  旅先で出会った二人  " },
  });
  fireEvent.click(screen.getByRole("button", { name: "この文で意味検索" }));

  expect(onApply).toHaveBeenCalledWith({
    includeTags: [],
    excludeTags: [],
    query: "旅先で出会った二人",
    reading: "入力した文をそのまま探します",
  });
  expect(onClose).toHaveBeenCalledOnce();
  expect(interpretSearch).not.toHaveBeenCalled();
});

it("ignores a reply for a phrase edited while the request is pending", async () => {
  assist.engine = { model: "test" };
  let resolve!: (value: object) => void;
  interpretSearch.mockReturnValueOnce(new Promise((done) => { resolve = done; }));
  render(
    <MantineProvider><QueryClientProvider client={new QueryClient()}>
      <SearchAssistModal opened onClose={vi.fn()} onApply={vi.fn()} />
    </QueryClientProvider></MantineProvider>,
  );
  const field = screen.getByRole("textbox", { name: "どんなものが読みたいか" });
  fireEvent.change(field, { target: { value: "古い条件" } });
  fireEvent.click(screen.getByRole("button", { name: "AIで条件に言い換える" }));
  await waitFor(() => expect(interpretSearch).toHaveBeenCalledOnce());
  fireEvent.change(field, { target: { value: "新しい条件" } });
  await act(async () => { resolve({ includeTags: ["古いタグ"], excludeTags: [], query: "", reading: "古い応答" }); });
  await waitFor(() => expect(screen.getByRole("button", { name: "AIで条件に言い換える" })).not.toHaveAttribute("data-loading"));
  expect(screen.queryByText("古い応答")).toBeNull();
  expect(screen.getByRole("button", { name: "この条件で棚を絞る" })).toBeDisabled();
});

it("ignores a reply after closing and reopening the modal", async () => {
  assist.engine = { model: "test" };
  let resolve!: (value: object) => void;
  interpretSearch.mockReturnValueOnce(new Promise((done) => { resolve = done; }));
  const client = new QueryClient();
  const view = (opened: boolean) => <MantineProvider><QueryClientProvider client={client}>
    <SearchAssistModal opened={opened} onClose={vi.fn()} onApply={vi.fn()} />
  </QueryClientProvider></MantineProvider>;
  const result = render(view(true));
  fireEvent.change(screen.getByRole("textbox", { name: "どんなものが読みたいか" }), { target: { value: "海辺" } });
  fireEvent.click(screen.getByRole("button", { name: "AIで条件に言い換える" }));
  await waitFor(() => expect(interpretSearch).toHaveBeenCalledOnce());
  result.rerender(view(false));
  result.rerender(view(true));
  await act(async () => { resolve({ includeTags: ["海"], excludeTags: [], query: "", reading: "前の応答" }); });
  await waitFor(() => expect(screen.queryByText("前の応答")).toBeNull());
  expect(screen.getByRole("button", { name: "この条件で棚を絞る" })).toBeDisabled();
});
