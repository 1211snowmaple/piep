import { useEffect, useRef, useState } from "react";
import { ArrowDownWideNarrow, Check } from "lucide-react";
import { sortGroups, sortLabels, type MenuGroup } from "./librarySort";
import type { PcSort } from "./pcApi";

export const relevanceLabel = "関連度順";

/** 検索欄の横に置く並び替えの入れ物。作品でも作者でも形を同じにする。
 * 携帯やタブレットでは横幅が足りないので印だけにする。いまの順は読み上げと開いた一覧で分かる。 */
export function ChoiceMenu<T extends string>({ current, label, groups, note, marked, onSelect }: {
  current: T;
  label: string;
  groups: MenuGroup<T>[];
  note?: string | null;
  marked?: boolean;
  onSelect: (value: T) => void;
}) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return;
    const backWindow = window as Window & { piepAndroidSortBack?: () => boolean };
    const close = () => { setOpen(false); trigger.current?.focus(); };
    const outside = (event: PointerEvent) => { if (!root.current?.contains(event.target as Node)) setOpen(false); };
    const keys = (event: KeyboardEvent) => { if (event.key === "Escape") { event.stopPropagation(); close(); } };
    backWindow.piepAndroidSortBack = () => { close(); return true; };
    document.addEventListener("pointerdown", outside);
    window.addEventListener("keydown", keys);
    root.current?.querySelector<HTMLElement>('[aria-checked="true"]')?.focus({ preventScroll: true });
    root.current?.querySelector<HTMLElement>('[aria-checked="true"]')?.scrollIntoView({ block: "nearest" });
    return () => {
      delete backWindow.piepAndroidSortBack;
      document.removeEventListener("pointerdown", outside);
      window.removeEventListener("keydown", keys);
    };
  }, [open]);

  const choose = (value: T) => {
    setOpen(false);
    onSelect(value);
    trigger.current?.focus();
  };

  return <div className="sort-menu" ref={root}>
    <button ref={trigger} type="button" className="sort-button" data-condition={marked ? "" : undefined} aria-haspopup="menu" aria-expanded={open} aria-label={`並び替え（現在は${label}）`} onClick={() => setOpen((value) => !value)}>
      <ArrowDownWideNarrow aria-hidden="true" />
    </button>
    {open && <div className="sort-popover" role="menu" aria-label="並び替え">
      {note && <p className="sort-popover-note">{note}</p>}
      {groups.map((group) => <div className="sort-popover-group" key={group.label}><span>{group.label}</span>{group.items.map((item) => <button key={item.value} type="button" role="menuitemradio" aria-checked={current === item.value} onClick={() => choose(item.value)}><span>{item.label}</span>{current === item.value && <Check aria-hidden="true" />}</button>)}</div>)}
    </div>}
  </div>;
}

const fixedGroups: MenuGroup<PcSort>[] = sortGroups.map((group) => ({ label: group.label, items: group.items.map((sort) => ({ value: sort, label: sortLabels[sort] })) }));

/** 本棚の並び替え。ここで選んだ順は、次に開いたときも使う並び順として覚える。関連度はその場かぎりで覚えない。 */
export function SortMenu({ current, relevance = false, onSelect, conditionLabel }: {
  current: PcSort;
  relevance?: boolean;
  onSelect: (sort: PcSort) => void;
  conditionLabel?: string | null;
}) {
  const label = current === "standard" ? relevanceLabel : sortLabels[current];
  const groups: MenuGroup<PcSort>[] = relevance ? [{ label: "この検索だけ", items: [{ value: "standard", label: relevanceLabel }] }, ...fixedGroups] : fixedGroups;
  return <ChoiceMenu
    current={current}
    label={label}
    groups={groups}
    marked={Boolean(conditionLabel)}
    note={conditionLabel ? `いまは絞り込みで「${conditionLabel}」を指定しています。ここで選ぶと絞り込みの並び順の指定を外して、選んだ順で並べます。` : null}
    onSelect={onSelect}
  />;
}
