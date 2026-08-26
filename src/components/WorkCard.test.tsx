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

  it("opens the work from non-control card content", () => {
    window.location.hash = "#/library";
    renderCard();

    fireEvent.click(screen.getByText("雨音が止んだ午後、閉館前の図書室で二人はもう一度出会った。"));

    expect(window.location.hash).toBe("#/works/101");
  });

  it("keeps the compact row surface clickable without stealing creator clicks", () => {
    window.location.hash = "#/library";
    const row = renderCard({ compact: true });
    fireEvent.click(screen.getByLabelText("雨上がりの図書室で（表紙なし）"));
    expect(window.location.hash).toBe("#/works/101");
    row.unmount();

    window.location.hash = "#/library";
    renderCard({ compact: true });
    fireEvent.click(screen.getByRole("link", { name: "青葉しおりの作品を見る" }));
    expect(window.location.hash).toBe("#/people/pixiv/8001234");
  });

  // 選ぶ面はカードそのもの。印は中央に大きく置くが、名前と押し込みの状態は
  // カードが持つ - でないとキーボードで選べる場所が無くなる。
  it("makes the whole card the only interactive action in selection mode", () => {
    const onSelect = vi.fn();
    window.location.hash = "#/library";
    const view = renderCard({ selectionMode: true, selected: false, onSelect });
    const card = screen.getByRole("button", { name: /雨上がりの図書室でを選択/ });
    expect(card).toHaveClass("work-card");
    expect(card).toHaveAttribute("aria-pressed", "false");
    expect(view.container.querySelector(".work-card__body")).toHaveAttribute("inert");
    expect(view.container.querySelector(".work-card__footer")).toHaveAttribute("inert");
    expect(view.container.querySelector(".card-select")).not.toBeNull();
    expect(screen.queryByRole("link")).toBeNull();
    expect(screen.getAllByRole("button")).toEqual([card]);

    fireEvent.click(screen.getByText("青葉しおり"));
    expect(onSelect).toHaveBeenCalledWith(demoWorks[0].id, true);
    expect(window.location.hash).toBe("#/library");
  });

  it("shows a clear selected state and lets the card undo it", () => {
    const onSelect = vi.fn();
    const view = renderCard({ selectionMode: true, selected: true, onSelect });

    expect(view.container.querySelector(".work-card")).toHaveAttribute("data-selected", "true");
    expect(view.container.querySelector(".card-select")).toHaveAttribute("data-checked", "true");
    const card = screen.getByRole("button", { name: /雨上がりの図書室でを選択解除/ });
    expect(card).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(card);
    expect(onSelect).toHaveBeenCalledWith(demoWorks[0].id, false);
  });

  it("keeps only the compact selection toggle in the tab order", () => {
    const view = renderCard({ compact: true, selectionMode: true, onSelect: vi.fn() });
    const toggle = screen.getByRole("button", { name: /雨上がりの図書室でを選択/ });
    expect(toggle.closest("[inert]")).toBeNull();
    expect(view.container.querySelector(".work-row__main")).toHaveAttribute("inert");
    expect(view.container.querySelector(".work-row__tags")).toHaveAttribute("inert");
    expect(view.container.querySelector(".work-row__facts")).toHaveAttribute("inert");
    expect(screen.queryByRole("link")).toBeNull();
    expect(screen.getAllByRole("button")).toEqual([toggle]);
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

    expect(screen.getByRole("button", { name: /雨上がりの図書室で：読む/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "雨上がりの図書室で：お気に入りを解除" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "雨上がりの図書室で：EPUBキューに追加" })).toBeInTheDocument();
    // Watching is a toggle on the card, not a link to the update centre.
    expect(screen.getByRole("button", { name: "雨上がりの図書室で：更新監視をオフにする" })).toHaveAttribute("aria-pressed", "true");
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
    expect(screen.getByLabelText("FANBOX")).toBeInTheDocument();
    expect(screen.queryByText("FANBOX")).toBeNull();
  });
});
