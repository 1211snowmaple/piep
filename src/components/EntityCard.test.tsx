import { MantineProvider } from "@mantine/core";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { AppRouter } from "@/app/router";
import { EntityCard } from "@/components/EntityCard";
import type { EntityFacet } from "@/types/library";

const entity: EntityFacet = {
  source: "pixiv",
  sourceKey: "123",
  displayName: "テスト作者",
  count: 1,
  coverPath: null,
};

function renderCard(onToggleWatch = vi.fn()) {
  window.location.hash = "#/library";
  render(
    <MantineProvider>
      <AppRouter>
        <EntityCard entity={entity} kind="person" onToggleWatch={onToggleWatch} />
      </AppRouter>
    </MantineProvider>,
  );
  return onToggleWatch;
}

describe("EntityCard", () => {
  it("keeps keyboard activation of the watch button inside the card", () => {
    const onToggleWatch = renderCard();
    const watchButton = screen.getByRole("button", { name: "この作者の更新を監視する" });

    for (const key of ["Enter", " "]) {
      fireEvent.keyDown(watchButton, { key });
      expect(window.location.hash).toBe("#/library");
    }
    fireEvent.click(watchButton);
    expect(onToggleWatch).toHaveBeenCalledWith(entity, true);
  });

  it("still opens the entity when the card itself receives Enter", () => {
    renderCard();
    fireEvent.keyDown(screen.getByRole("link", { name: "テスト作者を開く" }), { key: "Enter" });
    expect(window.location.hash).toBe("#/people/pixiv/123");
  });
});
