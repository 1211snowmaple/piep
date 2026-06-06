import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { SearchIcon } from "../icons/Icons";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { searchSuggest } from "@/services/searchApi";
import { store } from "@/store";
import type { SearchSuggestion } from "@/types/library";
import { cn } from "@/lib/utils";

interface SearchIndexStatus {
  totalDownloads: number;
  pendingDownloads: number;
  isComplete: boolean;
  phase?: string;
  indexedChunks?: number;
  semanticIndexedChunks?: number;
  semanticModelReady?: boolean;
  embeddingProvider?: string;
  gpuEnabled?: boolean;
  throughputPerSec?: number | null;
}

interface LibrarySearchBoxProps {
  value: string;
  onChange: (value: string) => void;
  searchMode: "smart" | "exact" | "semantic";
  onSearchModeChange: (value: "smart" | "exact" | "semantic") => void;
  searchIndexStatus: SearchIndexStatus | null;
  tauriAvailable: boolean;
}

const RECENT_KEY = "library.recentSearches";
const SAVED_KEY = "library.savedSearches";
const syntaxHints = ["tag:", "author:", "series:", "source:pixiv", "source:fanbox", "id:", "url:", "-term", "\"phrase\""];

function quoteToken(value: string): string {
  return `"${value.replace(/"/g, "\\\"")}"`;
}

function suggestionToken(item: SearchSuggestion): string {
  if (item.kind === "tag") return `tag:${quoteToken(item.label)}`;
  if (item.kind === "author") return `author:${quoteToken(item.label)}`;
  if (item.kind === "series") return `series:${item.value || quoteToken(item.label)}`;
  return quoteToken(item.label);
}

function appendToken(current: string, token: string): string {
  const trimmed = current.trim();
  return trimmed ? `${trimmed} ${token}` : token;
}

function uniqueLimited(values: string[], limit: number): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const value of values.map(v => v.trim()).filter(Boolean)) {
    if (seen.has(value)) continue;
    seen.add(value);
    out.push(value);
    if (out.length >= limit) break;
  }
  return out;
}

