// 作品の日付の種類と、その呼び名。

export type DateField = "saved" | "published" | "updated";

export const DATE_FIELD_OPTIONS: { value: DateField; label: string }[] = [
  { value: "saved", label: "保存日" },
  { value: "published", label: "公開日" },
  { value: "updated", label: "更新日" },
];

export function dateFieldLabel(value: DateField): string {
  return DATE_FIELD_OPTIONS.find((option) => option.value === value)?.label ?? "保存日";
}
