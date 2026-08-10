import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import SavePage from "./SavePage";

// The handover between the in-app pane and the large window needs the Tauri
// runtime mocked for the whole module, which would change what this preview
// mode test exercises. It lives in SavePage.handover.test.tsx instead.
describe("SavePage browser window affordance", () => {
  beforeEach(() => {
    window.location.hash = "#/save/pixiv";
  });

  it("offers a large app-window action instead of an external-browser action", async () => {
    const open = vi.spyOn(window, "open").mockImplementation(() => null);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <MantineProvider>
        <QueryClientProvider client={client}>
          <AppRouter><SavePage /></AppRouter>
        </QueryClientProvider>
      </MantineProvider>,
    );

    expect(await screen.findByRole("button", { name: "大きいウィンドウで開く" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "外部ブラウザで開く" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "大きいウィンドウで開く" }));
    expect(open).toHaveBeenCalledWith("https://www.pixiv.net/", "_blank", "noopener,noreferrer");
  });
});
