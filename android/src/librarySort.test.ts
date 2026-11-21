import { beforeEach, expect, it } from "vitest";
import { loadSavedConditions } from "./librarySort";

beforeEach(() => window.localStorage.clear());

it("moves saved conditions stored under the previous key once", () => {
  const saved = [{ id: "c1", name: "雨", criteria: { text: "雨" } }];
  window.localStorage.setItem("piep.android.library.saved-conditions.v1", JSON.stringify(saved));
  expect(loadSavedConditions()).toMatchObject([{ id: "c1", name: "雨", criteria: { text: "雨" } }]);
  expect(window.localStorage.getItem("piep.android.library.saved-conditions.v1")).toBeNull();
  expect(JSON.parse(window.localStorage.getItem("piep.android.library.saved-conditions") ?? "[]")).toEqual(saved);
});

it("keeps conditions already saved under the current key", () => {
  window.localStorage.setItem("piep.android.library.saved-conditions", JSON.stringify([{ id: "new", name: "今", criteria: {} }]));
  window.localStorage.setItem("piep.android.library.saved-conditions.v1", JSON.stringify([{ id: "old", name: "昔", criteria: {} }]));
  expect(loadSavedConditions().map((item) => item.id)).toEqual(["new"]);
  expect(window.localStorage.getItem("piep.android.library.saved-conditions.v1")).toBeNull();
});
