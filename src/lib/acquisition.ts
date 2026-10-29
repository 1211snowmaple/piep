import type { LibraryAcquisitionFilter } from "@/types/library";

export const acquisitionFilters: { value: LibraryAcquisitionFilter; label: string }[] = [
  { value: "attention", label: "要確認（すべて）" },
  { value: "complete", label: "要確認なし" },
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

export function acquisitionStatusMeta(value: string): { label: string; color: string } {
  const kind = parseAcquisitionFilter(value);
  if (!kind || kind === "attention") return { label: "保留", color: "gray" };
  return { label: acquisitionFilterName(kind), color: kind === "complete" ? "green" : kind === "dismissed" ? "gray" : kind === "restricted" || kind === "unavailable" ? "yellow" : "orange" };
}
