import { QueryClient } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { deleteThenCleanup } from "./deletedWorkCleanup";

describe("deleted work cleanup", () => {
  beforeEach(() => window.localStorage.clear());

  it("removes reading, EPUB, listing, and work-scoped caches after success", async () => {
    const client = new QueryClient();
    const removeFromEpubQueue = vi.fn();
    window.localStorage.setItem("piep.reader-position.101.current", JSON.stringify({ page: 2, top: 40 }));
    for (const key of [
      ["reader-metadata", 101],
      ["reader-content-page", 101, null, 0],
      ["reader-content-search", 101, null, "雨"],
      ["editor-document", 101],
      ["work-assets", 101],
      ["work-json", 101, "work.json"],
      ["library", { text: null }],
    ]) client.setQueryData(key, { stale: true });

    await deleteThenCleanup(() => Promise.resolve("deleted"), { queryClient: client, ids: [101], removeFromEpubQueue });

    expect(window.localStorage.getItem("piep.reader-position.101.current")).toBeNull();
    expect(removeFromEpubQueue).toHaveBeenCalledWith([101]);
    expect(client.getQueryData(["reader-metadata", 101])).toBeUndefined();
    expect(client.getQueryData(["reader-content-page", 101, null, 0])).toBeUndefined();
    expect(client.getQueryData(["editor-document", 101])).toBeUndefined();
    expect(client.getQueryData(["library", { text: null }])).toBeUndefined();
  });

  it("retains every client-side reference when database deletion fails", async () => {
    const client = new QueryClient();
    const removeFromEpubQueue = vi.fn();
    window.localStorage.setItem("piep.reader-position.101.current", JSON.stringify({ page: 2, top: 40 }));
    client.setQueryData(["reader-metadata", 101], { retained: true });
    client.setQueryData(["library", { text: null }], { retained: true });

    await expect(deleteThenCleanup(() => Promise.reject(new Error("database busy")), { queryClient: client, ids: [101], removeFromEpubQueue })).rejects.toThrow("database busy");

    expect(window.localStorage.getItem("piep.reader-position.101.current")).not.toBeNull();
    expect(removeFromEpubQueue).not.toHaveBeenCalled();
    expect(client.getQueryData(["reader-metadata", 101])).toEqual({ retained: true });
    expect(client.getQueryData(["library", { text: null }])).toEqual({ retained: true });
  });
});
