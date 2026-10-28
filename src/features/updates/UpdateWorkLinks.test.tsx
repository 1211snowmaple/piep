import { AppRouter } from "@/app/router";
import { MantineProvider } from "@mantine/core";
import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { UpdateWorkLinks, formatUpdateSubtitle } from "./UpdateWorkLinks";

it("shows provider timestamps in the app's local date format", () => {
  expect(formatUpdateSubtitle("蟹紅茶 · 2018-10-06T00:01:55+09:00"))
    .not.toContain("2018-10-06T00:01:55+09:00");
});

it("opens the saved work and its provider from the same row", () => {
  const openWork = vi.fn();
  window.location.hash = "#/updates";
  render(<MantineProvider><AppRouter><UpdateWorkLinks source="pixiv" sourceId="123" downloadId={42} title="作品" onOpenWork={openWork} /></AppRouter></MantineProvider>);
  fireEvent.click(screen.getByRole("button", { name: "作品の保存した詳細を開く" }));
  expect(openWork).toHaveBeenCalledWith(42);
  fireEvent.click(screen.getByRole("button", { name: "作品の取得元を開く" }));
  expect(window.location.hash).toContain("/save/pixiv?url=https%3A%2F%2Fwww.pixiv.net%2Fnovel%2Fshow.php%3Fid%3D123");
});
