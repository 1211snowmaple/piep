import { beforeEach, describe, expect, it, vi } from "vitest";

const event = vi.hoisted(() => ({ listen: vi.fn() }));

vi.mock("@tauri-apps/api/event", () => ({ listen: event.listen }));

import { subscribeTauriEvent } from "@/services/eventBus";

describe("subscribeTauriEvent", () => {
  beforeEach(() => event.listen.mockReset());

  it("unlistens when registration resolves after cleanup", async () => {
    let resolveRegistration!: (unlisten: () => void) => void;
    const unlisten: () => void = vi.fn();
    event.listen.mockReturnValue(new Promise<() => void>((resolve) => {
      resolveRegistration = resolve;
    }));

    const dispose = subscribeTauriEvent("late-event", vi.fn());
    dispose();
    resolveRegistration(unlisten);
    await Promise.resolve();

    expect(unlisten).toHaveBeenCalledOnce();
  });
});
