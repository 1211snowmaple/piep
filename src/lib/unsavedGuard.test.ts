import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { hasUnsavedWork, installCloseGuard, registerUnsavedGuard } from "./unsavedGuard";

const nativeWindow = vi.hoisted(() => ({
  onCloseRequested: vi.fn(), destroy: vi.fn(), close: vi.fn(),
}));
vi.mock("@/services/dbApi", () => ({ isTauriRuntime: () => true }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => nativeWindow }));

let unregister: () => void;
let requestClose: (event: { preventDefault: () => void }) => Promise<void>;

beforeEach(() => {
  nativeWindow.destroy.mockReset().mockResolvedValue(undefined);
  nativeWindow.close.mockReset().mockResolvedValue(undefined);
  nativeWindow.onCloseRequested.mockReset().mockImplementation((handler) => {
    requestClose = handler;
    return Promise.resolve(vi.fn());
  });
  unregister = registerUnsavedGuard(() => true);
});
afterEach(() => unregister());

describe("native close confirmation", () => {
  it("shows one confirmation for repeated close requests and allows another after cancelling", async () => {
    let answer!: (discard: boolean) => void;
    const confirm = vi.fn(() => new Promise<boolean>((resolve) => { answer = resolve; }));
    await installCloseGuard(confirm);
    const first = { preventDefault: vi.fn() };
    const second = { preventDefault: vi.fn() };
    const pending = requestClose(first);
    void requestClose(second);
    expect(first.preventDefault).toHaveBeenCalledOnce();
    expect(second.preventDefault).toHaveBeenCalledOnce();
    expect(confirm).toHaveBeenCalledOnce();

    answer(false);
    await pending;
    expect(nativeWindow.destroy).not.toHaveBeenCalled();
    const retry = requestClose({ preventDefault: vi.fn() });
    expect(confirm).toHaveBeenCalledTimes(2);
    answer(true);
    await retry;
    expect(nativeWindow.destroy).toHaveBeenCalledOnce();
  });

  it("keeps protecting edits if both native close methods fail and allows retrying", async () => {
    nativeWindow.destroy.mockRejectedValueOnce(new Error("destroy failed"));
    nativeWindow.close.mockRejectedValueOnce(new Error("close failed"));
    const confirm = vi.fn().mockResolvedValue(true);
    await installCloseGuard(confirm);
    await requestClose({ preventDefault: vi.fn() });

    expect(hasUnsavedWork("navigate")).toBe(true);
    expect(hasUnsavedWork("close")).toBe(true);
    await requestClose({ preventDefault: vi.fn() });
    expect(confirm).toHaveBeenCalledTimes(2);
    expect(nativeWindow.destroy).toHaveBeenCalledTimes(2);
  });
});
