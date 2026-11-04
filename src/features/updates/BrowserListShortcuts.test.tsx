import { fireEvent, render, screen } from "@testing-library/react";
import { MantineProvider } from "@mantine/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { consumeListRead, resetSaveDraftsForTest } from "@/features/save/saveDraft";
import { BrowserListShortcuts } from "./BrowserListShortcuts";

const navigate = vi.hoisted(() => vi.fn());
vi.mock("@/app/router", () => ({ useAppNavigate: () => navigate }));

describe("BrowserListShortcuts", () => {
  beforeEach(() => {
    navigate.mockReset();
    resetSaveDraftsForTest();
  });

  it("保存の画面で一覧を開き、一度だけ読むよう頼む", () => {
    render(<MantineProvider><BrowserListShortcuts /></MantineProvider>);
    expect(consumeListRead("pixiv")).toBe(false);

    fireEvent.click(screen.getByRole("button", { name: "フォロー新着" }));

    expect(navigate).toHaveBeenCalledWith(`/save/pixiv?url=${encodeURIComponent("https://www.pixiv.net/novel/bookmark_new.php")}`);
    expect(consumeListRead("pixiv")).toBe(true);
    expect(consumeListRead("pixiv")).toBe(false);
  });
});
