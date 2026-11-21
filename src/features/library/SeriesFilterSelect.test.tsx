import { MantineProvider } from "@mantine/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { searchShelfEntities, type ShelfEntityFacet } from "./shelfV2";
import { SeriesFilterSelect } from "./SeriesFilterSelect";

vi.mock("./shelfV2", () => ({ searchShelfEntities: vi.fn() }));

it("searches series independently and retains provider identity for matching numbers", async () => {
  vi.mocked(searchShelfEntities).mockResolvedValue({ total: 2, items: [
    { source: "pixiv", sourceKey: "123", displayName: "冬の季節の栞", count: 2 },
    { source: "fanbox", sourceKey: "123", displayName: "夏の季節の栞", count: 3 },
  ] as ShelfEntityFacet[] });
  const onChange = vi.fn();
  const user = userEvent.setup();
  render(<MantineProvider env="test"><QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><SeriesFilterSelect runtime value={null} onChange={onChange} /></QueryClientProvider></MantineProvider>);
  expect(searchShelfEntities).not.toHaveBeenCalled();
  const input = screen.getByRole("combobox", { name: "シリーズを指定" });
  await user.click(input);
  expect(searchShelfEntities).not.toHaveBeenCalled();
  await user.type(input, "季節");
  await user.click(await screen.findByRole("option", { name: "夏の季節の栞（fanbox）" }));
  expect(searchShelfEntities).toHaveBeenCalledWith("series", "季節", 30, 0, null, "work_count", null, null);
  expect(onChange).toHaveBeenCalledWith({ key: "123", source: "fanbox", label: "夏の季節の栞" });
});
