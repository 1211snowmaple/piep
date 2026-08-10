import { describe, expect, it } from "vitest";
import { isSavableCandidateStatus, normalizeConcurrency, parseWorkId } from "@/features/updates/UpdatesPage";

describe("update candidate selection", () => {
  it("only allows new and failed candidates to be selected for saving", () => {
    expect(isSavableCandidateStatus("candidate")).toBe(true);
    expect(isSavableCandidateStatus("failed")).toBe(true);
    for (const status of ["queued", "running", "saved", "skipped", "done"]) {
      expect(isSavableCandidateStatus(status)).toBe(false);
    }
  });
});

describe("update concurrency normalization", () => {
  it("clamps values to the command's integer range", () => {
    expect(normalizeConcurrency(3.9, 1, 8)).toBe(3);
    expect(normalizeConcurrency(99, 1, 8)).toBe(8);
    expect(normalizeConcurrency("", 1, 4)).toBe(1);
    expect(normalizeConcurrency(Number.NaN, 1, 3)).toBe(1);
  });
});

describe("work-scoped update URLs", () => {
  it("only accepts positive safe integer work IDs", () => {
    expect(parseWorkId("42")).toBe(42);
    expect(parseWorkId("abc")).toBeNull();
    expect(parseWorkId("1e3")).toBeNull();
    expect(parseWorkId("0")).toBeNull();
    expect(parseWorkId("99999999999999999999")).toBeNull();
  });
});
