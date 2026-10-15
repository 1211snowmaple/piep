import { MantineProvider } from "@mantine/core";
import { render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { ScrollingJobTitle } from "./UpdatesPage";

afterEach(() => vi.restoreAllMocks());

it("starts each new title at the beginning with its own scroll distance", () => {
  const widths: Record<string, number> = { "最初の長いタイトル": 520, "次のタイトル": 290, "短い": 120 };
  vi.spyOn(Element.prototype, "clientWidth", "get").mockImplementation(function (this: Element) {
    return this.classList.contains("update-job-title-viewport") ? 200 : 0;
  });
  vi.spyOn(Element.prototype, "scrollWidth", "get").mockImplementation(function (this: Element) {
    return this.classList.contains("update-job-title") ? widths[this.textContent ?? ""] ?? 0 : 0;
  });

  const page = render(<MantineProvider><ScrollingJobTitle label="最初の長いタイトル" /></MantineProvider>);
  const first = screen.getByRole("heading", { name: "最初の長いタイトル" });
  expect(first.style.getPropertyValue("--title-overflow")).toBe("320px");

  page.rerender(<MantineProvider><ScrollingJobTitle label="次のタイトル" /></MantineProvider>);
  const second = screen.getByRole("heading", { name: "次のタイトル" });
  expect(second).not.toBe(first);
  expect(second.style.getPropertyValue("--title-overflow")).toBe("90px");
  expect(second.parentElement).toHaveClass("update-job-title-viewport--scrolling");

  page.rerender(<MantineProvider><ScrollingJobTitle label="短い" /></MantineProvider>);
  const third = screen.getByRole("heading", { name: "短い" });
  expect(third).not.toBe(second);
  expect(third.style.getPropertyValue("--title-overflow")).toBe("0px");
  expect(third.parentElement).not.toHaveClass("update-job-title-viewport--scrolling");
});
