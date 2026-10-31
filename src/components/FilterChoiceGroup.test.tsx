import { MantineProvider } from "@mantine/core";
import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { FilterChoiceGroup, FilterMultiChoiceGroup } from "@/components/FilterChoiceGroup";

const options = [{ value: "a", label: "甲" }, { value: "b", label: "乙", startsGroup: true }] as const;

describe("FilterChoiceGroup", () => {
  it("chooses, and pressing the chosen one again clears it", () => {
    const onChange = vi.fn();
    const { rerender } = render(<MantineProvider env="test"><FilterChoiceGroup label="種類" value={null} options={options} onChange={onChange} /></MantineProvider>);
    expect(screen.getByRole("button", { name: "すべて" })).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(screen.getByRole("button", { name: "乙" }));
    expect(onChange).toHaveBeenLastCalledWith("b");
    rerender(<MantineProvider env="test"><FilterChoiceGroup label="種類" value="b" options={options} onChange={onChange} /></MantineProvider>);
    fireEvent.click(screen.getByRole("button", { name: /乙/ }));
    expect(onChange).toHaveBeenLastCalledWith(null);
    // 区切りは、まとまりの先頭にだけ付く。
    expect(screen.getByRole("button", { name: /乙/ }).parentElement).toHaveAttribute("data-starts-group");
  });

  it("always keeps one choice when there is no 'all'", () => {
    const onChange = vi.fn();
    render(<MantineProvider env="test"><FilterChoiceGroup label="日付の種類" allLabel={null} value="a" options={options} onChange={onChange} /></MantineProvider>);
    expect(screen.queryByRole("button", { name: "すべて" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /甲/ }));
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "乙" }));
    expect(onChange).toHaveBeenLastCalledWith("b");
  });

  it("names the unfiltered choice as told", () => {
    render(<MantineProvider env="test"><FilterChoiceGroup label="検索する版" allLabel="現在版だけ" value={null} options={[{ value: "all", label: "過去版も含む" }]} onChange={() => undefined} /></MantineProvider>);
    expect(screen.getByRole("button", { name: "現在版だけ" })).toHaveAttribute("aria-pressed", "true");
  });

  it("lets several be chosen together, and 'all' clears them", () => {
    const onChange = vi.fn();
    const { rerender } = render(<MantineProvider env="test"><FilterMultiChoiceGroup label="保存元" values={["a"]} options={options} onChange={onChange} /></MantineProvider>);
    fireEvent.click(screen.getByRole("button", { name: "乙" }));
    expect(onChange).toHaveBeenLastCalledWith(["a", "b"]);
    rerender(<MantineProvider env="test"><FilterMultiChoiceGroup label="保存元" values={["a", "b"]} options={options} onChange={onChange} /></MantineProvider>);
    expect(screen.getByRole("button", { name: /甲/ })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: /乙/ })).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(screen.getByRole("button", { name: /甲/ }));
    expect(onChange).toHaveBeenLastCalledWith(["b"]);
    fireEvent.click(screen.getByRole("button", { name: "すべて" }));
    expect(onChange).toHaveBeenLastCalledWith([]);
  });
});
