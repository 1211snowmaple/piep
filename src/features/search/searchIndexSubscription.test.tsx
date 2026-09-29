import { renderHook } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ listen: vi.fn() }));

vi.mock("@/services/dbApi", () => ({ isTauriRuntime: () => true }));
vi.mock("@/services/eventBus", () => ({ onTauriEvent: mocks.listen }));

import { useSearchIndexProgress } from "./searchIndexProgress";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it("retries a failed native progress subscription while the screen is open", async () => {
  vi.useFakeTimers();
  vi.spyOn(console, "error").mockImplementation(() => undefined);
  mocks.listen
    .mockRejectedValueOnce(new Error("native listener unavailable"))
    .mockResolvedValueOnce(() => undefined);

  renderHook(() => useSearchIndexProgress());
  expect(mocks.listen).toHaveBeenCalledTimes(1);
  await vi.advanceTimersByTimeAsync(1_000);
  expect(mocks.listen).toHaveBeenCalledTimes(2);
});
