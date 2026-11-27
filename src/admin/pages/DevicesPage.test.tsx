import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";
import { ReauthProvider } from "../shared";
import { DevicesPage } from "./DevicesPage";

afterEach(() => vi.unstubAllGlobals());

function renderDevices() {
  render(
    <MantineProvider>
      <ModalsProvider>
        <QueryClientProvider client={new QueryClient()}>
          <ReauthProvider>
            <DevicesPage />
          </ReauthProvider>
        </QueryClientProvider>
      </ModalsProvider>
    </MantineProvider>,
  );
}

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
  renderDevices();

  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: "別の PC へ入れ替える" }));
  await user.click(await screen.findByRole("button", { name: "登録文字列を出す" }));

  expect(await screen.findByRole("alert")).toHaveTextContent(message);
  expect(screen.queryByRole("button", { name: "登録文字列をコピー" })).toBeNull();
});

it("携帯ごとに本棚を読む許可を表示する", async () => {
  const createdAt = "2026-01-01T00:00:00Z";
  vi.stubGlobal("fetch", vi.fn(async (path: string) => {
    expect(path).toBe("/api/admin/status");
    return Response.json({
      devices: [
        { id: "admin", name: "試験 PC", role: "admin", createdAt, readsShelf: false },
        { id: "first", name: "一台目", role: "reader", createdAt, readsShelf: true },
        { id: "second", name: "二台目", role: "reader", createdAt, readsShelf: false },
      ],
    });
  }));
  renderDevices();

  const first = await screen.findByRole("row", { name: /一台目/ });
  expect(within(first).getByText("本棚：許可あり")).toBeInTheDocument();
  const second = screen.getByRole("row", { name: /二台目/ });
  expect(within(second).getByText("本棚：許可なし")).toBeInTheDocument();
  const admin = screen.getByRole("row", { name: /試験 PC/ });
  expect(within(admin).queryByText(/本棚：許可/)).toBeNull();
});

it("本棚を配っていないときも携帯で読めないことが分かる", async () => {
  vi.stubGlobal("fetch", vi.fn(async (path: string) => {
    expect(path).toBe("/api/admin/status");
    return Response.json({
      devices: [{ id: "reader", name: "試験携帯", role: "reader", createdAt: "2026-01-01T00:00:00Z" }],
    });
  }));
  renderDevices();

  const reader = await screen.findByRole("row", { name: /試験携帯/ });
  expect(within(reader).getByText("本棚：配信なし")).toBeInTheDocument();
});
