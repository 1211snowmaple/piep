import { beforeEach, describe, expect, it, vi } from "vitest";

const searchMock = vi.fn();
const readingMock = vi.fn(() => [11, 12]);

vi.mock("@/services/dbApi", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/services/dbApi")>()),
  searchDownloadsV2: (params: unknown) => searchMock(params),
}));
vi.mock("@/features/library/readingShelf", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/features/library/readingShelf")>()),
  readingWorkIds: () => readingMock(),
}));

const { isSavableCandidateStatus, parseWorkId, shelfWorkIds } = await import(
  "@/features/updates/UpdatesPage"
);

beforeEach(() => {
  searchMock.mockReset();
  readingMock.mockReset();
  readingMock.mockReturnValue([11, 12]);
});

describe("update candidate selection", () => {
  it("only allows new and failed candidates to be selected for saving", () => {
    expect(isSavableCandidateStatus("candidate")).toBe(true);
    expect(isSavableCandidateStatus("failed")).toBe(true);
    for (const status of ["queued", "running", "saved", "skipped", "done"]) {
      expect(isSavableCandidateStatus(status)).toBe(false);
    }
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

describe("shelf-scoped update checks", () => {
  it("leaves the ordinary scopes alone", async () => {
    for (const scope of ["all", "work", "author", "series"]) {
      await expect(shelfWorkIds(scope)).resolves.toBeNull();
    }
  });

  it("collects the works behind a shelf so the job can check them as works", async () => {
    searchMock.mockResolvedValueOnce({ items: [{ id: 3 }, { id: 7 }] });
    await expect(shelfWorkIds("favorite")).resolves.toEqual([3, 7]);
    expect(searchMock).toHaveBeenCalledWith(expect.objectContaining({ favorite: true }));
  });

  // 読みかけが空なら、取得しに行かずに「空の棚」と答える。
  it("answers with an empty shelf instead of querying for nothing", async () => {
    readingMock.mockReturnValueOnce([]);
    await expect(shelfWorkIds("reading")).resolves.toEqual([]);
    expect(searchMock).not.toHaveBeenCalled();
  });
});