export function LibrarySearchBox({
  value,
  onChange,
  searchMode,
  onSearchModeChange,
  searchIndexStatus,
  tauriAvailable,
}: LibrarySearchBoxProps) {
  const inputRef = useRef<HTMLInputElement>(null);
  const requestSeqRef = useRef(0);
  const [open, setOpen] = useState(false);
  const [suggestions, setSuggestions] = useState<SearchSuggestion[]>([]);
  const [recentSearches, setRecentSearches] = useState<string[]>([]);
  const [savedSearches, setSavedSearches] = useState<string[]>([]);
  const [isComposing, setIsComposing] = useState(false);

  useEffect(() => {
    store.get<string[]>(RECENT_KEY).then(values => setRecentSearches(Array.isArray(values) ? values : [])).catch(() => undefined);
    store.get<string[]>(SAVED_KEY).then(values => setSavedSearches(Array.isArray(values) ? values : [])).catch(() => undefined);
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setOpen(true);
        window.setTimeout(() => inputRef.current?.focus(), 0);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  useEffect(() => {
    if (!open || !tauriAvailable || !value.trim() || isComposing) {
      setSuggestions([]);
      return;
    }
    let cancelled = false;
    const seq = ++requestSeqRef.current;
    const handle = window.setTimeout(async () => {
      try {
        const result = await searchSuggest({ text: value.trim(), limit: 8 });
        if (!cancelled && seq === requestSeqRef.current) setSuggestions(result.items);
      } catch {
        if (!cancelled && seq === requestSeqRef.current) setSuggestions([]);
      }
    }, 140);
    return () => {
      cancelled = true;
      window.clearTimeout(handle);
    };
  }, [isComposing, open, tauriAvailable, value]);

  const rememberRecent = useCallback(async (query: string) => {
    const next = uniqueLimited([query, ...recentSearches], 8);
    setRecentSearches(next);
    await store.set(RECENT_KEY, next).catch(() => undefined);
  }, [recentSearches]);

  const saveCurrent = useCallback(async () => {
    if (!value.trim()) return;
    const next = uniqueLimited([value.trim(), ...savedSearches], 12);
    setSavedSearches(next);
    await store.set(SAVED_KEY, next).catch(() => undefined);
  }, [savedSearches, value]);

  const applyQuery = useCallback((query: string) => {
    onChange(query);
    rememberRecent(query).catch(() => undefined);
    setOpen(false);
  }, [onChange, rememberRecent]);

  const applyToken = useCallback((token: string) => {
    const next = appendToken(value, token);
    onChange(next);
    rememberRecent(next).catch(() => undefined);
    setOpen(false);
  }, [onChange, rememberRecent, value]);

  const groupedSuggestions = useMemo(() => {
    return suggestions.reduce<Record<string, SearchSuggestion[]>>((acc, item) => {
      const key = item.kind || "other";
      acc[key] = acc[key] ?? [];
      acc[key].push(item);
      return acc;
    }, {});
  }, [suggestions]);

  const indexProgress = searchIndexStatus && !searchIndexStatus.isComplete
    ? `${Math.max(0, searchIndexStatus.totalDownloads - searchIndexStatus.pendingDownloads).toLocaleString()}/${searchIndexStatus.totalDownloads.toLocaleString()}`
    : null;
  const semanticStatus = searchIndexStatus && searchIndexStatus.isComplete
    ? searchIndexStatus.semanticModelReady
      ? `意味検索 ${searchIndexStatus.gpuEnabled ? "GPU" : "CPU"} ${Math.max(0, searchIndexStatus.semanticIndexedChunks ?? searchIndexStatus.indexedChunks ?? 0).toLocaleString()} chunks`
      : "意味検索モデル未準備"
    : null;

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <div className={cn("search-input-wrapper", searchIndexStatus && !searchIndexStatus.isComplete && "has-index-status")}>
      <PopoverTrigger asChild>
        <div className="search-input-shell">
          <SearchIcon />
          <Command className="library-search-command" shouldFilter={false}>
            <CommandInput
              ref={inputRef}
              className="search-input h-10 border-0 bg-transparent pl-0 shadow-none focus-visible:ring-0"
              placeholder="タイトル、著者、タグ、本文で検索..."
              value={value}
              onFocus={() => setOpen(true)}
              onCompositionStart={() => setIsComposing(true)}
              onCompositionEnd={() => setIsComposing(false)}
              onValueChange={next => {
                onChange(next);
                setOpen(true);
              }}
              onKeyDown={event => {
                if (isComposing || event.nativeEvent.isComposing) return;
                if (event.key === "Enter" && value.trim()) {
                  rememberRecent(value.trim()).catch(() => undefined);
                  setOpen(false);
                }
                if (event.key === "Escape") setOpen(false);
              }}
            />
          </Command>
          {indexProgress ? <span className="search-index-status">本文検索を準備中 {indexProgress}</span> : null}
          {!indexProgress && semanticStatus ? <span className="search-index-status">{semanticStatus}</span> : null}
          <div className="search-mode-toggle" role="tablist" aria-label="検索モード">
            {(["smart", "exact", "semantic"] as const).map(mode => (
              <button
                key={mode}
                type="button"
                role="tab"
                aria-selected={searchMode === mode}
                className={cn("search-mode-button", searchMode === mode && "active")}
                onClick={() => onSearchModeChange(mode)}
              >
                {mode === "smart" ? "Smart" : mode === "exact" ? "Exact" : "Semantic"}
              </button>
            ))}
          </div>
        </div>
      </PopoverTrigger>
      <PopoverContent className="library-search-popover">
        <Command shouldFilter={false}>
          <CommandList>
            {value.trim() ? (
              <CommandGroup className="library-search-actions">
                <CommandItem value={`search-${value.trim()}`} onSelect={() => applyQuery(value.trim())}>検索: {value.trim()}</CommandItem>
                <CommandItem value={`semantic-${value.trim()}`} onSelect={() => { onSearchModeChange("semantic"); applyQuery(value.trim()); }}>意味も含めて探す: {value.trim()}</CommandItem>
                <CommandItem value={`save-${value.trim()}`} onSelect={saveCurrent}>保存検索に追加</CommandItem>
              </CommandGroup>
            ) : null}

            {Object.entries(groupedSuggestions).map(([kind, items]) => (
              <CommandGroup key={kind} className="library-search-group">
                <div className="command-group-label">{kind}</div>
                {items.map((item, index) => (
                  <CommandItem key={`${item.kind}-${item.value}-${index}`} value={`${item.kind}-${item.value}-${index}`} onSelect={() => applyToken(suggestionToken(item))}>
                    <span className="library-search-suggestion-main">{item.label}</span>
                    {item.count ? <span className="library-search-suggestion-count">{item.count}</span> : null}
                  </CommandItem>
                ))}
              </CommandGroup>
            ))}

            {savedSearches.length > 0 ? (
              <CommandGroup className="library-search-group">
                <div className="command-group-label">保存検索</div>
                {savedSearches.map(query => (
                  <CommandItem key={`saved-${query}`} value={`saved-${query}`} onSelect={() => applyQuery(query)}>{query}</CommandItem>
                ))}
              </CommandGroup>
            ) : null}

            {recentSearches.length > 0 ? (
              <CommandGroup className="library-search-group">
                <div className="command-group-label">最近の検索</div>
                {recentSearches.map(query => (
                  <CommandItem key={`recent-${query}`} value={`recent-${query}`} onSelect={() => applyQuery(query)}>{query}</CommandItem>
                ))}
              </CommandGroup>
            ) : null}

            <CommandGroup className="library-search-group">
              <div className="command-group-label">構文</div>
              {syntaxHints.map(token => (
                <CommandItem key={token} value={`syntax-${token}`} onSelect={() => applyToken(token)}>{token}</CommandItem>
              ))}
            </CommandGroup>

            {!value.trim() && recentSearches.length === 0 && savedSearches.length === 0 ? (
              <CommandEmpty>候補はありません</CommandEmpty>
            ) : null}
          </CommandList>
        </Command>
      </PopoverContent>
      </div>
    </Popover>
  );
}
