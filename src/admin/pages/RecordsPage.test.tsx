import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";
import { adminApi } from "../api";
import { RecordsPage } from "./RecordsPage";

afterEach(() => vi.restoreAllMocks());

it("画面と同じ現地時刻に UTC オフセットを付けて表をコピーする", async () => {
  const at = new Date(2026, 10, 28, 6, 30, 0);
  const offset = -at.getTimezoneOffset();
  const hours = String(Math.floor(Math.abs(offset) / 60)).padStart(2, "0");
  const minutes = String(Math.abs(offset) % 60).padStart(2, "0");
  const expectedTime = `2026-11-28 06:30:00 ${offset < 0 ? "-" : "+"}${hours}:${minutes}`;
  vi.spyOn(adminApi, "audit").mockResolvedValue([
    { id: 1, at: at.getTime(), kind: "login", action: "ログイン", ok: true, actor: "管理者", ip: null, detail: null },
  ]);
  const user = userEvent.setup();
  render(
    <MantineProvider>
      <QueryClientProvider client={new QueryClient()}>
        <RecordsPage />
      </QueryClientProvider>
    </MantineProvider>,
  );

  expect(await screen.findByRole("cell", { name: /11\/28 06:30:00$/ })).toBeInTheDocument();
  await user.click(screen.getByRole("button", { name: "表をコピー" }));
  const copied = await navigator.clipboard.readText();
  expect(copied.split("\n")[1]?.split("\t")[0]).toBe(expectedTime);
});
