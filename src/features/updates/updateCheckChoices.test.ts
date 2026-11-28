import { expect, it } from "vitest";
import { parseUpdateCheckChoices, updateCheckChoiceDefaults } from "./updateCheckChoices";

it("壊れた値や知らない値は既定に戻す", () => {
  expect(parseUpdateCheckChoices(null)).toEqual(updateCheckChoiceDefaults);
  expect(parseUpdateCheckChoices("auto_save")).toEqual(updateCheckChoiceDefaults);
  expect(parseUpdateCheckChoices({ scope: "everything", mode: "save", depth: 3 })).toEqual(updateCheckChoiceDefaults);
});

it("選べる値はそのまま読む", () => {
  expect(parseUpdateCheckChoices({ scope: "group:abc", mode: "auto_save", depth: "full" }))
    .toEqual({ scope: "group:abc", mode: "auto_save", depth: "full" });
});
