import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { entityPage, homePage, useAppHistory } from "./appHistory";

const KEY = "piep.place";
type LaunchWindow = Window & { piepAndroidLaunch?: { restoredBySystem: () => boolean } };

function launch(restoredBySystem: boolean) {
  (window as LaunchWindow).piepAndroidLaunch = { restoredBySystem: () => restoredBySystem };
}

function render() {
  const scroller = { current: document.createElement("div") };
  return renderHook(() => useAppHistory(scroller));
}

const author = entityPage("person", "pixiv", "8001234", "青葉しおり");

beforeEach(() => window.localStorage.clear());
afterEach(() => {
  cleanup();
  delete (window as LaunchWindow).piepAndroidLaunch;
});

describe("見ていた場所の書き留めと戻し", () => {
  it("OS に終わらされたあとの起動では、見ていた場所と戻る先から始める", () => {
    launch(false);
    const first = render();
    act(() => first.result.current.push({ page: author, detail: null, reader: null }));
    first.unmount();

    launch(true);
    const next = render();
    expect(next.result.current.entry.page).toEqual(author);
    expect(next.result.current.depth).toBe(2);
    act(() => { next.result.current.back(); });
    expect(next.result.current.entry.page).toEqual(homePage);
  });

  it("利用者が閉じたあとの起動では、最初の画面から始め、書き留めた場所を捨てる", () => {
    launch(false);
    const first = render();
    act(() => first.result.current.push({ page: author, detail: null, reader: null }));
    first.unmount();

    const next = render();
    expect(next.result.current.entry.page).toEqual(homePage);
    expect(next.result.current.depth).toBe(1);
    next.unmount();
    launch(true);
    const after = render();
    expect(after.result.current.entry.page).toEqual(homePage);
  });

  it("壊れた値が残っていたら、最初の画面から始める", () => {
    window.localStorage.setItem(KEY, JSON.stringify({ stack: [{ id: "x", page: { name: "unknown" } }], positions: [] }));
    launch(true);
    expect(render().result.current.entry.page).toEqual(homePage);
    window.localStorage.setItem(KEY, "{");
    cleanup();
    expect(render().result.current.entry.page).toEqual(homePage);
  });

  it("戻したあとに開いた場所は、前の番号と重ならない", () => {
    launch(false);
    const first = render();
    act(() => first.result.current.push({ page: author, detail: null, reader: null }));
    const authorId = first.result.current.entry.id;
    first.unmount();

    launch(true);
    const next = render();
    act(() => next.result.current.push({ page: { name: "settings" }, detail: null, reader: null }));
    expect(next.result.current.entry.id).toBeGreaterThan(authorId);
  });
});
