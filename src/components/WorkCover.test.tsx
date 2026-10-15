import { MantineProvider } from "@mantine/core";
import { fireEvent, render, screen } from "@testing-library/react";
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

  /**
   * 作品ページの枠は 2/3 に固定されていた。pixiv の表紙は 240x338（0.71）なので
   * 幅で合わされ、上下に余白ができて**カードより小さく縮む**。切れてはいないが、
   * 並べると詰まって見える。読み込めた表紙の比を枠へ渡して、表紙自身の形にする。
   */
  it("hands the frame the ratio of the cover it actually loaded", () => {
    const { container } = render(
      <MantineProvider>
        <WorkCover work={work} variant="detail" />
      </MantineProvider>,
    );
    const frame = container.querySelector(".work-cover") as HTMLElement;
    expect(frame.style.getPropertyValue("--work-cover-ratio")).toBe("");

    const image = screen.getByRole("img", { name: "縦長の表紙の表紙" });
    // jsdom は実際に読まないので、pixiv の表紙と同じ寸法を持たせる。
    Object.defineProperty(image, "naturalWidth", { value: 240, configurable: true });
    Object.defineProperty(image, "naturalHeight", { value: 338, configurable: true });
    fireEvent.load(image);

    expect(frame.style.getPropertyValue("--work-cover-ratio")).toBe(String(240 / 338));
  });

  /** 寸法の分からない画像で枠を潰さない。既定の 2/3 のままにする。 */
  it("leaves the frame alone when the image reports no size", () => {
    const { container } = render(
      <MantineProvider>
        <WorkCover work={work} variant="detail" />
      </MantineProvider>,
    );
    const image = screen.getByRole("img", { name: "縦長の表紙の表紙" });
    Object.defineProperty(image, "naturalWidth", { value: 0, configurable: true });
    Object.defineProperty(image, "naturalHeight", { value: 0, configurable: true });
    fireEvent.load(image);

    const frame = container.querySelector(".work-cover") as HTMLElement;
    expect(frame.style.getPropertyValue("--work-cover-ratio")).toBe("");
  });
});
