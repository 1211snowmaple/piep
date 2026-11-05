import type { ReactNode } from "react";
import { Search, X } from "lucide-react";

/**
 * 検索の列。虫眼鏡は欄の中に置き、右に絞り込みと並び替えを並べる。デスクトップ版と同じ形。
 * 大きな検索ボタンを別に立てると、確定するだけの操作がいちばん目立ってしまう。
 */
export function SearchField({ value, placeholder, label, onChange, onSubmit, children }: {
  value: string;
  placeholder: string;
  label: string;
  onChange: (value: string) => void;
  onSubmit: (value: string) => void;
  children?: ReactNode;
}) {
  return <form className="search-row" role="search" onSubmit={(event) => { event.preventDefault(); onSubmit(value.trim()); event.currentTarget.querySelector("input")?.blur(); }}>
    <div className="search-field">
      <button type="submit" className="search-field-go" aria-label="検索"><Search aria-hidden="true" /></button>
      <input type="search" enterKeyHint="search" aria-label={label} placeholder={placeholder} value={value} onChange={(event) => onChange(event.target.value)} />
      {value && <button type="button" className="search-field-clear" aria-label="検索語を消す" onClick={() => { onChange(""); onSubmit(""); }}><X aria-hidden="true" /></button>}
    </div>
    {children}
  </form>;
}
