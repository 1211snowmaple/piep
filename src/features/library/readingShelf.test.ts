import { beforeEach, describe, expect, it } from "vitest";
import { forgetReadingPositions, readingWorkIds } from "@/features/library/readingShelf";

function setPosition(id: number, version: string, value: string) {
  window.localStorage.setItem(`piep.reader-position.${id}.${version}`, value);
}

describe("reading shelf membership", () => {
  beforeEach(() => window.localStorage.clear());

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

  it("survives values written by an older build", () => {
    window.localStorage.setItem("piep.reader-position.11.current", "\"340\"");
    window.localStorage.setItem("piep.reader-position.12.current", "not-a-number");
    // A quoted number is still a position; something unparseable is kept rather
    // than guessed away, since the work was opened at least once.
    expect(readingWorkIds()).toEqual([11, 12]);
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
