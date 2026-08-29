import { MantineProvider } from "@mantine/core";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  getAssetUrl: (path: string | null | undefined) => path ?? null,
}));

import { WorkCover } from "./WorkCover";

const work = {
  coverPath: "C:/library/cover.jpg",
  source: "pixiv",
  sourceId: "123",
  title: "縦長の表紙",
  authorName: "作者",
  personName: "作者",
  seriesTitle: "シリーズ",
};

describe("WorkCover", () => {
  it.each(["card", "compact", "detail"] as const)(
    "shows the whole image in the %s frame",
    (variant) => {
      render(
        <MantineProvider>
          <WorkCover work={work} variant={variant} />
        </MantineProvider>,
      );

      expect(screen.getByRole("img", { name: "縦長の表紙の表紙" })).toHaveStyle({
        "--image-object-fit": "contain",
      });
    },
  );
});
