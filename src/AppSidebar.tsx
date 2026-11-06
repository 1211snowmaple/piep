import { useEffect, useState, type ReactNode, type Ref } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { isTauri } from "@tauri-apps/api/core";
import { useQuery } from "@tanstack/react-query";
import { BookMarked, BookOpen, Heart, History, LibraryBig, Rows3, Search, Settings, X } from "lucide-react";
import { PiepLockup } from "./PiepLockup";
import type { AppPage } from "./appHistory";
import { defaultPcSearch, searchPcWorks, type PcConnectionStatus, type PcSearchCriteria } from "./pcApi";
import type { SavedCondition } from "./librarySort";
import { useReadingShelf } from "./recentReading";

export type Section = "pc" | "offline" | "settings";
export type Shelf = "all" | "favorite" | "reading" | "revised";

const shelfCriteria: Record<Exclude<Shelf, "reading">, Partial<PcSearchCriteria>> = { all: {}, favorite: { favorite: true }, revised: { revision: "revised" } };

function currentShelf(page: AppPage): Shelf | null {
  if (page.name !== "library" || page.tab !== "works") return null;
  if (page.shelf === "reading") return "reading";
  for (const shelf of ["all", "favorite", "revised"] as const) {
    const wanted: PcSearchCriteria = { ...defaultPcSearch, ...shelfCriteria[shelf] };
    if ((Object.keys(wanted) as (keyof PcSearchCriteria)[]).every((key) => ["sort", "tagMode", "dateField"].includes(key) || page.criteria[key] === wanted[key])) return shelf;
  }
  return null;
}

/** サイドバーが引き出しになる幅。App.desktop.css と同じ条件。 */
const drawerLayout = "(max-width: 599px), (max-width: 900px) and (orientation: portrait)";

function useShelfCount(shelf: Exclude<Shelf, "reading">, enabled: boolean): number | null {
  return useQuery({
    queryKey: ["shelf-count", shelf],
    // 欲しいのは件数だけ。一覧の中身を運ばない。
    queryFn: () => searchPcWorks({ ...defaultPcSearch, ...shelfCriteria[shelf], sort: "dateDesc" }, null, null, null, 1),
    enabled,
    staleTime: 60_000,
    select: (page) => page.totalEstimate ?? null,
  }).data ?? null;
}

/** サイドバーの下の一行。デスクトップ版の「● デスクトップ　v0.35.0」と同じ形で、つながり方と版を出す。 */
function connectionLabel(status: PcConnectionStatus | null): string {
  if (!status?.paired) return "サーバー未登録";
  if (status.reachable) return "オンライン";
  if (status.issue === "not_ready") return "サーバー準備中";
  return "オフライン";
}

function useAppVersion(): string | null {
  const [version, setVersion] = useState<string | null>(null);
  useEffect(() => {
    if (!isTauri()) return;
    let alive = true;
    getVersion().then((value) => { if (alive) setVersion(value); }).catch(() => undefined);
    return () => { alive = false; };
  }, []);
  return version;
}

function ShelfItem({ icon, label, count, active, buttonRef, onClick }: { icon: ReactNode; label: string; count: number | null; active: boolean; buttonRef?: Ref<HTMLButtonElement>; onClick: () => void }) {
  return <button type="button" ref={buttonRef} className={active ? "shelf-item active" : "shelf-item"} aria-current={active ? "page" : undefined} onClick={onClick}>
    {icon}<span>{label}</span>{count !== null && <small>{count.toLocaleString("ja-JP")}</small>}
  </button>;
}

/**
 * デスクトップ版のサイドバーと同じ組み立て。ライブラリの下に棚を字下げして並べ、
 * その下に保存した検索を置く。条件の入力は絞り込みの面へ移し、ここは行き先だけにする。
 * 下メニューのある幅では、端末内と設定は下メニューに任せて二重に置かない。
 */
export function AppSidebar({ open, section, page, status, savedConditions, firstControlRef, onSection, onShelf, onCondition, onRemoveCondition }: {
  open: boolean;
  section: Section;
  page: AppPage;
  status: PcConnectionStatus | null;
  savedConditions: SavedCondition[];
  firstControlRef: Ref<HTMLButtonElement>;
  onSection: (section: Section) => void;
  onShelf: (shelf: Shelf) => void;
  onCondition: (condition: SavedCondition) => void;
  onRemoveCondition: (condition: SavedCondition) => void;
}) {
  const reachable = Boolean(status?.reachable);
  const version = useAppVersion();
  const active = currentShelf(page);
  const reading = useReadingShelf();
  // 閉じている引き出しの数字のために、起動時の通信を増やさない。
  const visible = open || !window.matchMedia(drawerLayout).matches;
  const all = useShelfCount("all", reachable && visible);
  const favorite = useShelfCount("favorite", reachable && visible);
  const revised = useShelfCount("revised", reachable && visible);

  const destination = (id: Section, label: string, icon: ReactNode) => <button type="button" className={section === id ? "nav-item active" : "nav-item"} aria-current={section === id ? "page" : undefined} onClick={() => onSection(id)}>{icon}<span>{label}</span></button>;

  return <aside className={open ? "sidebar mobile-open" : "sidebar"} id="app-navigation" aria-label="サイドバー">
    <div className="brand"><PiepLockup size={24} /></div>
    <nav className="side-nav" aria-label="画面の移動">
      <div className="side-destination">{destination("pc", "ライブラリ", <LibraryBig className="icon" aria-hidden="true" />)}</div>
      <div className="shelf-list">
        <ShelfItem buttonRef={firstControlRef} icon={<Rows3 aria-hidden="true" />} label="すべて" count={all} active={active === "all"} onClick={() => onShelf("all")} />
        <ShelfItem icon={<Heart aria-hidden="true" />} label="お気に入り" count={favorite} active={active === "favorite"} onClick={() => onShelf("favorite")} />
        <ShelfItem icon={<BookMarked aria-hidden="true" />} label="読みかけ" count={reading.length || null} active={active === "reading"} onClick={() => onShelf("reading")} />
        <ShelfItem icon={<History aria-hidden="true" />} label="改稿あり" count={revised} active={active === "revised"} onClick={() => onShelf("revised")} />
      </div>
      <div className="saved-search">
        <span className="sidebar-group-label">保存した検索</span>
        {savedConditions.length > 0
          ? <ul>{savedConditions.map((condition) => <li key={condition.id}><button type="button" className="shelf-item" onClick={() => onCondition(condition)}><Search aria-hidden="true" /><span>{condition.name}</span></button><button type="button" className="saved-search-remove" aria-label={`${condition.name}を削除`} onClick={() => onRemoveCondition(condition)}><X aria-hidden="true" /></button></li>)}</ul>
          : <p>絞り込みの条件を保存すると、ここから開けます。</p>}
      </div>
      <div className="side-destination side-destination--end">
        {destination("offline", "端末内", <BookOpen className="icon" aria-hidden="true" />)}
        {destination("settings", "設定", <Settings className="icon" aria-hidden="true" />)}
      </div>
    </nav>
    <div className={reachable ? "sidebar-bottom connected" : "sidebar-bottom"} role="status"><span className="status-dot" aria-hidden="true" /><span className="sidebar-connection">{connectionLabel(status)}</span>{version && <span className="sidebar-version">v{version}</span>}</div>
  </aside>;
}
