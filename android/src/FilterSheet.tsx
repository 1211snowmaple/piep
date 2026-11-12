import { useEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Bookmark, Check, X } from "lucide-react";
import { defaultPcSearch, type PcSearchCriteria } from "./pcApi";
import { DATE_FIELD_OPTIONS } from "@shared/dateFields";
import { describeCondition, hasConditionToSave, relevanceLabel, sortGroups, sortLabels } from "./librarySort";

/** 検索語を除いて、効いている条件を数える。上限と下限は合わせて一つと考える。 */
export function activeFilterCount(criteria: PcSearchCriteria): number {
  const on = [
    criteria.source, criteria.contentType, criteria.author, criteria.authorExclude, criteria.favorite,
    criteria.tag, criteria.tagExclude, criteria.minChars || criteria.maxChars, criteria.minAssets || criteria.maxAssets,
    criteria.revision, criteria.edit, criteria.cover, criteria.dateFrom || criteria.dateTo,
    criteria.sort !== "remembered" && criteria.sort !== "standard",
  ];
  return on.filter(Boolean).length;
}

function Chips<T extends string>({ label, value, options, onChange }: { label: string; value: T; options: { id: T; label: string }[]; onChange: (value: T) => void }) {
  return <div className="filter-group">
    <span className="filter-label">{label}</span>
    <div className="filter-chips" role="group" aria-label={label}>{options.map((option) => <button key={option.id} type="button" className="filter-chip" aria-pressed={value === option.id} onClick={() => onChange(option.id)}>{value === option.id && <Check aria-hidden="true" />}{option.label}</button>)}</div>
  </div>;
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return <section className="filter-section"><h3>{title}</h3>{children}</section>;
}

function problemOf(draft: PcSearchCriteria): string | null {
  if (draft.minChars && draft.maxChars && Number(draft.minChars) > Number(draft.maxChars)) return "文字数の下限は上限以下にしてください。";
  if (draft.minAssets && draft.maxAssets && Number(draft.minAssets) > Number(draft.maxAssets)) return "添付数の下限は上限以下にしてください。";
  if (draft.dateFrom && draft.dateTo && draft.dateFrom > draft.dateTo) return "日付の開始日は終了日以前にしてください。";
  for (const value of [draft.author, draft.authorExclude, draft.tag, draft.tagExclude]) {
    if (value.split(/[\n,、]+/).filter((part) => part.trim()).length > 10) return "作者とタグはそれぞれ 10 件まで指定できます。";
  }
  return null;
}

function trimmed(draft: PcSearchCriteria): PcSearchCriteria {
  return { ...draft, author: draft.author.trim(), authorExclude: draft.authorExclude.trim(), tag: draft.tag.trim(), tagExclude: draft.tagExclude.trim(), minChars: draft.minChars.trim(), maxChars: draft.maxChars.trim(), minAssets: draft.minAssets.trim(), maxAssets: draft.maxAssets.trim() };
}

/**
 * 絞り込み。デスクトップ版の「詳細フィルター」と同じく、選ぶものは札で並べ、足元に
 * リセットと適用を置く。サイドバーへ入力欄を積むと、本棚の横に長い書式が居座ってしまう。
 */
