import { MantineProvider } from "@mantine/core";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { AppRouter } from "@/app/router";
import { CollectionCard } from "@/features/collections/CollectionCard";
import { demoCollections } from "@/mocks/demoData";
import { withClippedText } from "@/test/clipped";

const longName = demoCollections[1];

function renderCard() {
  return render(
    <MantineProvider>
      <AppRouter>
        <CollectionCard collection={longName} />
      </AppRouter>
    </MantineProvider>,
  );
}

describe("CollectionCard", () => {
  it("hangs the full name off the name, not off the whole card", async () => {
    withClippedText(renderCard);

    // 表紙の上でも作品数の上でも同じ吹き出しが出ていた。カード全体が的だと、
    // 名前ではなくカードの中央——一つ上のカードの上——に浮かぶ。
    fireEvent.mouseEnter(screen.getByText(`${longName.memberCount}作品`));
    await new Promise((resolve) => setTimeout(resolve, 500));
    expect(screen.queryByRole("tooltip")).toBeNull();

    fireEvent.mouseEnter(screen.getByText(longName.name));
    expect(await screen.findByRole("tooltip")).toHaveTextContent(longName.name);
  });

  it("leaves a name that fits without a tooltip repeating it", async () => {
    renderCard();

    fireEvent.mouseEnter(screen.getByText(longName.name));
    await new Promise((resolve) => setTimeout(resolve, 500));
    expect(screen.queryByRole("tooltip")).toBeNull();
  });
});
