import { beforeEach, describe, expect, it } from "vitest";
import { forgetReadingPositions, readReadingPosition, subscribeReadingPositions, writeReadingPosition } from "@/features/library/readingShelf";

function setPosition(id: string, version: string, value: string) {
  window.localStorage.setItem(`piep.reader-position.${id}.${version}`, value);
}

describe("reading shelf membership", () => {
  beforeEach(() => {
    window.localStorage.clear();
    window.sessionStorage.clear();
  });

  it("keeps positions of shelf works under their lower-case id", () => {
    const shelfWork = "3fa85f64-5717-4562-b3fc-2c963f66afa6";
    writeReadingPosition(shelfWork, null, { page: 2, top: 40 });
    expect(readReadingPosition(shelfWork, null)).toMatchObject({ page: 2, top: 40 });
    // 大文字の ID では憶えない。同じ作品の位置が二つに割れる。
    writeReadingPosition(shelfWork.toUpperCase(), null, { page: 3, top: 0 });
    expect(readReadingPosition(shelfWork.toUpperCase(), null)).toBeNull();
    expect(readReadingPosition(shelfWork, null)).toMatchObject({ page: 2, top: 40 });
  });

  it("reads bare numeric values and ignores corrupt entries", () => {
    window.localStorage.setItem("piep.reader-position.11.current", "\"340\"");
    window.localStorage.setItem("piep.reader-position.12.current", "not-a-number");
    expect(readReadingPosition("11", null)).toEqual({ page: 1, top: 340 });
    expect(readReadingPosition("12", null)).toBeNull();
  });

  it("writes one validated object schema and migrates the previous session value", () => {
    writeReadingPosition("21", null, { page: 3, top: 180 });
    expect(JSON.parse(window.localStorage.getItem("piep.reader-position.21.current") ?? "null")).toEqual({ page: 3, top: 180, at: expect.any(Number) });
    expect(readReadingPosition("21", null)).toMatchObject({ page: 3, top: 180 });

    window.sessionStorage.setItem("piep.reader-position.22.4", JSON.stringify({ page: 2, top: 75 }));
    expect(readReadingPosition("22", 4)).toEqual({ page: 2, top: 75 });
    expect(JSON.parse(window.localStorage.getItem("piep.reader-position.22.4") ?? "null")).toEqual({ page: 2, top: 75 });
  });

  it("announces shelf membership changes without emitting on every scroll", () => {
    let changes = 0;
    const unsubscribe = subscribeReadingPositions(() => { changes += 1; });
    writeReadingPosition("31", null, { page: 1, top: 0 });
    expect(changes).toBe(0);
    writeReadingPosition("31", null, { page: 1, top: 10 });
    expect(changes).toBe(1);
    writeReadingPosition("31", null, { page: 1, top: 20 });
    expect(changes).toBe(1);
    forgetReadingPositions("31");
    expect(changes).toBe(2);
    unsubscribe();
  });

  it("forgets a work's positions across every version when it is deleted", () => {
    setPosition("4", "current", "10");
    setPosition("4", "2", "20");
    setPosition("5", "current", "30");
    forgetReadingPositions("4");
    expect(readReadingPosition("4", null)).toBeNull();
    expect(readReadingPosition("4", 2)).toBeNull();
    expect(readReadingPosition("5", null)).toMatchObject({ top: 30 });
    // A work whose id is a prefix of another must not be caught by it.
    setPosition("40", "current", "10");
    forgetReadingPositions("4");
    expect(readReadingPosition("40", null)).toMatchObject({ top: 10 });
  });
});

describe("reading positions", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("stamps the time a position was written so a later read elsewhere can win", () => {
    writeReadingPosition("5", null, { page: 2, top: 30 });
    expect(readReadingPosition("5", null)?.at).toBeGreaterThan(0);
  });
});
