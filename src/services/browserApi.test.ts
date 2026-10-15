import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { closeStandaloneBrowser, openStandaloneBrowser } from "@/services/browserApi";

describe("standalone browser API", () => {
  beforeEach(() => invoke.mockReset());

  it("passes the provider and optional user agent to the bounded native command", async () => {
    invoke.mockResolvedValueOnce(false);
    await expect(openStandaloneBrowser("https://www.fanbox.cc/", {
      source: "fanbox",
      userAgent: "test-agent",
    })).resolves.toBe(false);
    expect(invoke).toHaveBeenCalledWith("open_standalone_browser", {
      url: "https://www.fanbox.cc/",
      source: "fanbox",
      userAgent: "test-agent",
    });
  });

  it("closes only the window belonging to the requested provider", async () => {
    invoke.mockResolvedValueOnce(true);
    await expect(closeStandaloneBrowser("pixiv")).resolves.toBe(true);
    expect(invoke).toHaveBeenCalledWith("close_standalone_browser", { source: "pixiv" });
  });
});
