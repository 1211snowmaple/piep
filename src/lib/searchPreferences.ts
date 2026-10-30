import { useLocalStorage } from "@mantine/hooks";
import type { LibrarySearchMode } from "@/types/library";

export const searchModes = [
  { value: "smart", label: "標準検索", description: "表記の違いを補い、タイトル・作者・タグ・本文から検索します。" },
  { value: "exact", label: "キーワード検索", description: "関連語へ展開せず、入力した語を検索します。" },
  { value: "semantic", label: "意味検索", description: "文章の内容が近い作品を検索します。現在の保存版が対象です。" },
] as const;

export function parseSearchMode(value: unknown): LibrarySearchMode {
  return value === "exact" || value === "semantic" ? value : "smart";
}

export function useDefaultSearchMode() {
  const [value, setValue] = useLocalStorage<string>({ key: "piep.default-search-mode", defaultValue: "smart", getInitialValueInEffect: false });
  return [parseSearchMode(value), (next: LibrarySearchMode) => setValue(next)] as const;
}
