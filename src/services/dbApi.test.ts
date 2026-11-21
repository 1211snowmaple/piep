import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());

vi.mock("@tauri-apps/api/core", () => ({
  invoke,
  convertFileSrc: (path: string) => `asset://${path}`,
}));

import { getAssetUrl } from "@/services/dbApi";

describe("dbApi asset URLs", () => {
  beforeEach(() => invoke.mockReset());





  it("allows demo image URLs only in preview and never auto-fetches them in native", () => {
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
    expect(getAssetUrl("data:image/png;base64,AAAA")).toBe("data:image/png;base64,AAAA");
    expect(getAssetUrl("https://images.example/cover.jpg")).toBe("https://images.example/cover.jpg");
    expect(getAssetUrl("C:\\library\\cover.jpg")).toBeNull();

    Object.defineProperty(window, "__TAURI_INTERNALS__", { value: {}, configurable: true });
    expect(getAssetUrl("https://tracker.example/cover.jpg")).toBeNull();
    expect(getAssetUrl("http://tracker.example/cover.jpg")).toBeNull();
    expect(getAssetUrl("data:image/png;base64,AAAA")).toBeNull();
    expect(getAssetUrl("C:\\library\\cover.jpg")).toBe("asset://C:\\library\\cover.jpg");
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
  });
});
