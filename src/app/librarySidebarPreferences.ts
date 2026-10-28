import { useLocalStorage } from "@mantine/hooks";

export const optionalLibraryShelves = [
  { value: "favorite", label: "お気に入り" },
  { value: "reading", label: "読みかけ" },
  { value: "revised", label: "改稿あり" },
  { value: "attention", label: "要確認" },
] as const;
type OptionalShelf = typeof optionalLibraryShelves[number]["value"];
type Visibility = Record<OptionalShelf, boolean>;
const defaults: Visibility = { favorite: true, reading: true, revised: true, attention: true };

export function normalizeLibrarySidebarVisibility(value: unknown): Visibility {
  const input = value && typeof value === "object" ? value as Record<string, unknown> : {};
  return Object.fromEntries(optionalLibraryShelves.map(({ value: key }) => [key, typeof input[key] === "boolean" ? input[key] : defaults[key]])) as Visibility;
}

export function useLibrarySidebarVisibility() {
  const [stored, setStored] = useLocalStorage<Visibility>({ key: "piep.library-sidebar.v1", defaultValue: defaults });
  return [normalizeLibrarySidebarVisibility(stored), (key: OptionalShelf, shown: boolean) => setStored((previous) => ({ ...normalizeLibrarySidebarVisibility(previous), [key]: shown }))] as const;
}
