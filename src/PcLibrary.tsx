import { useEffect, useMemo, useState, type ReactNode } from "react";
import { Funnel, X } from "lucide-react";
import { Icons } from "./icons";
import { ContinueShelf } from "./ContinueShelf";
import { activeFilterCount, FilterSheet } from "./FilterSheet";
import { SearchField } from "./SearchField";
import { WorkCard } from "./WorkCard";
import { VirtualWorkList } from "./VirtualWorkList";
import { useReadingShelf } from "./recentReading";
import { CollectionBrowser, CollectionMembers, EntityBrowser } from "./LibraryGroups";
import { ChoiceMenu, SortMenu, relevanceLabel } from "./SortMenu";
import { effectiveSort, entitySortGroups, entitySortLabel, loadEntitySort, saveEntitySort, sortLabels, type EntitySort, type FixedSort } from "./librarySort";
import { defaultPcSearch, pcConnectionIssue, type PcConnectionStatus, type PcSearchCriteria, type PcSort, type PcWork } from "./pcApi";
import { useAppNavigation, type LibraryPage, type LibraryTab } from "./appHistory";
import { useSavedStates, useWorkList, WorkListSkeleton, WorkResults } from "./workList";
import { scrollMainToTop } from "./workText";

const tabs: [LibraryTab, string, ReactNode][] = [
  ["works", "作品", <Icons.works key="works" aria-hidden="true" />],
  ["people", "作者・クリエイター", <Icons.people key="people" aria-hidden="true" />],
  ["series", "シリーズ", <Icons.series key="series" aria-hidden="true" />],
  ["collections", "コレクション", <Icons.collection key="collections" aria-hidden="true" />],
];

/** 読みかけの棚。サーバーへは問い合わせず、この端末で読んだ記録から並べる。 */
function ReadingShelf({ savedRevision, onOpenWork, onOpenDetail, onSaved }: { savedRevision: number; onOpenWork: (work: PcWork) => void; onOpenDetail: (work: PcWork) => void; onSaved: () => void }) {
  const items = useReadingShelf();
  const works = useMemo(() => items.map((item) => item.work), [items]);
  const savedState = useSavedStates(works, savedRevision, true);
  return <>
    <div className="results-bar"><h2>読みかけ</h2><span>この端末で読んだ {items.length} 作品</span></div>
    {items.length ? <VirtualWorkList listKey="reading-shelf" items={items} itemKey={(item) => item.work.id} renderItem={(item) => <WorkCard work={item.work} prefix={`${Math.min(99, Math.round(item.progress * 100))}%`} savedState={savedState(item.work)} onSaved={onSaved} onRead={() => onOpenWork(item.work)} onDetail={() => onOpenDetail(item.work)} />} />
      : <section className="empty-card"><h2>読みかけの作品はありません</h2><p>この端末で読み始めた作品がここに並びます。読み終えると棚から外れます。</p></section>}
  </>;
}

