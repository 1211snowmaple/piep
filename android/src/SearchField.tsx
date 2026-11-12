import { Icons } from "./icons";
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { searchPcSuggestions, type SearchSuggestion } from "./pcApi";

/**
 * 検索の列。虫眼鏡は欄の中に置き、右に絞り込みと並び替えを並べる。デスクトップ版と同じ形。
 * 大きな検索ボタンを別に立てると、確定するだけの操作がいちばん目立ってしまう。
 */
export function SearchField({ value, placeholder, label, onChange, onSubmit, onSuggestion, children }: {
  value: string;
  placeholder: string;
  label: string;
  onChange: (value: string) => void;
  onSubmit: (value: string) => void;
  onSuggestion?: (item: SearchSuggestion) => void;
  children?: ReactNode;
}) {
  const id = useId();
  const input = useRef<HTMLInputElement>(null);
  const composing = useRef(false);
  const [focused, setFocused] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const [active, setActive] = useState(-1);
  const [result, setResult] = useState<{ query: string; items: SearchSuggestion[] } | null>(null);
  useEffect(() => {
    let alive = true;
    setActive(-1);
    if (!onSuggestion || !focused || !value.trim() || value.length > 200) return;
    const timer = window.setTimeout(() => {
      void searchPcSuggestions(value).then((items) => { if (alive) setResult({ query: value, items }); }).catch(() => { if (alive) setResult(null); });
    }, 220);
    return () => { alive = false; window.clearTimeout(timer); };
  }, [value, focused, Boolean(onSuggestion)]);
  const items = result?.query === value ? result.items : [];
  const expanded = Boolean(onSuggestion && focused && !dismissed && items.length);
  useEffect(() => {
    if (expanded && active >= 0) document.getElementById(`${id}-${active}`)?.scrollIntoView?.({ block: "nearest" });
  }, [expanded, active, id]);
  const choose = (item: SearchSuggestion) => { setDismissed(true); setFocused(false); input.current?.blur(); onSuggestion?.(item); };
  return <form className="search-row" role="search" onSubmit={(event) => {
    event.preventDefault();
    if (composing.current) return;
    if (expanded && active >= 0 && items[active]) choose(items[active]);
    else { onSubmit(value.trim()); setDismissed(true); input.current?.blur(); }
  }}>
    <div className="search-field" onBlur={(event) => { if (!event.currentTarget.contains(event.relatedTarget)) setFocused(false); }}>
      <button type="submit" className="search-field-go" aria-label="検索"><Icons.search aria-hidden="true" /></button>
      <input ref={input} type="search" enterKeyHint="search" aria-label={label} placeholder={placeholder} value={value}
        role={onSuggestion ? "combobox" : undefined} aria-autocomplete={onSuggestion ? "list" : undefined}
        aria-expanded={onSuggestion ? expanded : undefined} aria-controls={expanded ? id : undefined}
        aria-activedescendant={expanded && active >= 0 ? `${id}-${active}` : undefined}
        onFocus={() => { setFocused(true); setDismissed(false); }}
        onCompositionStart={() => { composing.current = true; }} onCompositionEnd={() => { composing.current = false; }}
        onKeyDown={(event) => {
          if (event.nativeEvent.isComposing || composing.current || event.keyCode === 229) return;
          if (event.key === "Escape") { setDismissed(true); setActive(-1); event.preventDefault(); }
          if (expanded && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
            event.preventDefault();
            setActive((old) => event.key === "ArrowDown" ? (old + 1) % items.length : old <= 0 ? items.length - 1 : old - 1);
          }
        }} onChange={(event) => { setDismissed(false); onChange(event.target.value); }} />
      {value && <button type="button" className="search-field-clear" aria-label="検索語を消す" onClick={() => { onChange(""); onSubmit(""); }}><Icons.cancel aria-hidden="true" /></button>}
      {expanded && <div className="search-suggestions" id={id} role="listbox" aria-label="検索候補">{items.map((item, index) => <button
        key={`${item.kind}:${item.source ?? ""}:${item.sourceKey ?? item.value}`} id={`${id}-${index}`} type="button" role="option" tabIndex={-1} aria-selected={active === index}
        onMouseDown={(event) => event.preventDefault()} onClick={() => choose(item)}>
        <span className="search-suggestion-kind">{{ tag: "タグ", author: "作者", series: "シリーズ", title: "作品" }[item.kind] ?? "検索"}</span>
        <span>{item.label}</span>{item.count !== null && <small>{item.count.toLocaleString("ja-JP")}</small>}
      </button>)}</div>}
    </div>
    {children}
  </form>;
}
