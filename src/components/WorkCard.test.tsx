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
    expect(onSelect).toHaveBeenCalledWith(demoWorks[0].id, true);
    expect(window.location.hash).toBe("#/library");
  });

  it("filters the library by a tag without opening the work", () => {
    renderCard({ work: { ...demoWorks[0], tags: ["創作", "青春", "短編", "恋愛", "雨", "図書室"] } });

    // Every tag is rendered; how many stay visible is decided by measurement,
    // which needs a real layout and so is exercised in the browser instead.
    expect(screen.getByText("図書室")).toBeInTheDocument();
    window.location.hash = "#/library";
    fireEvent.click(screen.getByText("雨"));
    expect(window.location.hash).toBe(`#/library?q=${encodeURIComponent("tag:雨")}`);
  });

  it("keeps frequent actions directly available on the card", () => {
    renderCard();

    expect(screen.getByRole("button", { name: /雨上がりの図書室でを読む/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "お気に入りを解除" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "EPUBキューに追加" })).toBeInTheDocument();
    // Watching is a toggle on the card, not a link to the update centre.
    expect(screen.getByRole("button", { name: "更新監視をオフにする" })).toHaveAttribute("aria-pressed", "true");
  });

  it("only offers the version chip once a work has more than one revision", () => {
    const single = renderCard({ work: { ...demoWorks[0], currentVersion: 1 } });
    expect(screen.queryByRole("button", { name: /バージョン履歴/ })).toBeNull();
    single.unmount();

    renderCard({ work: { ...demoWorks[0], currentVersion: 3 } });
    window.location.hash = "#/library";
    fireEvent.click(screen.getByRole("button", { name: /バージョン履歴/ }));
    expect(window.location.hash).toBe("#/works/101?tab=history");
  });

  it("uses the shared no-cover treatment and keeps the provider visible", () => {
    renderCard({ work: { ...demoWorks[0], source: "fanbox", coverPath: null } });

    expect(screen.getByLabelText("雨上がりの図書室で（表紙なし）")).toBeInTheDocument();
    expect(screen.getByText("FANBOX")).toBeInTheDocument();
  });
});
