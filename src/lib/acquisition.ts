import type { LibraryAcquisitionFilter } from "@/types/library";

export const acquisitionFilters: { value: LibraryAcquisitionFilter; label: string }[] = [
  { value: "attention", label: "要確認" },
  { value: "incomplete", label: "一部保存" },
  { value: "unavailable", label: "見つからない" },
  { value: "restricted", label: "閲覧条件待ち" },
  { value: "error", label: "取得エラー" },
  { value: "dismissed", label: "非表示" },
];

export function parseAcquisitionFilter(value: unknown): LibraryAcquisitionFilter | null {
  return acquisitionFilters.find((filter) => filter.value === value)?.value ?? null;
}

export function acquisitionFilterName(value: LibraryAcquisitionFilter): string {
  return acquisitionFilters.find((filter) => filter.value === value)?.label ?? "要確認";
}
