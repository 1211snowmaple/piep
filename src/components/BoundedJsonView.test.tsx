import { MantineProvider } from "@mantine/core";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { BoundedJsonView, JSON_INITIAL_RENDER_CHARS, JSON_MAX_RENDER_CHARS } from "@/components/BoundedJsonView";

describe("BoundedJsonView", () => {
  it("renders a bounded prefix and reveals large JSON incrementally", () => {
    const payload = { body: "x".repeat(JSON_MAX_RENDER_CHARS + 100_000) };
    const view = render(<MantineProvider><BoundedJsonView value={payload} /></MantineProvider>);
    const pre = view.container.querySelector("pre");

    expect(pre?.textContent?.length).toBeLessThan(JSON_INITIAL_RENDER_CHARS + 200);
    expect(screen.getByText(/残り.+文字は省略/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "さらに表示" }));
    expect(pre?.textContent?.length).toBeGreaterThan(JSON_INITIAL_RENDER_CHARS);
    expect(screen.getByRole("button", { name: "全体をコピー" })).toBeInTheDocument();
  });

  it("handles values that cannot be stringified", () => {
    const cyclic: Record<string, unknown> = {};
    cyclic.self = cyclic;
    render(<MantineProvider><BoundedJsonView value={cyclic} /></MantineProvider>);
    expect(screen.getByText(/JSONを表示できません/)).toBeInTheDocument();
  });
});
