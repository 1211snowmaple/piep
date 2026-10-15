import { beforeEach, describe, expect, it } from "vitest";
import { forgetReadingPositions, readReadingPosition, readingWorkIds, subscribeReadingPositions, writeReadingPosition } from "@/features/library/readingShelf";

function setPosition(id: number, version: string, value: string) {
  window.localStorage.setItem(`piep.reader-position.${id}.${version}`, value);
}

describe("reading shelf membership", () => {
  beforeEach(() => {
    window.localStorage.clear();
    window.sessionStorage.clear();
  });

  it("is empty when nothing has been read", () => {
    expect(readingWorkIds()).toEqual([]);
  });

  it("collects each work once, however many versions were read", () => {
    setPosition(7, "current", "820");
    setPosition(7, "3", "120");
    setPosition(2, "current", "40");
    expect(readingWorkIds()).toEqual([2, 7]);
  });

  it("does not count a work that was only opened at the top", () => {
    // Opening a work writes position 0 before a single line is read; treating
    // that as "part-read" would put the whole library on the shelf.
    setPosition(5, "current", "0");
    setPosition(6, "current", "1");
    expect(readingWorkIds()).toEqual([6]);
  });

  it("ignores keys that are not reading positions", () => {
    window.localStorage.setItem("piep.library-view", "\"gallery\"");
    window.localStorage.setItem("piep.reader-settings.v4", "{}");
    window.localStorage.setItem("piep.reader-position.notanumber.current", "50");
    window.localStorage.setItem("piep.reader-position.-3.current", "50");
    setPosition(9, "current", "50");
    expect(readingWorkIds()).toEqual([9]);
  });

  it("reads legacy numeric values and ignores corrupt entries", () => {
    window.localStorage.setItem("piep.reader-position.11.current", "\"340\"");
    window.localStorage.setItem("piep.reader-position.12.current", "not-a-number");
    expect(readReadingPosition(11, null)).toEqual({ page: 1, top: 340 });
    expect(readReadingPosition(12, null)).toBeNull();
    expect(readingWorkIds()).toEqual([11]);
  });

  it("writes one validated object schema and migrates the previous session value", () => {
    writeReadingPosition(21, null, { page: 3, top: 180 });
    expect(JSON.parse(window.localStorage.getItem("piep.reader-position.21.current") ?? "null")).toEqual({ page: 3, top: 180 });
    expect(readReadingPosition(21, null)).toEqual({ page: 3, top: 180 });

    window.sessionStorage.setItem("piep.reader-position.22.4", JSON.stringify({ page: 2, top: 75 }));
    expect(readReadingPosition(22, 4)).toEqual({ page: 2, top: 75 });
    expect(JSON.parse(window.localStorage.getItem("piep.reader-position.22.4") ?? "null")).toEqual({ page: 2, top: 75 });
  });

  it("announces shelf membership changes without emitting on every scroll", () => {
    let changes = 0;
    const unsubscribe = subscribeReadingPositions(() => { changes += 1; });
    writeReadingPosition(31, null, { page: 1, top: 0 });
    expect(changes).toBe(0);
    writeReadingPosition(31, null, { page: 1, top: 10 });
    expect(changes).toBe(1);
    writeReadingPosition(31, null, { page: 1, top: 20 });
    expect(changes).toBe(1);
    forgetReadingPositions(31);
    expect(changes).toBe(2);
    unsubscribe();
  });

  it("forgets a work's positions across every version when it is deleted", () => {
    setPosition(4, "current", "10");
    setPosition(4, "2", "20");
    setPosition(5, "current", "30");
    forgetReadingPositions(4);
    expect(readingWorkIds()).toEqual([5]);
    // A work whose id is a prefix of another must not be caught by it.
    setPosition(40, "current", "10");
    forgetReadingPositions(4);
    expect(readingWorkIds()).toEqual([5, 40]);
  });
});
