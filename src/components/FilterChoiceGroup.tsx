import { UnstyledButton } from "@mantine/core";
import { Icons, IconSize } from "@/lib/icons";

/** Direct choices for a short, mutually exclusive filter. */
export function FilterChoiceGroup<T extends string>({ label, value, options, onChange }: {
  label: string;
  value: T | null;
  options: readonly { value: T; label: string }[];
  onChange: (value: T | null) => void;
}) {
  return <div className="filter-choice-group" role="group" aria-label={label}>
    <div className="filter-choice-group__label">{label}</div>
    <div className="filter-choice-group__options">
      {[{ value: null, label: "すべて" }, ...options].map((option) => <UnstyledButton key={option.value ?? "all"} className="filter-choice" data-selected={value === option.value || undefined} aria-pressed={value === option.value} onClick={() => onChange(option.value === value ? null : option.value)}>{value === option.value && <Icons.confirm size={IconSize.inline} aria-hidden />}{option.label}</UnstyledButton>)}
    </div>
  </div>;
}
