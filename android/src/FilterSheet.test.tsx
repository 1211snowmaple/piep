import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { FilterSheet } from "./FilterSheet";
import { defaultPcSearch, splitSearchTerms } from "./pcApi";

afterEach(cleanup);

it("札で選んだ区切り文字入りのタグは、入力欄を触らなければ一件のまま適用する", () => {
  const onApply = vi.fn();
  render(<FilterSheet criteria={{ ...defaultPcSearch, tag: ["猫、海"], author: ["山田,太郎"] }} onApply={onApply} onClose={() => {}} />);
  fireEvent.click(screen.getByRole("button", { name: "適用" }));
  expect(onApply).toHaveBeenCalledWith(expect.objectContaining({ tag: ["猫、海"], author: ["山田,太郎"] }));
});

it("入力欄に書いた作者とタグは、読点・コンマ・改行で一件ずつに分ける", () => {
  const onApply = vi.fn();
  render(<FilterSheet criteria={{ ...defaultPcSearch, tag: ["猫、海"] }} onApply={onApply} onClose={() => {}} />);
  fireEvent.change(screen.getByLabelText("タグ"), { target: { value: "春、夏, 秋" } });
  fireEvent.click(screen.getByRole("button", { name: "適用" }));
  expect(onApply).toHaveBeenCalledWith(expect.objectContaining({ tag: ["春", "夏", "秋"] }));
});

it("自由入力の分け方", () => {
  expect(splitSearchTerms(" 猫、海,空，雲\n星 ")).toEqual(["猫", "海", "空", "雲", "星"]);
  expect(splitSearchTerms("、 ,")).toEqual([]);
});
