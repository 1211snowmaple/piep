import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import { RepairAttemptLimitCard } from "./RepairAttemptLimitCard";

const api = vi.hoisted(() => ({ read: vi.fn(), write: vi.fn() }));
vi.mock("@/services/shelfApi", () => ({ shelfRepairAttemptLimit: api.read, shelfSetRepairAttemptLimit: api.write }));

beforeEach(() => {
  vi.clearAllMocks();
  api.read.mockResolvedValue({ value: 2, min: 1, max: 5 });
  api.write.mockImplementation(async (value: number) => ({ value, min: 1, max: 5 }));
});

const renderCard = (runtime = true) => render(<MantineProvider><QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><RepairAttemptLimitCard runtime={runtime} /></QueryClientProvider></MantineProvider>);

it("shows the shelf's limit and writes a new one straight away", async () => {
  renderCard();
  const input = screen.getByRole("textbox", { name: "欠けた作品を取り直す回数" });
  await waitFor(() => expect(input).toHaveValue("2 回"));
  const user = userEvent.setup();
  await user.clear(input);
  await user.type(input, "3");
  await waitFor(() => expect(api.write).toHaveBeenCalledWith(3));
  await waitFor(() => expect(input).toHaveValue("3 回"));
});

it("does not write a value outside the range", async () => {
  renderCard();
  const input = screen.getByRole("textbox", { name: "欠けた作品を取り直す回数" });
  await waitFor(() => expect(input).toHaveValue("2 回"));
  const user = userEvent.setup();
  await user.clear(input);
  await user.type(input, "0");
  expect(api.write).not.toHaveBeenCalled();
});

it("stays in place but disabled outside the app", () => {
  renderCard(false);
  expect(screen.getByRole("textbox", { name: "欠けた作品を取り直す回数" })).toBeDisabled();
  expect(api.read).not.toHaveBeenCalled();
});
