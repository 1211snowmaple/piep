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
  const openWindow = vi.spyOn(window, "open").mockImplementation(() => null);
  render(<MantineProvider><UpdateWorkLinks source="pixiv" sourceId="123" downloadId={42} title="作品" onOpenWork={openWork} /></MantineProvider>);
  fireEvent.click(screen.getByRole("button", { name: "作品の保存した詳細を開く" }));
  expect(openWork).toHaveBeenCalledWith(42);
  fireEvent.click(screen.getByRole("button", { name: "作品の取得元を開く" }));
  expect(openWindow).toHaveBeenCalledWith("https://www.pixiv.net/novel/show.php?id=123", "_blank", "noopener,noreferrer");
  openWindow.mockRestore();
});
