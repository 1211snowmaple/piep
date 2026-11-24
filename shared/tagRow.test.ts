import { expect, it } from "vitest";
import { visibleTagCount } from "./tagRow";

it("タグと間隔を足し、残りがある間だけ残件数の幅を空ける", () => {
  const widths = [30, 30, 30];
  const overflowWidth = () => 20;
  expect(visibleTagCount({ widths, available: 98, gap: 4, overflowWidth })).toBe(3);
  expect(visibleTagCount({ widths, available: 88, gap: 4, overflowWidth })).toBe(2);
  expect(visibleTagCount({ widths, available: 87, gap: 4, overflowWidth })).toBe(1);
});

it("省略表示では残件数に応じた幅を使い、最後の一件は残件数に含める", () => {
  const widths = [30, 30, 30];
  const overflowWidth = (hidden: number) => hidden === 2 ? 25 : 20;
  expect(visibleTagCount({ widths, available: 88, gap: 4, limit: 2, overflowWidth })).toBe(2);
  expect(visibleTagCount({ widths, available: 200, gap: 4, limit: 2, overflowWidth })).toBe(2);
});

it("先頭が長くても一件は残し、タグがなければ空にする", () => {
  expect(visibleTagCount({ widths: [120, 30], available: 20, gap: 4 })).toBe(1);
  expect(visibleTagCount({ widths: [], available: 20, gap: 4 })).toBe(0);
});
