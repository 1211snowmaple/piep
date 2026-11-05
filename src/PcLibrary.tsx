import { useEffect, useMemo, useState } from "react";
import { CollectionBrowser, CollectionMembers, EntityBrowser } from "./LibraryGroups";
import { ChoiceMenu, SortMenu, relevanceLabel } from "./SortMenu";
import { effectiveSort, entitySortGroups, entitySortLabel, loadEntitySort, saveEntitySort, sortLabels, type EntitySort, type FixedSort } from "./librarySort";
import { defaultPcSearch, pcConnectionIssue, type PcConnectionStatus, type PcSearchCriteria, type PcSort, type PcWork } from "./pcApi";
import { useAppNavigation, type LibraryPage, type LibraryTab } from "./appHistory";
import { useWorkList, WorkListSkeleton, WorkResults } from "./workList";
import { scrollMainToTop } from "./workText";

const tabs: [LibraryTab, string][] = [["works", "作品"], ["people", "作者・クリエイター"], ["series", "シリーズ"], ["collections", "コレクション"]];

export function PcLibrary({ page, status, statusLoading, onOpenWork, onOpenDetail, onGoToSettings, savedRevision, pagingMode, rememberedSort, onRememberedSortChange }: {
  page: LibraryPage;
  status: PcConnectionStatus | null;
  statusLoading: boolean;
  onOpenWork: (work: PcWork) => void;
  onOpenDetail: (work: PcWork) => void;
  onGoToSettings: () => void;
  savedRevision: number;
  pagingMode: "auto" | "pages";
  rememberedSort: FixedSort;
  onRememberedSortChange: (sort: FixedSort) => void;
}) {
  const navigation = useAppNavigation();
  const { tab, criteria, queries, collectionId, listPage } = page;
  const reachable = Boolean(status?.reachable);
  const inCollection = tab === "collections" && collectionId !== null;
  const appliedQuery = tab === "works" ? criteria.text : tab === "collections" && collectionId ? queries.members : queries[tab];
  const [input, setInput] = useState(appliedQuery);
  useEffect(() => { setInput(appliedQuery); }, [appliedQuery, tab, collectionId]);
  const [entitySorts, setEntitySorts] = useState<Record<"people" | "series", EntitySort>>(() => ({ people: loadEntitySort("people"), series: loadEntitySort("series") }));

  const sentSort = effectiveSort(criteria, rememberedSort);
  const search = useMemo(() => ({ ...criteria, sort: sentSort }), [criteria, sentSort]);
  const list = useWorkList({ search, scope: null, pagingMode, page: listPage, enabled: reachable && tab === "works" });

  const move = (patch: Partial<LibraryPage>) => navigation.push({ page: { ...page, ...patch } });
  const changeCriteria = (next: PcSearchCriteria) => move({ criteria: next, listPage: 1 });

  const submitSearch = () => {
    const query = input.trim();
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

  const hasActiveFilter = Object.entries(defaultPcSearch).some(([key, value]) =>
    key !== "text" && !(key === "tagMode" && !criteria.tag) && !(key === "dateField" && !criteria.dateFrom && !criteria.dateTo)
    && !(key === "sort" && criteria.sort === "standard")
    && criteria[key as keyof PcSearchCriteria] !== value);
  // 絞り込みで指定した順。関連度は条件ではなく、その場の表示なので含めない。
  const conditionSortLabel = criteria.sort !== "remembered" && criteria.sort !== "standard" ? sortLabels[criteria.sort] : null;
  const target = tab === "works" ? "作品" : tab === "people" ? "作者・クリエイター" : tab === "series" ? "シリーズ" : inCollection ? "コレクション内の作品" : "コレクション";

  if (statusLoading) return <WorkListSkeleton />;
  if (!reachable) return <section className="empty-card pc-connection-empty"><h2>{status?.paired ? "サーバーの本棚を開けません" : "サーバーの本棚をつなぐ"}</h2><p>{status?.issue ? pcConnectionIssue(status) : "設定でサーバーとの接続を登録すると、作品がここに並びます。"}</p><button className="primary-button" onClick={onGoToSettings}>{status?.issue === "protocol_mismatch" ? "設定で更新を確かめる" : "設定で接続を確認"}</button></section>;

  return <>
    <form className="pc-search-form pc-library-search" onSubmit={(event) => { event.preventDefault(); submitSearch(); }}>
      <input aria-label={`${target}を検索`} type="search" placeholder={tab === "works" ? "作品・作者・本文を検索" : `${target}を検索`} value={input} onChange={(event) => setInput(event.target.value)} />
      {tab === "works" && <SortMenu current={sentSort} relevance={Boolean(criteria.text.trim())} onSelect={selectSort} conditionLabel={conditionSortLabel} />}
      {(tab === "people" || tab === "series") && <ChoiceMenu current={entitySorts[tab]} label={entitySortLabel(entitySorts[tab])} groups={entitySortGroups} onSelect={(sort) => selectEntitySort(tab, sort)} />}
      <button className="primary-button" type="submit">検索</button>
    </form>
    {tab === "works" && hasActiveFilter && <div className="active-filters" aria-label="適用中の絞り込み">{criteria.source && <span>{criteria.source === "fanbox" ? "FANBOX" : "pixiv"}</span>}{criteria.contentType && <span>種類: {{ novel: "小説", article: "記事", text: "テキスト", image: "画像", file: "ファイル" }[criteria.contentType]}</span>}{criteria.author && <span>作者: {criteria.author}</span>}{criteria.authorExclude && <span>除外作者: {criteria.authorExclude}</span>}{criteria.favorite && <button className="favorite-filter-remove" type="button" aria-label="お気に入り条件を解除" onClick={() => changeCriteria({ ...criteria, favorite: false })}>お気に入り ×</button>}{criteria.tag && <span>タグ: {criteria.tag}{criteria.tagMode === "or" ? "（いずれか）" : ""}</span>}{criteria.tagExclude && <span>除外タグ: {criteria.tagExclude}</span>}{(criteria.minChars || criteria.maxChars) && <span>文字数: {criteria.minChars || "0"}〜{criteria.maxChars || "上限なし"}</span>}{(criteria.minAssets || criteria.maxAssets) && <span>画像数: {criteria.minAssets || "0"}〜{criteria.maxAssets || "上限なし"}</span>}{criteria.revision && <span>{criteria.revision === "revised" ? "改稿あり" : "初版のみ"}</span>}{criteria.edit && <span>{criteria.edit === "edited" ? "編集あり" : "編集なし"}</span>}{criteria.cover && <span>{criteria.cover === "has_cover" ? "表紙あり" : "表紙なし"}</span>}{(criteria.dateFrom || criteria.dateTo) && <span>{criteria.dateField === "downloaded_at" ? "保存日" : criteria.dateField === "source_created_at" ? "公開日" : "更新日"}: {criteria.dateFrom || "指定なし"}〜{criteria.dateTo || "指定なし"}</span>}{conditionSortLabel && <button className="favorite-filter-remove" type="button" aria-label="条件の並び順を解除" onClick={() => changeCriteria({ ...criteria, sort: "remembered" })}>{conditionSortLabel}（この条件の順）×</button>}<button type="button" onClick={() => changeCriteria(defaultPcSearch)}>絞り込みを解除</button></div>}
    <nav className="library-tabs" aria-label="ライブラリの種類">{tabs.map(([id, label]) => <button key={id} type="button" className={tab === id ? "active" : ""} aria-current={tab === id ? "page" : undefined} onClick={() => move({ tab: id, collectionId: null, listPage: 1 })}>{label}</button>)}</nav>
    {tab === "people" && <EntityBrowser kind="people" query={queries.people} sort={entitySorts.people} onOpen={(entity) => navigation.openEntity("person", entity.source, entity.sourceKey, entity.displayName)} />}
    {tab === "series" && <EntityBrowser kind="series" query={queries.series} sort={entitySorts.series} onOpen={(entity) => navigation.openEntity("series", entity.source, entity.sourceKey, entity.displayName)} />}
    {tab === "collections" && !collectionId && <CollectionBrowser query={queries.collections} onOpen={(collection) => move({ collectionId: collection.id, queries: { ...queries, members: "" } })} />}
    {tab === "collections" && collectionId && <CollectionMembers collectionId={collectionId} query={queries.members} onBack={navigation.back} onOpenWork={onOpenWork} onOpenDetail={onOpenDetail} />}
    {tab === "works" && <>
      <div className="section-heading pc-results-heading"><h2>{criteria.text ? `「${criteria.text}」の検索結果` : criteria.favorite ? "お気に入り" : "すべての作品"}</h2><span>{list.total != null ? `約 ${list.total.toLocaleString("ja-JP")} 件` : list.loading ? "" : `${list.items.length} 件を表示`}<span className="pc-results-sort">{list.total != null || !list.loading ? "・" : ""}{sentSort === "standard" ? relevanceLabel : sortLabels[sentSort]}</span></span></div>
      <WorkResults list={list} savedRevision={savedRevision} reachable={reachable} onOpenWork={onOpenWork} onOpenDetail={onOpenDetail} onPage={(next) => move({ listPage: next })} emptyTitle={criteria.favorite ? "お気に入りの作品が見つかりません" : "作品が見つかりません"} emptyHint={criteria.favorite ? "デスクトップ版のお気に入り登録と絞り込み条件を確認してください。" : "検索語や絞り込み条件を変えて試してください。"} />
      {pagingMode === "pages" && !list.numbered && <p className="paging-relevance-hint">関連度順の検索は自動読み込みです。ページ番号を使うには並び順を選んでください。</p>}
    </>}
  </>;
}
