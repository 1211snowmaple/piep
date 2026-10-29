import { useLocalStorage } from "@mantine/hooks";
import type { LibrarySearchMode } from "@/types/library";

export const searchModes = [
  { value: "smart", label: "標準", description: "表記ゆれも含めて、タイトル・作者・タグ・本文を探します。" },
  { value: "exact", label: "入力した語で", description: "言葉を広げず、入力した語を使って探します。" },
  { value: "semantic", label: "意味で", description: "入力した文章に内容が近い作品を探します。現在版が対象です。" },
] as const;

export function parseSearchMode(value: unknown): LibrarySearchMode {
  return value === "exact" || value === "semantic" ? value : "smart";
}

export function useDefaultSearchMode() {
  const [value, setValue] = useLocalStorage<string>({ key: "piep.default-search-mode", defaultValue: "smart", getInitialValueInEffect: false });
  return [parseSearchMode(value), (next: LibrarySearchMode) => setValue(next)] as const;
}
