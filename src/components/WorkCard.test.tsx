import { MantineProvider } from "@mantine/core";
import { ModalsProvider } from "@mantine/modals";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { WorkspaceProvider } from "@/app/WorkspaceContext";
import { WorkCard } from "@/components/WorkCard";
import { demoWorks } from "@/mocks/demoData";

function renderCard(props: Partial<React.ComponentProps<typeof WorkCard>> = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <MantineProvider>
      <QueryClientProvider client={client}>
        <ModalsProvider>
          <AppRouter>
            <WorkspaceProvider>
              <WorkCard work={demoWorks[0]} {...props} />
            </WorkspaceProvider>
          </AppRouter>
        </ModalsProvider>
      </QueryClientProvider>
    </MantineProvider>,
  );
}

describe("WorkCard", () => {
  it("is keyboard accessible and opens the work route", () => {
    window.location.hash = "#/library";
    renderCard();
    fireEvent.keyDown(screen.getByRole("link", { name: /雨上がりの図書室でを開く/ }), { key: "Enter" });
    expect(window.location.hash).toBe("#/works/101");
  });

  it("toggles selection instead of navigating in selection mode", () => {
    const onSelect = vi.fn();
    window.location.hash = "#/library";
    renderCard({ selectionMode: true, selected: false, onSelect });
    fireEvent.keyDown(screen.getByRole("link", { name: /雨上がりの図書室でを開く/ }), { key: " " });
    expect(onSelect).toHaveBeenCalledWith(true);
    expect(window.location.hash).toBe("#/library");
  });

  it("exposes clickable tags without opening the work", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <MantineProvider>
        <QueryClientProvider client={client}>
          <ModalsProvider>
            <AppRouter>
              <WorkspaceProvider>
                <WorkCard
                  work={{ ...demoWorks[0], tags: ["創作", "青春", "短編", "恋愛", "雨", "図書室"] }}
                />
              </WorkspaceProvider>
            </AppRouter>
          </ModalsProvider>
        </QueryClientProvider>
      </MantineProvider>,
    );

    const overflow = screen.getByLabelText("残りのタグ 恋愛、雨、図書室");
    expect(overflow).toHaveTextContent("+3");
    window.location.hash = "#/library";
    fireEvent.click(overflow);
    expect(await screen.findByRole("button", { name: "#雨" })).toBeInTheDocument();
    expect(window.location.hash).toBe("#/library");
  });

  it("keeps frequent actions directly available on the card", () => {
    renderCard();

    expect(screen.getByRole("button", { name: /雨上がりの図書室でを読む/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "お気に入りを解除" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "更新状況を確認" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "EPUBキューに追加" })).toBeInTheDocument();
  });

  it("uses the shared no-cover treatment and keeps the provider visible", () => {
    renderCard({ work: { ...demoWorks[0], source: "fanbox", coverPath: null } });

    expect(screen.getByLabelText("雨上がりの図書室で（表紙なし）")).toBeInTheDocument();
    expect(screen.getByText("FANBOX")).toBeInTheDocument();
  });
});
