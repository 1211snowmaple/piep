import { useState } from "react";
import { Select } from "@mantine/core";
import { useDebouncedValue } from "@mantine/hooks";
import { useQuery } from "@tanstack/react-query";
import { searchSuggest } from "@/services/searchApi";

export interface SeriesSelection { key: string; source: string | null; label: string }

export function SeriesFilterSelect({ value, onChange, runtime }: { value: SeriesSelection | null; onChange: (value: SeriesSelection | null) => void; runtime: boolean }) {
  const [query, setQuery] = useState("");
  const [opened, setOpened] = useState(false);
  const [debounced] = useDebouncedValue(query.trim(), 180);
  const candidates = useQuery({
    queryKey: ["filter-series", debounced],
    queryFn: async () => {
      const result = runtime ? await searchSuggest({ text: debounced, limit: 30 }) : { items: [{ kind: "series", label: "季節の栞", value: "pixiv:12552619", source: "pixiv", sourceKey: "12552619" }] };
      return result.items.filter((item) => item.kind === "series" && item.sourceKey && item.label.includes(runtime ? "" : debounced)).map((item) => ({ key: item.sourceKey!, source: item.source ?? null, label: item.label }));
    },
    enabled: opened && Boolean(debounced), staleTime: 60_000,
  });
  const selections = [...new Map([...(value ? [value] : []), ...(candidates.data ?? [])].map((item) => [JSON.stringify([item.source, item.key]), item])).entries()];
  return <Select label="シリーズを指定" placeholder="シリーズ名で検索" aria-label="シリーズを指定" searchable clearable value={value ? JSON.stringify([value.source, value.key]) : null} searchValue={query} onSearchChange={setQuery} onDropdownOpen={() => setOpened(true)} onDropdownClose={() => setOpened(false)} onChange={(key) => onChange(selections.find(([id]) => id === key)?.[1] ?? null)} data={selections.map(([id, item]) => ({ value: id, label: `${item.label}${item.source ? `（${item.source}）` : ""}` }))} nothingFoundMessage={candidates.isFetching ? "検索中…" : candidates.error ? "候補を取得できません。もう一度入力してください" : debounced ? "該当するシリーズはありません" : "シリーズ名を入力してください"} styles={{ option: { whiteSpace: "normal", overflowWrap: "anywhere" } }} />;
}
