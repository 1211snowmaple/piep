import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";
import { ReauthProvider } from "../shared";
import { DevicesPage } from "./DevicesPage";

afterEach(() => vi.unstubAllGlobals());

it("管理者の入れ替えを断られた理由を端末ページに表示する", async () => {
  const message = "このアドレスはこの PC でしか使えません。登録する端末から届く URL を使ってください";
  const fetch = vi.fn(async (path: string) => {
    if (path === "/api/admin/status") {
      return Response.json({
        devices: [{ id: "admin", name: "試験端末", role: "admin", createdAt: "2026-01-01T00:00:00Z" }],
      });
    }
    expect(path).toBe("/api/admin/pairings");
    return Response.json({ code: "invalid_request", message }, { status: 400 });
  });
  vi.stubGlobal("fetch", fetch);
  const client = new QueryClient();
  render(
    <MantineProvider>
      <ModalsProvider>
        <QueryClientProvider client={client}>
          <ReauthProvider>
            <DevicesPage />
          </ReauthProvider>
        </QueryClientProvider>
      </ModalsProvider>
    </MantineProvider>,
  );

  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: "別の PC へ入れ替える" }));
  await user.click(await screen.findByRole("button", { name: "登録文字列を出す" }));

  expect(await screen.findByRole("alert")).toHaveTextContent(message);
  expect(screen.queryByRole("button", { name: "登録文字列をコピー" })).toBeNull();
});
