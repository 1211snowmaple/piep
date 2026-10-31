import { afterEach, describe, expect, it, vi } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { createTextDiff } from "./textDiff";
import { useTextDiff } from "./useTextDiff";

class FakeWorker {
  static instances: FakeWorker[] = [];
  onmessage: ((event: MessageEvent) => void) | null = null;
  onerror: (() => void) | null = null;
  terminated = false;
  constructor() {
    FakeWorker.instances.push(this);
  }
  postMessage({ before, after }: { before: string; after: string }) {
    setTimeout(() => this.terminated || this.onmessage?.({ data: createTextDiff(before, after) } as MessageEvent), 0);
  }
  terminate() {
    this.terminated = true;
  }
}

afterEach(() => {
  vi.unstubAllGlobals();
  FakeWorker.instances = [];
});

describe("useTextDiff", () => {
  it("diffs a short revision at once without a worker", () => {
    vi.stubGlobal("Worker", FakeWorker);
    const { result } = renderHook(() => useTextDiff("古い言葉", "新しい言葉"));
    expect(result.current.pending).toBe(false);
    expect(result.current.diff?.changed).toBe(true);
    expect(FakeWorker.instances).toHaveLength(0);
  });

  it("diffs a long revision in a worker and cancels it when the versions change", async () => {
    vi.stubGlobal("Worker", FakeWorker);
    const long = "雨上がりの図書室で、彼女は振り返った。\n".repeat(1_000);
    const { result, rerender } = renderHook(({ after }) => useTextDiff(long, after), { initialProps: { after: `${long}追記` } });
    expect(result.current).toEqual({ diff: null, pending: true });

    rerender({ after: `${long}別の追記` });
    expect(FakeWorker.instances[0].terminated).toBe(true);
    await waitFor(() => expect(result.current.pending).toBe(false));
    expect(result.current.diff?.addedCharacters).toBe(4);
  });

  it("shows nothing to compare for the first version", () => {
    const { result } = renderHook(() => useTextDiff(null, "本文"));
    expect(result.current).toEqual({ diff: null, pending: false });
  });
});
