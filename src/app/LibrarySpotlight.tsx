import { useState } from "react";
import { Box, Checkbox, Group, SegmentedControl, Select, Text } from "@mantine/core";
import { useDebouncedValue } from "@mantine/hooks";
import { Spotlight, type SpotlightActionData } from "@mantine/spotlight";
import { useQuery } from "@tanstack/react-query";
import { useAppNavigate } from "@/app/router";
import { Icons, IconSize } from "@/lib/icons";
import { isTauriRuntime, searchDownloadsV2 } from "@/services/dbApi";
import { searchDemoWorks } from "@/mocks/demoData";
import { parseSearchMode, searchModes, useDefaultSearchMode } from "@/lib/searchPreferences";
import type { LibrarySearchMode } from "@/types/library";

/** Library search comes first; commands have their own explicit mode. */
export function LibrarySpotlight({ actions }: { actions: SpotlightActionData[] }) {
  const navigate = useAppNavigate();
  const [query, setQuery] = useState("");
  const [mode, setMode] = useState("library");
  const [opened, setOpened] = useState(false);
  const [composing, setComposing] = useState(false);
  const [includeHistory, setIncludeHistory] = useState(false);
  const [method, setMethod] = useState<LibrarySearchMode | null>(null);
  const [defaultMode] = useDefaultSearchMode();
  const requestedMethod = method ?? defaultMode;
  const searchMode = includeHistory && requestedMethod === "semantic" ? "smart" : requestedMethod;
  const [debounced] = useDebouncedValue(query.trim().slice(0, 500), 200);
  const current = query.trim().slice(0, 500) === debounced;
  const results = useQuery({
    queryKey: ["launcher-library", debounced, searchMode, includeHistory],
    queryFn: () => isTauriRuntime() ? searchDownloadsV2({ text: debounced, limit: 6, searchMode, versionScope: includeHistory ? "all" : "current", projection: "libraryCompact" }) : Promise.resolve(searchDemoWorks(debounced)),
    enabled: opened && mode === "library" && Boolean(debounced) && !composing,
    staleTime: 30_000, gcTime: 60_000,
  });
  const openResults = () => navigate(`/library?q=${encodeURIComponent(query.trim().slice(0, 500))}&intent=${searchMode}${includeHistory ? "&versions=all" : ""}`, { scroll: "top" });
  const matchingActions = actions.filter((action) => `${action.label} ${action.description ?? ""}`.toLocaleLowerCase("ja-JP").includes(query.trim().toLocaleLowerCase("ja-JP")));
  return <Spotlight.Root shortcut={null} attributes={{ content: { "aria-label": "検索と画面移動" } }} query={query} onQueryChange={setQuery} onSpotlightOpen={() => setOpened(true)} onCompositionStartCapture={() => setComposing(true)} onCompositionEndCapture={() => setComposing(false)} onKeyDownCapture={(event) => { if ((composing || event.nativeEvent.isComposing) && event.key === "Enter") event.stopPropagation(); }} onSpotlightClose={() => { setOpened(false); setQuery(""); setComposing(false); setMethod(null); }} scrollable maxHeight="60vh">
    <Box px="md" pt="md"><SegmentedControl fullWidth size="sm" aria-label="検索する場所" value={mode} onChange={setMode} data={[{ value: "library", label: "ライブラリ" }, { value: "commands", label: "画面・操作" }]} /></Box>
    <Spotlight.Search data-autofocus aria-label={mode === "library" ? "ライブラリ全体を検索" : "画面・操作を検索"} placeholder={mode === "library" ? "タイトル、作者、タグ、本文を検索" : "設定、保存、更新などを検索"} leftSection={<Icons.search size={IconSize.nav} />} maxLength={500} />
    {mode === "library" && <Group px="md" pb="sm" justify="space-between"><Select size="xs" w={140} aria-label="全体検索の方法" allowDeselect={false} value={searchMode} onChange={(value) => setMethod(parseSearchMode(value))} data={searchModes.map(({ value, label }) => ({ value, label, disabled: value === "semantic" && includeHistory }))} /><Checkbox size="xs" label="過去版も検索" checked={includeHistory} onChange={(event) => setIncludeHistory(event.currentTarget.checked)} /></Group>}
    <Spotlight.ActionsList>
      {mode === "library" ? query.trim() ? <>
        <Spotlight.Action id="search-results" label="検索結果をすべて表示" leftSection={<Icons.search size={IconSize.nav} />} onClick={openResults} />
        {current && !composing && results.data?.items.slice(0, 6).map((work) => <Spotlight.Action key={work.id} id={`work-${work.id}`} label={work.title} description={work.personName || work.authorName} onClick={() => navigate(`/works/${work.id}`)} leftSection={<Icons.read size={IconSize.nav} />} />)}
        {results.isFetching && <Spotlight.Empty>作品を検索中…</Spotlight.Empty>}
        {results.error && <Spotlight.Empty>候補を取得できません。検索結果から再試行できます。</Spotlight.Empty>}
        {current && results.isSuccess && !results.data.items.length && <Spotlight.Empty>一致する作品はありません。検索結果で条件を調整できます。</Spotlight.Empty>}
      </> : <>
        <Spotlight.Empty>言葉を入力して作品を探す</Spotlight.Empty>
        <Spotlight.ActionsGroup label="保存状態から探す">
          <Spotlight.Action id="attention" label="要確認" description="一部保存・取得エラーなど" leftSection={<Icons.warning size={IconSize.nav} />} onClick={() => navigate("/library?acquisition=attention")} />
          <Spotlight.Action id="revised" label="改稿あり" leftSection={<Icons.versionHistory size={IconSize.nav} />} onClick={() => navigate("/library?revision=revised")} />
        </Spotlight.ActionsGroup>
      </> : matchingActions.length ? [...new Set(matchingActions.map((action) => action.group ?? "画面・操作"))].map((group) => <Spotlight.ActionsGroup key={group} label={group}>{matchingActions.filter((action) => (action.group ?? "画面・操作") === group).map(({ group: _group, ...action }) => <Spotlight.Action {...action} key={action.id} />)}</Spotlight.ActionsGroup>) : <Spotlight.Empty>一致する画面・操作はありません。</Spotlight.Empty>}
    </Spotlight.ActionsList>
    <Spotlight.Footer><Group justify="space-between"><Text size="xs" c="dimmed">Enterで検索・開く</Text><Text size="xs" c="dimmed">↑↓ 選択 · Esc 閉じる</Text></Group></Spotlight.Footer>
  </Spotlight.Root>;
}
