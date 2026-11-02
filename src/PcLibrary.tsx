import { useCallback, useEffect, useRef, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { WorkCard } from "./WorkCard";
import { VirtualWorkList } from "./VirtualWorkList";
import { CollectionBrowser, EntityBrowser } from "./LibraryGroups";
import { defaultPcSearch, getPcMetadata, getPcStatus, listOfflineWorks, pcConnectionIssue, pcErrorMessage, searchPcWorks, type OfflineWork, type PcConnectionStatus, type PcEntity, type PcSearchCriteria, type PcSearchPage, type PcWork, type PcWorkScope } from "./pcApi";

type LibraryTab = "works" | "people" | "series" | "collections";
type SelectedGroup = PcWorkScope & { name: string };
const sortLabels: Record<PcSearchCriteria["sort"], string> = {
  standard: "自動", dateDesc: "保存日：新しい順", dateAsc: "保存日：古い順", publishedDesc: "公開日：新しい順", publishedAsc: "公開日：古い順",
  titleAsc: "タイトル：昇順", titleDesc: "タイトル：降順", authorAsc: "作者名：昇順", authorDesc: "作者名：降順",
  lengthDesc: "文字数：多い順", lengthAsc: "文字数：少ない順", assetsDesc: "画像数：多い順", assetsAsc: "画像数：少ない順",
  versionDesc: "版番号：大きい順", versionAsc: "版番号：小さい順",
};

export function PcLibrary({ criteria, onCriteriaChange, onOpenWork, onOpenDetail, onStatusChange, onGoToSettings, navigationKey, savedRevision, pagingMode }: { criteria: PcSearchCriteria; onCriteriaChange: (criteria: PcSearchCriteria) => void; onOpenWork: (work: PcWork) => void; onOpenDetail: (work: PcWork) => void; onStatusChange: (status: PcConnectionStatus) => void; onGoToSettings: () => void; navigationKey: number; savedRevision: number; pagingMode: "auto" | "pages" }) {
  const [status, setStatus] = useState<PcConnectionStatus | null>(null);
  const [loadingStatus, setLoadingStatus] = useState(true);
  const [input, setInput] = useState(criteria.text);
  const [appliedQuery, setAppliedQuery] = useState(criteria.text);
  const [tabQueries, setTabQueries] = useState<Record<LibraryTab, string>>({ works: criteria.text, people: "", series: "", collections: "" });
  const [results, setResults] = useState<PcSearchPage | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [savedWorks, setSavedWorks] = useState<OfflineWork[]>([]);
  const [editHashes, setEditHashes] = useState<Record<number, string | null>>({});
  const [tab, setTab] = useState<LibraryTab>("works");
  const [group, setGroup] = useState<SelectedGroup | null>(null);
  const [collectionSelected, setCollectionSelected] = useState(false);
  const collectionListQuery = useRef("");
  const [currentPage, setCurrentPage] = useState(1);
  const numbered = pagingMode === "pages" && !(criteria.text.trim() && criteria.sort === "standard");
  const pageSize = 20;
  const loadMoreRef = useRef<HTMLDivElement>(null);
  const requestIdRef = useRef(0);
  const criteriaRef = useRef(criteria);
  criteriaRef.current = criteria;
  const groupRef = useRef(group);
  groupRef.current = group;

  useEffect(() => { setTab("works"); setGroup(null); setCollectionSelected(false); }, [navigationKey]);

  const openGroup = (kind: "person" | "series", entity: PcEntity) => {
    setProblem(null);
    setGroup({ kind, source: entity.source, key: entity.sourceKey, name: entity.displayName });
    setTab("works");
    setInput("");
    setAppliedQuery("");
    setTabQueries((previous) => ({ ...previous, works: "" }));
    onCriteriaChange(defaultPcSearch);
    document.querySelector<HTMLElement>(".main-content")?.scrollTo({ top: 0, behavior: "instant" });
  };

  const selectTab = (next: LibraryTab) => {
    setProblem(null);
    setGroup(null);
    setTab(next);
    setCollectionSelected(false);
    const query = next === "works" ? criteria.text : tabQueries[next];
    setInput(query);
    setAppliedQuery(query);
    document.querySelector<HTMLElement>(".main-content")?.scrollTo({ top: 0, behavior: "instant" });
  };

  const submitSearch = () => {
    setProblem(null);
    const query = input.trim();
    setAppliedQuery(query);
    if (!(tab === "collections" && collectionSelected)) setTabQueries((previous) => ({ ...previous, [tab]: query }));
    if (tab === "works") onCriteriaChange({ ...criteria, text: query });
  };

  const changeCollectionSelection = (selected: boolean) => {
    setCollectionSelected(selected);
    if (selected) {
      collectionListQuery.current = appliedQuery;
      setInput("");
      setAppliedQuery("");
    } else {
      setInput(collectionListQuery.current);
      setAppliedQuery(collectionListQuery.current);
    }
  };

  const refreshStatus = useCallback(async () => {
    if (!isTauri()) { setLoadingStatus(false); return; }
    try { const next = await getPcStatus(); setStatus(next); onStatusChange(next); setProblem(null); }
    catch (error) { setProblem(pcErrorMessage(error)); }
    finally { setLoadingStatus(false); }
  }, [onStatusChange]);

  useEffect(() => { void refreshStatus(); }, [refreshStatus]);
  useEffect(() => { if (isTauri()) void listOfflineWorks().then(setSavedWorks).catch(() => {}); }, [savedRevision]);
  useEffect(() => {
    const savedIds = new Set(savedWorks.map((item) => item.work.id));
    const works = results?.items.filter((item) => savedIds.has(item.id)) ?? [];
    let active = true;
    setEditHashes({});
    if (status?.reachable && works.length) {
      void Promise.allSettled(works.map(async (item) => {
        const metadata = await getPcMetadata(item.id);
        return [item.id, metadata.activeEditHash] as const;
      })).then((responses) => {
        if (!active) return;
        const next: Record<number, string | null> = {};
        for (const response of responses) {
          if (response.status === "fulfilled") next[response.value[0]] = response.value[1];
        }
        setEditHashes(next);
      });
    }
    return () => { active = false; };
  }, [results, savedWorks, status?.reachable]);

  const runSearch = useCallback(async (search: PcSearchCriteria, cursor: string | null = null, targetPage = 1) => {
    const requestId = ++requestIdRef.current;
    setBusy(true);
    setProblem(null);
    try {
      const page = await searchPcWorks(search, cursor, group, numbered ? (targetPage - 1) * pageSize : null);
      if (requestId !== requestIdRef.current || criteriaRef.current !== search || groupRef.current !== group) return;
      setResults((previous) => !numbered && cursor && previous ? { ...page, items: [...previous.items, ...page.items] } : page);
    } catch (error) {
      if (requestId !== requestIdRef.current) return;
      setProblem(pcErrorMessage(error));
      const code = typeof error === "object" && error && "code" in error ? error.code : null;
      if (code === "offline" || code === "certificate_changed" || code === "protocol_mismatch" || code === "revoked") void refreshStatus();
    } finally { if (requestId === requestIdRef.current) setBusy(false); }
  }, [refreshStatus, group, numbered]);

  useEffect(() => {
    if (tab === "works") { setInput(criteria.text); setAppliedQuery(criteria.text); }
    setResults(null);
    setCurrentPage(1);
    requestIdRef.current++;
    if (status?.reachable && tab === "works") void runSearch(criteria);
  }, [criteria, status?.reachable, runSearch, tab]);
  useEffect(() => { setTabQueries((previous) => previous.works === criteria.text ? previous : { ...previous, works: criteria.text }); }, [criteria.text]);

  // デスクトップ版の既定と同じく、一覧の続きをスクロール時に読み込む。
  useEffect(() => {
    if (numbered || !status?.reachable || tab !== "works" || !results?.nextCursor || busy || !loadMoreRef.current) return;
    const cursor = results.nextCursor;
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) {
        observer.disconnect();
        void runSearch(criteria, cursor);
      }
    }, { rootMargin: "250px" });
    observer.observe(loadMoreRef.current);
    return () => observer.disconnect();
  }, [busy, criteria, numbered, results?.nextCursor, runSearch, status?.reachable, tab]);

  const goToPage = (target: number) => {
    if (!Number.isSafeInteger(target) || target < 1 || target > 251 || target === currentPage) return;
    setCurrentPage(target);
    setResults(null);
    void runSearch(criteria, null, target);
    document.querySelector<HTMLElement>(".main-content")?.scrollTo({ top: 0, behavior: "instant" });
  };

  const estimatedPages = Math.max(1, Math.ceil((results?.totalEstimate ?? currentPage * pageSize) / pageSize));
  const maxDirectPage = Math.min(251, estimatedPages);
  const hasActiveFilter = Object.entries(defaultPcSearch).some(([key, value]) =>
    key !== "text" && !(key === "tagMode" && !criteria.tag) && !(key === "dateField" && !criteria.dateFrom && !criteria.dateTo)
    && criteria[key as keyof PcSearchCriteria] !== value);

  return <>
    {problem && <div className="inline-alert" role="alert">{problem}</div>}
    {loadingStatus ? <section className="empty-card"><p>接続状態を確認しています…</p></section> : status?.reachable ? <>
      <form className="pc-search-form pc-library-search" onSubmit={(event) => { event.preventDefault(); submitSearch(); }}><input aria-label={`${tab === "works" ? "作品" : tab === "people" ? "作者・クリエイター" : tab === "series" ? "シリーズ" : collectionSelected ? "コレクション内の作品" : "コレクション"}を検索`} type="search" placeholder={tab === "works" ? group ? `${group.name}の作品を検索` : "サーバーの作品・作者・本文を検索" : tab === "collections" && collectionSelected ? "このコレクションの作品を検索" : `${tab === "people" ? "作者・クリエイター" : tab === "series" ? "シリーズ" : "コレクション"}を検索`} value={input} onChange={(event) => setInput(event.target.value)} /><button className="primary-button" type="submit" disabled={busy}>検索</button></form>
      {tab === "works" && hasActiveFilter && <div className="active-filters" aria-label="適用中の絞り込み">{criteria.source && <span>{criteria.source === "fanbox" ? "FANBOX" : "pixiv"}</span>}{criteria.contentType && <span>種類: {{ novel: "小説", article: "記事", text: "テキスト", image: "画像", file: "ファイル" }[criteria.contentType]}</span>}{criteria.author && <span>作者: {criteria.author}</span>}{criteria.authorExclude && <span>除外作者: {criteria.authorExclude}</span>}{criteria.favorite && <button className="favorite-filter-remove" type="button" aria-label="お気に入り条件を解除" onClick={() => onCriteriaChange({ ...criteria, favorite: false })}>お気に入り ×</button>}{criteria.tag && <span>タグ: {criteria.tag}{criteria.tagMode === "or" ? "（いずれか）" : ""}</span>}{criteria.tagExclude && <span>除外タグ: {criteria.tagExclude}</span>}{(criteria.minChars || criteria.maxChars) && <span>文字数: {criteria.minChars || "0"}〜{criteria.maxChars || "上限なし"}</span>}{(criteria.minAssets || criteria.maxAssets) && <span>画像数: {criteria.minAssets || "0"}〜{criteria.maxAssets || "上限なし"}</span>}{criteria.revision && <span>{criteria.revision === "revised" ? "改稿あり" : "初版のみ"}</span>}{criteria.edit && <span>{criteria.edit === "edited" ? "編集あり" : "編集なし"}</span>}{criteria.cover && <span>{criteria.cover === "has_cover" ? "表紙あり" : "表紙なし"}</span>}{(criteria.dateFrom || criteria.dateTo) && <span>{criteria.dateField === "downloaded_at" ? "保存日" : criteria.dateField === "source_created_at" ? "公開日" : "更新日"}: {criteria.dateFrom || "指定なし"}〜{criteria.dateTo || "指定なし"}</span>}{criteria.sort !== "standard" && <span>{sortLabels[criteria.sort]}</span>}<button type="button" onClick={() => onCriteriaChange(defaultPcSearch)}>絞り込みを解除</button></div>}
      <nav className="library-tabs" aria-label="ライブラリの種類">{([ ["works", "作品"], ["people", "作者・クリエイター"], ["series", "シリーズ"], ["collections", "コレクション"] ] as const).map(([id, label]) => <button key={id} type="button" className={tab === id ? "active" : ""} aria-current={tab === id ? "page" : undefined} onClick={() => selectTab(id)}>{label}</button>)}</nav>
      {tab === "people" && <EntityBrowser kind="people" query={appliedQuery} onOpen={(entity) => openGroup("person", entity)} />}
      {tab === "series" && <EntityBrowser kind="series" query={appliedQuery} onOpen={(entity) => openGroup("series", entity)} />}
      {tab === "collections" && <CollectionBrowser query={appliedQuery} onSelectionChange={changeCollectionSelection} onOpenWork={onOpenWork} onOpenDetail={onOpenDetail} />}
      {tab === "works" && <>
      {group && <div className="group-context"><button className="group-back" onClick={() => selectTab(group.kind === "person" ? "people" : "series")}>← {group.kind === "person" ? "作者・クリエイター" : "シリーズ"}一覧</button></div>}
      <div className="section-heading pc-results-heading"><h2>{criteria.text ? `「${criteria.text}」の検索結果` : group ? `${group.name}の作品` : criteria.favorite ? "お気に入り" : "すべての作品"}</h2><span>{results?.totalEstimate != null ? `約 ${results.totalEstimate} 件` : `${results?.items.length ?? 0} 件を表示`}</span></div>
      {busy && !results ? <section className="empty-card"><p>作品を読み込んでいます…</p></section> : results?.items.length ? <VirtualWorkList items={results.items} itemKey={(work) => work.id} renderItem={(work) => {
        const saved = savedWorks.find((item) => item.work.id === work.id);
        const changed = saved && (work.currentVersion !== saved.savedVersion || work.contentHash !== saved.savedContentHash || (Object.prototype.hasOwnProperty.call(editHashes, work.id) && editHashes[work.id] !== saved.savedEditHash));
        return <WorkCard key={work.id} work={work} onRead={() => onOpenWork(work)} onDetail={() => onOpenDetail(work)} savedState={saved ? { label: changed ? "更新あり" : "保存済み", changed: Boolean(changed) } : null} />;
      }} /> : <section className="empty-card"><h2>{criteria.favorite ? "お気に入りの作品が見つかりません" : "作品が見つかりません"}</h2><p>{criteria.favorite ? "デスクトップ版のお気に入り登録と絞り込み条件を確認してください。" : "検索語や絞り込み条件を変えて試してください。"}</p></section>}
      {numbered && results && <><nav className="numbered-pager" aria-label="作品一覧のページ"><button type="button" disabled={busy || currentPage <= 1} onClick={() => goToPage(currentPage - 1)}>前へ</button><select aria-label="ページ番号" value={currentPage} onChange={(event) => goToPage(Number(event.target.value))}>{Array.from({ length: maxDirectPage }, (_, index) => <option key={index + 1} value={index + 1}>{index + 1} / {maxDirectPage} ページ</option>)}</select><button type="button" disabled={busy || currentPage >= maxDirectPage || results.items.length < pageSize} onClick={() => goToPage(currentPage + 1)}>次へ</button></nav>{estimatedPages > maxDirectPage && <p className="paging-relevance-hint">残りの作品は「スクロールで自動」で表示できます。</p>}</>}
      {!numbered && results?.nextCursor && <div className="load-more" ref={loadMoreRef}><button className="secondary-button" disabled={busy} onClick={() => void runSearch(criteria, results.nextCursor)}>{busy ? "読み込み中…" : "さらに表示"}</button></div>}
      {pagingMode === "pages" && !numbered && <p className="paging-relevance-hint">関連度順の検索は自動読み込みです。ページ番号を使うには並び順を選んでください。</p>}
      </>}
    </> : <section className="empty-card pc-connection-empty"><h2>{status?.paired ? "サーバーの本棚を開けません" : "サーバーの本棚をつなぐ"}</h2><p>{status?.issue ? pcConnectionIssue(status) : "設定でサーバーとの接続を登録すると、作品がここに並びます。"}</p><button className="primary-button" onClick={onGoToSettings}>{status?.issue === "protocol_mismatch" ? "設定で更新を確かめる" : "設定で接続を確認"}</button></section>}
  </>;
}
