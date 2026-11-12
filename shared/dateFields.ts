// 作品の日付の種類と、その呼び名。

export type DateField = "downloaded_at" | "source_created_at" | "source_updated_at";

export const DATE_FIELD_OPTIONS: { value: DateField; label: string }[] = [
  { value: "downloaded_at", label: "保存日" },
  { value: "source_created_at", label: "公開日" },
  { value: "source_updated_at", label: "更新日" },
];

export function dateFieldLabel(value: DateField): string {
  return DATE_FIELD_OPTIONS.find((option) => option.value === value)?.label ?? "保存日";
}
