import { Pill } from "@mantine/core";

export type FilterTokenTone = "include" | "exclude";

/**
 * A selected filter has one visual language everywhere it appears.
 *
 * The filter drawer used to render Mantine's neutral Pill while the applied
 * condition row rendered a blue Badge. They represented the same state but
 * looked unrelated. Keeping both uses on Pill also preserves the compact
 * remove-button behaviour inside PillsInput.
 */
export function FilterToken({
  label,
  onRemove,
  tone = "include",
}: {
  label: string;
  onRemove: () => void;
  tone?: FilterTokenTone;
}) {
  return (
    <Pill
      className="filter-token"
      data-tone={tone}
      withRemoveButton
      onRemove={onRemove}
      removeButtonProps={{ "aria-label": `${label}を解除` }}
    >
      {label}
    </Pill>
  );
}
