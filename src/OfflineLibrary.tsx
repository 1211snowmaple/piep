import { useEffect, useState } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { ChevronLeft } from "lucide-react";
import { SearchField } from "./SearchField";
import { WorkThumbnail } from "./WorkThumbnail";
import { WorkCard } from "./WorkCard";
import { VirtualWorkList } from "./VirtualWorkList";
import { ProviderMark } from "./ProviderMark";
import { CollectionArtwork } from "./CollectionArtwork";
import { GroupArtwork } from "./LibraryGroups";
import { savedEntityCoverUrl } from "./images";
import { useAppNavigation, type OfflinePageState, type OfflineTab } from "./appHistory";
import { SortMenu } from "./SortMenu";
import { sortOfflineWorks, type FixedSort } from "./librarySort";
import { deleteOfflineIssue, listOfflineIssues, listOfflineWorks, type OfflineIssue, type OfflineWork, type PcCollection } from "./pcApi";

type SavedGroup = { id: string; name: string; works: OfflineWork[]; cover: OfflineWork; coverWorkId?: number | null; collection?: PcCollection };
const emptySearches: Record<OfflineTab, string> = { works: "", people: "", series: "", collections: "" };
// 戻ったときに「確認しています」を挟まない。中身は裏で確かめ直す。
let lastWorks: OfflineWork[] | null = null;

function hasRepresentativeCover(item: OfflineWork, kind: "people" | "series", groupId: string): boolean {
  const work = item.work;
  const hasCover = kind === "people" ? item.hasAuthorCover : item.hasSeriesCover;
  if (!hasCover) return false;
  const savedEntity = kind === "people" ? item.authorCoverEntity : item.seriesCoverEntity;
  const source = savedEntity?.source ?? work.source;
  const key = savedEntity?.key ?? (kind === "people" ? work.authorId : work.seriesId);
  return Boolean(key) && groupId === `${source}:${key}`;
}

function savedGroups(items: OfflineWork[], kind: Exclude<OfflineTab, "works">): SavedGroup[] {
  const groups = new Map<string, SavedGroup>();
  for (const item of items) {
    const work = item.work;
    const entries: { id: string; name: string; coverWorkId?: number | null; collection?: PcCollection }[] = kind === "people"
      ? item.people?.length
        ? item.people.map((person) => ({ id: `${person.source}:${person.key}`, name: person.displayName }))
        : [{ id: `${work.source}:${work.authorId || work.authorName}`, name: work.authorName }]
      : kind === "series"
        ? item.series?.length
          ? item.series.map((series) => ({ id: `${series.source}:${series.key}`, name: series.title }))
          : work.seriesTitle ? [{ id: `${work.source}:${work.seriesId || work.seriesTitle}`, name: work.seriesTitle }] : []
        : (item.collections ?? []).map((collection) => ({ id: collection.id, name: collection.name, coverWorkId: collection.coverWorkId, collection }));
    const seen = new Set<string>();
    for (const entry of entries) {
      if (seen.has(entry.id)) continue;
      seen.add(entry.id);
      const existing = groups.get(entry.id);
      if (existing) {
        existing.works.push(item);
        if (entry.collection && (entry.collection.coverRevision ?? 0) > (existing.collection?.coverRevision ?? 0)) {
          existing.collection = entry.collection;
          existing.coverWorkId = entry.coverWorkId;
        }
        if (kind !== "collections" && !hasRepresentativeCover(existing.cover, kind, entry.id)
          && hasRepresentativeCover(item, kind, entry.id)) existing.cover = item;
      }
      else groups.set(entry.id, { ...entry, works: [item], cover: item });
    }
  }
  if (kind === "series") {
    for (const group of groups.values()) {
      group.works.sort((left, right) => {
        const a = left.series?.find((series) => `${series.source}:${series.key}` === group.id)?.contentOrder;
        const b = right.series?.find((series) => `${series.source}:${series.key}` === group.id)?.contentOrder;
        if (a != null && b != null && a !== b) return a - b;
        if (a != null && b == null) return -1;
        if (a == null && b != null) return 1;
        return right.savedAtMs - left.savedAtMs;
      });
    }
  }
  if (kind === "collections") {
    for (const group of groups.values()) {
      group.works.sort((left, right) => {
        const a = left.collectionPositions?.[group.id];
        const b = right.collectionPositions?.[group.id];
        if (a !== undefined && b !== undefined && a !== b) return a - b;
        if (a !== undefined && b === undefined) return -1;
        if (a === undefined && b !== undefined) return 1;
        return right.savedAtMs - left.savedAtMs;
      });
      group.cover = group.works.find((item) => item.work.id === group.coverWorkId)
        ?? group.works.find((item) => item.work.hasCover)
        ?? group.works[0];
    }
  }
  return [...groups.values()].sort((a, b) => a.name.localeCompare(b.name, "ja"));
}

