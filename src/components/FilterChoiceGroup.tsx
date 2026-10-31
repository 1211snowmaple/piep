import { UnstyledButton } from "@mantine/core";
import { Icons, IconSize } from "@/lib/icons";

export interface FilterChoiceOption<T extends string> {
  value: T;
  label: string;
  /** ここから別のまとまり。前に細い区切りを置く（「要確認」と、その内訳など）。 */
  startsGroup?: boolean;
}

/**
 * 短い択一の絞り込み。押すだけで選べ、選んでいるものが開かずに見える。
 *
 * `allLabel` は「指定しない」の呼び名。`null` にすると「指定しない」が無くなり、
 * いつもどれか一つが選ばれている（日付の種類のように、外すことのない選択）。
 */
export function FilterChoiceGroup<T extends string>({ label, value, options, onChange, allLabel = "すべて" }: {
  label: string;
  value: T | null;
  options: readonly FilterChoiceOption<T>[];
  onChange: (value: T | null) => void;
  allLabel?: string | null;
}) {
  const choices: { value: T | null; label: string; startsGroup?: boolean }[] = allLabel === null ? [...options] : [{ value: null, label: allLabel }, ...options];
  const choose = (next: T | null) => {
    if (allLabel === null) { if (next !== null && next !== value) onChange(next); return; }
    onChange(next === value ? null : next);
  };
  return <div className="filter-choice-group" role="group" aria-label={label}>
    <div className="filter-choice-group__label">{label}</div>
    <div className="filter-choice-group__options">
      {/* 「指定しない」の鍵は値と重ならない形にする。値そのものが "all" の項目がある（検索する版）。 */}
      {choices.map((option) => <span key={option.value === null ? "unset:" : `value:${option.value}`} className="filter-choice__slot" data-starts-group={option.startsGroup || undefined}>
        <UnstyledButton className="filter-choice" data-selected={value === option.value || undefined} aria-pressed={value === option.value} onClick={() => choose(option.value)}>{value === option.value && <Icons.confirm size={IconSize.inline} aria-hidden />}{option.label}</UnstyledButton>
      </span>)}
    </div>
  </div>;
}

/**
 * いくつでも選べる短い絞り込み。何も選ばなければ「すべて」。
 *
 * 見た目と押し方は択一のものと同じ。選べる数だけが違う（保存元のように、
 * 二つ以上を合わせて見たいことがある項目）。
 */
export function FilterMultiChoiceGroup<T extends string>({ label, values, options, onChange, allLabel = "すべて" }: {
  label: string;
  values: readonly T[];
  options: readonly FilterChoiceOption<T>[];
  onChange: (values: T[]) => void;
  allLabel?: string;
}) {
  const toggle = (value: T) => onChange(values.includes(value) ? values.filter((item) => item !== value) : [...values, value]);
  return <div className="filter-choice-group" role="group" aria-label={label}>
    <div className="filter-choice-group__label">{label}</div>
    <div className="filter-choice-group__options">
      <span className="filter-choice__slot">
        <UnstyledButton className="filter-choice" data-selected={values.length === 0 || undefined} aria-pressed={values.length === 0} onClick={() => onChange([])}>{values.length === 0 && <Icons.confirm size={IconSize.inline} aria-hidden />}{allLabel}</UnstyledButton>
      </span>
      {options.map((option) => {
        const selected = values.includes(option.value);
        return <span key={option.value} className="filter-choice__slot" data-starts-group={option.startsGroup || undefined}>
          <UnstyledButton className="filter-choice" data-selected={selected || undefined} aria-pressed={selected} onClick={() => toggle(option.value)}>{selected && <Icons.confirm size={IconSize.inline} aria-hidden />}{option.label}</UnstyledButton>
        </span>;
      })}
    </div>
  </div>;
}
