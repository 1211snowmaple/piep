import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
const openUrl = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl }));

import {
  openExternalUrl,
  openFilesystemPath,
  revealPathInFileManager,
} from "@/services/openerApi";

describe("openerApi", () => {
  beforeEach(() => {
    invoke.mockReset();
    openUrl.mockReset();
  });

  it("routes filesystem operations through validated Rust commands", async () => {
    invoke.mockResolvedValue(undefined);
    await openFilesystemPath("C:/library/work");
    expect(invoke).toHaveBeenLastCalledWith("open_managed_path", {
      path: "C:/library/work",
    });

    await revealPathInFileManager("C:/library/work/content.json");
    expect(invoke).toHaveBeenLastCalledWith("reveal_managed_path", {
      path: "C:/library/work/content.json",
    });
  });

  it("leaves scoped external URLs with the opener plugin", async () => {
    openUrl.mockResolvedValue(undefined);
    await openExternalUrl("https://example.com/");
    expect(openUrl).toHaveBeenCalledWith("https://example.com/");
  });
});
