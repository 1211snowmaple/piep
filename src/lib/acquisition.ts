import type { LibraryAcquisitionFilter } from "@/types/library";

// 並びは「要確認かどうか」→「要確認の内訳」→「非表示」。三つのまとまりは重ならない。
// 非表示は見たうえで外した作品なので、要確認にもその内訳にも、要確認なしにも入らない。
export const acquisitionFilters: { value: LibraryAcquisitionFilter; label: string; startsGroup?: boolean }[] = [
  { value: "complete", label: "要確認なし" },
  { value: "attention", label: "要確認" },
  { value: "incomplete", label: "一部保存", startsGroup: true },
  { value: "unavailable", label: "見つからない" },
  { value: "restricted", label: "閲覧条件待ち" },
  { value: "error", label: "取得エラー" },
  { value: "dismissed", label: "非表示", startsGroup: true },
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

const metadataLabels = { title: "題名", caption: "説明", tags: "タグ" } as const;

/** One sentence for a partial save: what is missing, without the raw hold reason. */
export function incompleteSummary(missingImages: boolean, missingMetadata: readonly (keyof typeof metadataLabels)[]): string {
  const details = missingMetadata.length ? `作品情報（${missingMetadata.map((field) => metadataLabels[field]).join("・")}）` : "";
  if (missingImages && details) return `作品は保存済みです。一部の挿絵と${details}を取得できませんでした。`;
  if (details) return `作品は保存済みです。${details}が取得元で伏せられた状態で保存されています。再確認すると補完します。`;
  return missingImages ? "作品は保存済みです。一部の挿絵を取得できませんでした。" : "作品は保存済みです。一部の内容を取得できませんでした。";
}