function SavedGroupArtwork({ group, kind }: { group: SavedGroup; kind: "people" | "series" }) {
  const hasImage = hasRepresentativeCover(group.cover, kind, group.id);
  return <GroupArtwork kind={kind} src={hasImage ? savedEntityCoverUrl(group.cover.work.id, kind, group.cover.savedAtMs) : null} />;
}

export function OfflineLibrary({ page, onOpenWork, onOpenDetail, onShowPc, savedRevision, pagingMode, rememberedSort, onRememberedSortChange }: {
  page: OfflinePageState;
  onOpenWork: (work: OfflineWork) => void;
  onOpenDetail: (work: OfflineWork) => void;
  onShowPc: () => void;
  savedRevision: number;
  pagingMode: "auto" | "pages";
  rememberedSort: FixedSort;
  onRememberedSortChange: (sort: FixedSort) => void;
}) {
  const navigation = useAppNavigation();
  const { tab, groupId: selectedGroup } = page;
  const [works, setWorks] = useState<OfflineWork[]>(lastWorks ?? []);
  const [issues, setIssues] = useState<OfflineIssue[]>([]);
  const [loading, setLoading] = useState(lastWorks === null);
  const [problem, setProblem] = useState<string | null>(null);
  const [issuesProblem, setIssuesProblem] = useState<string | null>(null);
  const [loadRevision, setLoadRevision] = useState(0);
  const [deletingIssueId, setDeletingIssueId] = useState<number | null>(null);
  const [searchInputs, setSearchInputs] = useState<Record<OfflineTab, string>>(emptySearches);
  const [queries, setQueries] = useState<Record<OfflineTab, string>>(emptySearches);
  const [groupSearchInput, setGroupSearchInput] = useState("");
  const [groupQuery, setGroupQuery] = useState("");
  const [currentPage, setCurrentPage] = useState(1);

  useEffect(() => {
    if (!isTauri()) { setLoading(false); return; }
    let alive = true;
    setLoading(lastWorks === null);
    setProblem(null);
    setIssuesProblem(null);
    void Promise.allSettled([listOfflineWorks(), listOfflineIssues()])
      .then(([worksResult, issuesResult]) => {
        if (!alive) return;
        if (worksResult.status === "fulfilled") { lastWorks = worksResult.value; setWorks(worksResult.value); }
        else setProblem("保存した作品一覧を読み込めませんでした。保存状態を確認して、もう一度お試しください。");
        if (issuesResult.status === "fulfilled") setIssues(issuesResult.value);
        else setIssuesProblem("読み込めない保存データを確認できませんでした。もう一度確認してください。");
      })
      .finally(() => { if (alive) setLoading(false); });
    return () => { alive = false; };
  }, [savedRevision, loadRevision]);

  const removeUnreadableWork = async (workId: number) => {
    setDeletingIssueId(workId);
    setIssuesProblem(null);
    try {
      await deleteOfflineIssue(workId);
      setIssues((current) => current.filter((issue) => issue.workId !== workId));
      setLoadRevision((revision) => revision + 1);
    } catch (error) {
      const code = typeof error === "object" && error && "code" in error ? error.code : null;
      if (code === "offline_work_recovered") {
        setIssuesProblem("作品は保存し直されています。一覧を更新しました。");
        setLoadRevision((revision) => revision + 1);
      } else {
        setIssuesProblem("保存データを削除できませんでした。もう一度お試しください。");
      }
    } finally {
      setDeletingIssueId(null);
    }
  };

  const groups = tab === "works" ? [] : savedGroups(works, tab);
  const selected = groups.find((group) => group.id === selectedGroup);
  const searchInput = selected ? groupSearchInput : searchInputs[tab];
  const query = selected ? groupQuery : queries[tab];
  const needle = query.toLocaleLowerCase();
  const matchingGroups = needle ? groups.filter((group) => group.name.toLocaleLowerCase().includes(needle)) : groups;
  // シリーズやコレクションの中は、そのグループ自身の順を保つ。
  const allVisibleWorks = tab === "works" ? sortOfflineWorks(works, rememberedSort) : selected?.works ?? [];
  const visibleWorks = needle ? allVisibleWorks.filter((item) => `${item.work.title} ${item.work.authorName} ${item.work.seriesTitle ?? ""} ${item.work.tags.join(" ")}`.toLocaleLowerCase().includes(needle)) : allVisibleWorks;
  const pageCount = Math.max(1, Math.ceil(visibleWorks.length / 20));
  const pagedWorks = pagingMode === "pages" ? visibleWorks.slice((currentPage - 1) * 20, currentPage * 20) : visibleWorks;
  const tabLabel = { works: "作品", people: "作者・クリエイター", series: "シリーズ", collections: "コレクション" };
  const searchTarget = selected ? "このグループの作品・タグ" : {
    works: "保存作品・作者・タグ",
    people: "保存した作者・クリエイター",
    series: "保存したシリーズ",
    collections: "保存したコレクション",
  }[tab];

  const submitSearch = (next: string) => {
    if (selected) setGroupQuery(next);
    else setQueries((previous) => ({ ...previous, [tab]: next }));
  };

  useEffect(() => { setCurrentPage(1); }, [query, tab, selectedGroup, pagingMode]);

  return <>
    <SearchField value={searchInput} label={`${searchTarget}を検索`} placeholder={`${searchTarget}を検索`} onChange={(next) => { if (selected) setGroupSearchInput(next); else setSearchInputs((previous) => ({ ...previous, [tab]: next })); }} onSubmit={submitSearch}>{tab === "works" && <SortMenu current={rememberedSort} onSelect={(sort) => { if (sort !== "standard") onRememberedSortChange(sort); }} />}</SearchField>
    <div className="offline-library-heading"><strong>保存した作品</strong><span>{loading ? "確認中…" : problem || issuesProblem ? "保存一覧を確認できません" : issues.length ? "読み込めない保存データがあります" : `${works.length} 作品`}</span></div>
    {problem && <div className="inline-alert" role="alert">{problem}</div>}
    {problem && <button className="secondary-button offline-retry" type="button" onClick={() => setLoadRevision((revision) => revision + 1)} disabled={loading}>{loading ? "確認中…" : "もう一度確認"}</button>}
    {(issues.length > 0 || issuesProblem) && <section className="offline-issues" aria-labelledby="offline-issues-title">
      <h2 id="offline-issues-title">読み込めない保存データ</h2>
      <p>本文を読み込めない保存データがあります。不要なものは端末から削除できます。</p>
      {issuesProblem && <p className="offline-issues-error" role="alert">{issuesProblem}</p>}
      {issues.map((issue) => <div className="offline-issue-row" key={issue.workId}><span>作品番号 {issue.workId} の保存データを読み込めません</span><button className="secondary-button" type="button" disabled={deletingIssueId === issue.workId} onClick={() => void removeUnreadableWork(issue.workId)}>{deletingIssueId === issue.workId ? "削除中…" : "このデータを削除"}</button></div>)}
      {issuesProblem && <button className="secondary-button offline-retry" type="button" onClick={() => setLoadRevision((revision) => revision + 1)} disabled={loading}>{loading ? "確認中…" : "一覧を再確認"}</button>}
    </section>}
    <nav className="library-tabs" aria-label="端末内のライブラリの種類">{(["works", "people", "series", "collections"] as const).map((id) => <button key={id} type="button" className={tab === id ? "active" : ""} aria-current={tab === id ? "page" : undefined} onClick={() => navigation.push({ page: { name: "offline", tab: id, groupId: null } })}>{tabLabel[id]}</button>)}</nav>
    {loading ? <section className="empty-card"><p>保存した作品を確認しています…</p></section> : <>
      {selected && <div className="group-context"><button className="page-back" type="button" onClick={navigation.back}><ChevronLeft aria-hidden="true" />戻る</button><strong>{selected.name}</strong><span>端末に保存した {selected.works.length} 作品</span></div>}
      {tab !== "works" && !selected && <div className="group-grid offline-group-grid">{matchingGroups.map((group) => <button className="group-card" key={group.id} onClick={() => { setGroupSearchInput(""); setGroupQuery(""); navigation.push({ page: { name: "offline", tab, groupId: group.id } }); }}>
        {tab === "collections" ? <span className="offline-group-art">{group.collection?.coverMode ? <CollectionArtwork collection={group.collection} offlineWorks={group.works} /> : <WorkThumbnail work={group.cover.work} offline snapshotGeneration={group.cover.savedAtMs} />}</span> : <SavedGroupArtwork group={group} kind={tab} />}
        <span className="group-card-text"><strong>{group.name}</strong><small className="group-card-meta">{tab !== "collections" && <ProviderMark source={group.cover.work.source} compact />}<span>端末に保存した {group.works.length} 作品</span></small></span>
      </button>)}</div>}
      {(tab === "works" || selected) && visibleWorks.length > 0 && <VirtualWorkList listKey={`offline:${tab}:${selectedGroup ?? ""}:${query}:${rememberedSort}:${currentPage}`} items={pagedWorks} itemKey={(item) => item.work.id} renderItem={(item) => <div className="offline-row">
        <WorkCard work={item.work} offline offlineAuthorCover={item.hasAuthorCover} snapshotGeneration={item.savedAtMs} onRead={() => onOpenWork(item)} onDetail={() => onOpenDetail(item)} prefix={`${item.pageCount} ページ`} />
      </div>} />}
      {pagingMode === "pages" && (tab === "works" || selected) && visibleWorks.length > 20 && <nav className="numbered-pager" aria-label="端末内作品のページ"><button type="button" disabled={currentPage <= 1} onClick={() => setCurrentPage((page) => page - 1)}>前へ</button><select aria-label="端末内のページ番号" value={currentPage} onChange={(event) => setCurrentPage(Number(event.target.value))}>{Array.from({ length: pageCount }, (_, index) => <option key={index + 1} value={index + 1}>{index + 1} / {pageCount} ページ</option>)}</select><button type="button" disabled={currentPage >= pageCount} onClick={() => setCurrentPage((page) => page + 1)}>次へ</button></nav>}
      {!works.length && !issues.length && !problem && !issuesProblem && <section className="empty-card"><h2>保存した作品はまだありません</h2><p>サーバーの本棚で作品を開き、「端末に保存」を押すと、ここでオフラインでも読めます。</p><button className="primary-button" onClick={onShowPc}>サーバーの本棚を見る</button></section>}
      {works.length > 0 && tab !== "works" && !selected && !matchingGroups.length && <section className="empty-card"><h2>{query ? "該当する項目はありません" : `${tabLabel[tab]}はまだありません`}</h2><p>{query ? "検索語を変えて試してください。" : "対応する作品を保存すると、ここにまとまって表示されます。以前に保存した作品のコレクションは、サーバーに接続して保存し直すと反映されます。"}</p></section>}
      {works.length > 0 && (tab === "works" || selected) && !visibleWorks.length && <section className="empty-card"><h2>作品が見つかりません</h2><p>検索語を変えて試してください。</p></section>}
    </>}
  </>;
}