export function FilterSheet({ criteria, onApply, onSaveCondition, onClose, scope }: {
  criteria: PcSearchCriteria;
  onApply: (next: PcSearchCriteria) => void;
  /** 本棚の条件だけ保存できる。作者やシリーズの中の絞り込みは、その画面の中だけで使う。 */
  onSaveCondition?: (name: string, criteria: PcSearchCriteria) => void;
  onClose: () => void;
  /**
   * 作者やシリーズの画面の中で絞るとき。作者の画面では作者の欄を出さない。
   * 含めるタグと並び順はその画面の札と並び替えで選ぶので、ここには出さない。
   */
  scope?: "person" | "series";
}) {
  const [draft, setDraft] = useState(criteria);
  const [problem, setProblem] = useState<string | null>(null);
  const [naming, setNaming] = useState(false);
  const [name, setName] = useState("");
  const [savedName, setSavedName] = useState<string | null>(null);
  const set = (patch: Partial<PcSearchCriteria>) => { setDraft((old) => ({ ...old, ...patch })); setProblem(null); setSavedName(null); };
  const count = activeFilterCount(draft);
  const sheet = useRef<HTMLElement>(null);

  useEffect(() => {
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const shell = document.querySelector(".app-shell");
    shell?.setAttribute("inert", "");
    sheet.current?.focus({ preventScroll: true });
    return () => { shell?.removeAttribute("inert"); previous?.focus({ preventScroll: true }); };
  }, []);

  useEffect(() => {
    const backWindow = window as Window & { piepAndroidSortBack?: () => boolean };
    backWindow.piepAndroidSortBack = () => { onClose(); return true; };
    const keys = (event: KeyboardEvent) => { if (event.key === "Escape") { event.stopPropagation(); onClose(); } };
    window.addEventListener("keydown", keys);
    return () => { delete backWindow.piepAndroidSortBack; window.removeEventListener("keydown", keys); };
  }, [onClose]);

  const apply = () => {
    const invalid = problemOf(draft);
    if (invalid) { setProblem(invalid); return; }
    onApply(trimmed(draft));
  };

  const saveCondition = () => {
    const invalid = problemOf(draft);
    if (invalid) { setProblem(invalid); return; }
    const next = trimmed(draft);
    const finalName = name.trim() || describeCondition(next);
    onSaveCondition?.(finalName, next);
    setSavedName(finalName);
    setNaming(false);
    setName("");
  };

  const textField = (label: string, key: "author" | "authorExclude" | "tag" | "tagExclude") => <label>{label}<input value={draft[key]} maxLength={1000} placeholder="読点で区切って指定" onChange={(event) => set({ [key]: event.target.value })} /></label>;
  const range = (label: string, minKey: "minChars" | "minAssets", maxKey: "maxChars" | "maxAssets", max: string) => <div className="filter-range"><span className="filter-label">{label}</span><input aria-label={`${label}の下限`} type="number" inputMode="numeric" min="0" max={max} value={draft[minKey]} placeholder="下限" onChange={(event) => set({ [minKey]: event.target.value })} /><span aria-hidden="true">〜</span><input aria-label={`${label}の上限`} type="number" inputMode="numeric" min="0" max={max} value={draft[maxKey]} placeholder="上限" onChange={(event) => set({ [maxKey]: event.target.value })} /></div>;

  return createPortal(<div className="filter-layer">
    <button type="button" className="filter-scrim" aria-label="絞り込みを閉じる" onClick={onClose} />
    <aside className="filter-sheet" ref={sheet} tabIndex={-1} role="dialog" aria-modal="true" aria-label="絞り込み">
      <span className="filter-grab" aria-hidden="true" />
      <header className="filter-header">
        <h2>絞り込み</h2>
        <button type="button" className="icon-button" aria-label="閉じる" onClick={onClose}><X aria-hidden="true" /></button>
      </header>
      <div className="filter-body">
        <Section title="基本">
          <Chips label="取得元" value={draft.source} options={[{ id: "", label: "すべて" }, { id: "pixiv", label: "pixiv" }, { id: "fanbox", label: "FANBOX" }]} onChange={(source) => set({ source })} />
          <Chips label="種類" value={draft.contentType} options={[{ id: "", label: "すべて" }, { id: "novel", label: "小説" }, { id: "article", label: "記事" }, { id: "text", label: "テキスト" }, { id: "image", label: "画像" }, { id: "file", label: "ファイル" }]} onChange={(contentType) => set({ contentType })} />
          <Chips label="お気に入り" value={draft.favorite ? "on" : "off"} options={[{ id: "off", label: "すべて" }, { id: "on", label: "お気に入りのみ" }]} onChange={(value) => set({ favorite: value === "on" })} />
        </Section>
        <Section title={scope === "person" ? "タグ" : "作者とタグ"}>
          <div className="filter-fields">
            {scope !== "person" && textField("作者", "author")}
            {scope !== "person" && textField("除外する作者", "authorExclude")}
            {!scope && textField("タグ", "tag")}
            {textField("除外するタグ", "tagExclude")}
          </div>
          {!scope && <Chips label="複数のタグ" value={draft.tagMode} options={[{ id: "and", label: "すべて含む" }, { id: "or", label: "いずれかを含む" }]} onChange={(tagMode) => set({ tagMode })} />}
          <p className="filter-hint">{scope ? "含めるタグは一覧の上の札で選びます。" : ""}{scope === "person" ? "タグ" : "作者とタグ"}は読点、コンマ、改行で 10 件まで。検索語には「語句」や -語句 も使えます。</p>
        </Section>
        <Section title="状態">
          <Chips label="改稿" value={draft.revision} options={[{ id: "", label: "すべて" }, { id: "revised", label: "改稿あり" }, { id: "first_version", label: "初版のみ" }]} onChange={(revision) => set({ revision })} />
          <Chips label="ローカル編集" value={draft.edit} options={[{ id: "", label: "すべて" }, { id: "edited", label: "編集あり" }, { id: "unedited", label: "編集なし" }]} onChange={(edit) => set({ edit })} />
          <Chips label="表紙" value={draft.cover} options={[{ id: "", label: "すべて" }, { id: "has_cover", label: "表紙あり" }, { id: "no_cover", label: "表紙なし" }]} onChange={(cover) => set({ cover })} />
        </Section>
        <Section title="分量と日付">
          {range("文字数", "minChars", "maxChars", "1000000000")}
          {range("添付数", "minAssets", "maxAssets", "1000000")}
          <Chips label="日付の種類" value={draft.dateField} options={DATE_FIELD_OPTIONS.map((option) => ({ id: option.value, label: option.label }))} onChange={(dateField) => set({ dateField })} />
          <div className="filter-range"><span className="filter-label">期間</span><input aria-label="開始日" type="date" value={draft.dateFrom} onChange={(event) => set({ dateFrom: event.target.value })} /><span aria-hidden="true">〜</span><input aria-label="終了日" type="date" value={draft.dateTo} onChange={(event) => set({ dateTo: event.target.value })} /></div>
        </Section>
        {!scope && <Section title="この条件の並び順">
          <select aria-label="この条件の並び順" className="filter-select" value={draft.sort} onChange={(event) => set({ sort: event.target.value as PcSearchCriteria["sort"] })}><option value="remembered">指定しない</option>{draft.text.trim() && <option value="standard">{relevanceLabel}</option>}{sortGroups.map((group) => <optgroup key={group.label} label={group.label}>{group.items.map((sort) => <option key={sort} value={sort}>{sortLabels[sort]}</option>)}</optgroup>)}</select>
          <p className="filter-hint">指定しなければ、検索欄の横の並び替えで選んだ順になります。ここで選ぶと、この条件で絞り込んでいる間だけその順で並べます。</p>
        </Section>}
        {!onSaveCondition ? null : naming ? <div className="filter-save"><input autoFocus aria-label="保存する条件の名前" value={name} maxLength={60} placeholder={describeCondition(draft)} onChange={(event) => setName(event.target.value)} /><button type="button" className="secondary-button" onClick={saveCondition}>保存</button><button type="button" className="icon-button" aria-label="保存をやめる" onClick={() => setNaming(false)}><X aria-hidden="true" /></button></div>
          : <button type="button" className="quiet-button" disabled={!hasConditionToSave(draft)} onClick={() => setNaming(true)}><Bookmark aria-hidden="true" />この条件を保存</button>}
        {savedName && <p className="filter-hint" role="status">「{savedName}」を保存しました。サイドバーから開けます。</p>}
        {problem && <p className="inline-alert" role="alert">{problem}</p>}
      </div>
      <footer className="filter-footer">
        <span className="filter-summary">{count ? `${count} 個の条件` : "すべての作品"}</span>
        <button type="button" className="secondary-button" onClick={() => { setDraft({ ...defaultPcSearch, text: draft.text }); setProblem(null); setSavedName(null); }}>リセット</button>
        <button type="button" className="primary-button" onClick={apply}>適用</button>
      </footer>
    </aside>
  </div>, document.body);
}