export function PcLibrary({ page, status, statusLoading, onOpenWork, onOpenDetail, onGoToSettings, onSaved, onSaveCondition, savedRevision, pagingMode, rememberedSort, onRememberedSortChange }: {
  page: LibraryPage;
  status: PcConnectionStatus | null;
  statusLoading: boolean;
  onOpenWork: (work: PcWork) => void;
  onOpenDetail: (work: PcWork) => void;
  onGoToSettings: () => void;
  onSaved: () => void;
  onSaveCondition: (name: string, criteria: PcSearchCriteria) => void;
  savedRevision: number;
  pagingMode: "auto" | "pages";
  rememberedSort: FixedSort;
  onRememberedSortChange: (sort: FixedSort) => void;
}) {
  const navigation = useAppNavigation();
  const { tab, criteria, queries, collectionId, listPage, shelf } = page;
  const [filterOpen, setFilterOpen] = useState(false);
  const readingShelf = useReadingShelf();
  const reachable = Boolean(status?.reachable);
  const inCollection = tab === "collections" && collectionId !== null;
  const appliedQuery = tab === "works" ? criteria.text : tab === "collections" && collectionId ? queries.members : queries[tab];
  const [input, setInput] = useState(appliedQuery);
  useEffect(() => { setInput(appliedQuery); }, [appliedQuery, tab, collectionId]);
  const [entitySorts, setEntitySorts] = useState<Record<"people" | "series", EntitySort>>(() => ({ people: loadEntitySort("people"), series: loadEntitySort("series") }));

  const sentSort = effectiveSort(criteria, rememberedSort);
  const search = useMemo(() => ({ ...criteria, sort: sentSort }), [criteria, sentSort]);
  const list = useWorkList({ search, scope: null, pagingMode, page: listPage, enabled: reachable && tab === "works" && shelf !== "reading" });

  const move = (patch: Partial<LibraryPage>) => navigation.push({ page: { ...page, ...patch } });
  const changeCriteria = (next: PcSearchCriteria) => move({ criteria: next, listPage: 1 });

  const submitSearch = (query: string) => {
    if (tab === "works") {
      changeCriteria({ ...criteria, text: query, sort: !query && criteria.sort === "standard" ? "remembered" : criteria.sort });
    } else if (inCollection) {
      move({ queries: { ...queries, members: query } });
    } else {
      move({ queries: { ...queries, [tab]: query } });
    }
  };

  const selectSort = (sort: PcSort) => {
    // 関連度はその場かぎり。それ以外は、いつもの順として覚える。
    if (sort === "standard") {
      navigation.replace({ page: { ...page, criteria: { ...criteria, sort }, listPage: 1 } });
    } else {
      onRememberedSortChange(sort);
      navigation.replace({ page: { ...page, criteria: { ...criteria, sort: "remembered" }, listPage: 1 } });
    }
    scrollMainToTop();
  };

  const selectEntitySort = (kind: "people" | "series", sort: EntitySort) => {
    saveEntitySort(kind, sort);
    setEntitySorts((old) => ({ ...old, [kind]: sort }));
    scrollMainToTop();
  };

  const filterCount = activeFilterCount(criteria);
  // 絞り込みで指定した順。関連度は条件ではなく、その場の表示なので含めない。
  const conditionSortLabel = criteria.sort !== "remembered" && criteria.sort !== "standard" ? sortLabels[criteria.sort] : null;
  const target = tab === "works" ? "作品" : tab === "people" ? "作者・クリエイター" : tab === "series" ? "シリーズ" : inCollection ? "コレクション内の作品" : "コレクション";

  if (statusLoading) return <WorkListSkeleton />;
  if (!reachable) return <section className="empty-card pc-connection-empty"><h2>{status?.paired ? "サーバーの本棚を開けません" : "サーバーの本棚をつなぐ"}</h2><p>{status?.issue ? pcConnectionIssue(status) : "設定でサーバーとの接続を登録すると、作品がここに並びます。"}</p><button className="primary-button" onClick={onGoToSettings}>{status?.issue === "protocol_mismatch" ? "設定で更新を確かめる" : "設定で接続を確認"}</button></section>;

  if (shelf === "reading") return <ReadingShelf savedRevision={savedRevision} onOpenWork={onOpenWork} onOpenDetail={onOpenDetail} onSaved={onSaved} />;

  const resultsTitle = criteria.text ? `「${criteria.text}」の検索結果` : filterCount === 1 && criteria.favorite ? "お気に入り" : filterCount === 1 && criteria.revision === "revised" ? "改稿あり" : null;
  const plainShelf = tab === "works" && !criteria.text && filterCount === 0 && listPage === 1;
  const filterChips: { key: string; text: string; clear: Partial<PcSearchCriteria> }[] = [];
  if (tab === "works") {
    if (criteria.source) filterChips.push({ key: "source", text: criteria.source === "fanbox" ? "FANBOX" : "pixiv", clear: { source: "" } });
    if (criteria.contentType) filterChips.push({ key: "type", text: { novel: "小説", article: "記事", text: "テキスト", image: "画像", file: "ファイル" }[criteria.contentType], clear: { contentType: "" } });
    if (criteria.favorite) filterChips.push({ key: "favorite", text: "お気に入り", clear: { favorite: false } });
    if (criteria.author) filterChips.push({ key: "author", text: `作者: ${criteria.author}`, clear: { author: "" } });
    if (criteria.authorExclude) filterChips.push({ key: "authorExclude", text: `除外作者: ${criteria.authorExclude}`, clear: { authorExclude: "" } });
    if (criteria.tag) filterChips.push({ key: "tag", text: `タグ: ${criteria.tag}${criteria.tagMode === "or" ? "（いずれか）" : ""}`, clear: { tag: "" } });
    if (criteria.tagExclude) filterChips.push({ key: "tagExclude", text: `除外タグ: ${criteria.tagExclude}`, clear: { tagExclude: "" } });
    if (criteria.minChars || criteria.maxChars) filterChips.push({ key: "chars", text: `文字数: ${criteria.minChars || "0"}〜${criteria.maxChars || "上限なし"}`, clear: { minChars: "", maxChars: "" } });
    if (criteria.minAssets || criteria.maxAssets) filterChips.push({ key: "assets", text: `画像数: ${criteria.minAssets || "0"}〜${criteria.maxAssets || "上限なし"}`, clear: { minAssets: "", maxAssets: "" } });
    if (criteria.revision) filterChips.push({ key: "revision", text: criteria.revision === "revised" ? "改稿あり" : "初版のみ", clear: { revision: "" } });
    if (criteria.edit) filterChips.push({ key: "edit", text: criteria.edit === "edited" ? "編集あり" : "編集なし", clear: { edit: "" } });
    if (criteria.cover) filterChips.push({ key: "cover", text: criteria.cover === "has_cover" ? "表紙あり" : "表紙なし", clear: { cover: "" } });
    if (criteria.dateFrom || criteria.dateTo) filterChips.push({ key: "date", text: `${criteria.dateField === "downloaded_at" ? "追加日" : criteria.dateField === "source_created_at" ? "公開日" : "更新日"}: ${criteria.dateFrom || "指定なし"}〜${criteria.dateTo || "指定なし"}`, clear: { dateFrom: "", dateTo: "" } });
    if (conditionSortLabel) filterChips.push({ key: "sort", text: `並び順: ${conditionSortLabel}`, clear: { sort: "remembered" } });
  }

  return <>
    <SearchField value={input} label={`${target}を検索`} placeholder={tab === "works" ? "作品・作者・本文を検索" : `${target}を検索`} onChange={setInput} onSubmit={submitSearch}>
      {tab === "works" && <button type="button" className="filter-button" data-active={filterCount > 0 || undefined} aria-haspopup="dialog" aria-label={filterCount ? `絞り込み（${filterCount} 個の条件）` : "絞り込み"} onClick={() => setFilterOpen(true)}><Funnel aria-hidden="true" />{filterCount > 0 && <small aria-hidden="true">{filterCount}</small>}</button>}
      {tab === "works" && <SortMenu current={sentSort} relevance={Boolean(criteria.text.trim())} onSelect={selectSort} conditionLabel={conditionSortLabel} />}
      {(tab === "people" || tab === "series") && <ChoiceMenu current={entitySorts[tab]} label={entitySortLabel(entitySorts[tab])} groups={entitySortGroups} onSelect={(sort) => selectEntitySort(tab, sort)} />}
    </SearchField>
    {filterChips.length > 0 && <div className="active-filters" aria-label="適用中の絞り込み">{filterChips.map((chip) => <button key={chip.key} type="button" className="active-filter" aria-label={`${chip.text}を解除`} onClick={() => changeCriteria({ ...criteria, ...chip.clear })}>{chip.text}<X aria-hidden="true" /></button>)}{filterChips.length > 1 && <button type="button" className="quiet-button active-filters-clear" onClick={() => changeCriteria({ ...defaultPcSearch, text: criteria.text })}>すべて解除</button>}</div>}
    {filterOpen && <FilterSheet criteria={criteria} onClose={() => setFilterOpen(false)} onSaveCondition={onSaveCondition} onApply={(next) => { setFilterOpen(false); changeCriteria(next); }} />}
    <nav className="library-tabs" aria-label="ライブラリの種類">{tabs.map(([id, label, icon]) => <button key={id} type="button" className={tab === id ? "active" : ""} aria-current={tab === id ? "page" : undefined} onClick={() => move({ tab: id, collectionId: null, listPage: 1 })}>{icon}{label}</button>)}</nav>
    {tab === "people" && <EntityBrowser kind="people" query={queries.people} sort={entitySorts.people} onOpen={(entity) => navigation.openEntity("person", entity.source, entity.sourceKey, entity.displayName)} />}
    {tab === "series" && <EntityBrowser kind="series" query={queries.series} sort={entitySorts.series} onOpen={(entity) => navigation.openEntity("series", entity.source, entity.sourceKey, entity.displayName)} />}
    {tab === "collections" && !collectionId && <CollectionBrowser query={queries.collections} onOpen={(collection) => move({ collectionId: collection.id, queries: { ...queries, members: "" } })} />}
    {tab === "collections" && collectionId && <CollectionMembers collectionId={collectionId} query={queries.members} savedRevision={savedRevision} onOpenWork={onOpenWork} onOpenDetail={onOpenDetail} onSaved={onSaved} />}
    {tab === "works" && <>
      {/* 種類の切り替えより下に置く。上に置くと、作品以外へ切り替えたときに切り替えの列ごと動く。 */}
      {plainShelf && <ContinueShelf items={readingShelf} onOpen={onOpenWork} onAll={() => move({ shelf: "reading" })} />}
      <div className="results-bar">{resultsTitle && <h2>{resultsTitle}</h2>}<span>{list.total != null ? `約 ${list.total.toLocaleString("ja-JP")} 件` : list.loading ? "" : `${list.items.length} 件を表示`}{list.total != null || !list.loading ? "・" : ""}{sentSort === "standard" ? relevanceLabel : sortLabels[sentSort]}</span></div>
      <WorkResults list={list} savedRevision={savedRevision} reachable={reachable} onOpenWork={onOpenWork} onOpenDetail={onOpenDetail} onSaved={onSaved} onPage={(next) => move({ listPage: next })} emptyTitle={criteria.favorite ? "お気に入りの作品が見つかりません" : "作品が見つかりません"} emptyHint={criteria.favorite ? "デスクトップ版のお気に入り登録と絞り込み条件を確認してください。" : "検索語や絞り込み条件を変えて試してください。"} />
      {pagingMode === "pages" && !list.numbered && <p className="paging-relevance-hint">関連度順の検索は自動読み込みです。ページ番号を使うには並び順を選んでください。</p>}
    </>}
  </>;
}
