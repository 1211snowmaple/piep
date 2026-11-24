import { contentTypeLabel } from "@shared/format";
import { ENTITY_SORT_OPTIONS, RELEVANCE_SORT_OPTION, SERIES_ORDER_SORT_OPTIONS, WORK_SORT_OPTIONS, sortLabel, type SortChoice } from "@shared/sortOptions";
import { defaultPcSearch, type FixedSort, type OfflineWork, type PcSearchCriteria, type PcSort, type PcWorkSort, joinSearchTerms } from "./pcApi";

export type { FixedSort } from "./pcApi";
export type MenuGroup<T extends string> = { label: string; items: { value: T; label: string }[] };

/** 携帯で選べる順。値と呼び名はデスクトップと共通の定義（`@shared/sortOptions`）のまま。 */
const fixedSorts: FixedSort[] = [
  "saved:desc", "saved:asc", "published:desc", "published:asc",
  "title:asc", "title:desc", "author:asc", "author:desc",
  "length:desc", "length:asc", "media:desc", "media:asc", "version:desc", "version:asc",
];

export const sortLabels = Object.fromEntries(
  fixedSorts.map((sort) => [sort, sortLabel(WORK_SORT_OPTIONS, sort as SortChoice)]),
) as Record<FixedSort, string>;

export const relevanceLabel = RELEVANCE_SORT_OPTION.label;

export const sortGroups: { label: string; items: FixedSort[] }[] = [
  { label: "日付", items: ["saved:desc", "saved:asc", "published:desc", "published:asc"] },
  { label: "名前", items: ["title:asc", "title:desc", "author:asc", "author:desc"] },
  { label: "内容", items: ["length:desc", "length:asc", "media:desc", "media:asc", "version:desc", "version:asc"] },
];

const defaultSort: FixedSort = "saved:desc";
const rememberedKey = "piep.android.library.sort";
const savedConditionsKey = "piep.android.library.saved-conditions";

export function loadRememberedSort(): FixedSort {
  try {
    const value = window.localStorage.getItem(rememberedKey);
    return value && value in sortLabels ? value as FixedSort : defaultSort;
  } catch { return defaultSort; }
}

export function saveRememberedSort(sort: FixedSort): void {
  try { window.localStorage.setItem(rememberedKey, sort); } catch { /* storage unavailable */ }
}

/**
 * 実際に送る順を決める。条件に順が入っていれば、その条件の間だけ優先する。
 * 関連度は検索語がなければ意味がないので、いつもの順に戻す。
 */
export function effectiveSort(criteria: PcSearchCriteria, remembered: FixedSort): PcSort {
  if (criteria.sort === "remembered") return remembered;
  if (criteria.sort === "standard" && !criteria.text.trim()) return remembered;
  return criteria.sort;
}

function time(value: string | null | undefined): number {
  const parsed = value ? Date.parse(value) : NaN;
  return Number.isNaN(parsed) ? 0 : parsed;
}

function offlineCompare(sort: FixedSort): (left: OfflineWork, right: OfflineWork) => number {
  const saved = (item: OfflineWork) => time(item.work.savedAt) || item.savedAtMs;
  const published = (item: OfflineWork) => time(item.work.sourceCreatedAt);
  const text = (a: string, b: string) => a.localeCompare(b, "ja");
  switch (sort) {
    case "saved:desc": return (a, b) => saved(b) - saved(a);
    case "saved:asc": return (a, b) => saved(a) - saved(b);
    case "published:desc": return (a, b) => published(b) - published(a);
    case "published:asc": return (a, b) => published(a) - published(b);
    case "title:asc": return (a, b) => text(a.work.title, b.work.title);
    case "title:desc": return (a, b) => text(b.work.title, a.work.title);
    case "author:asc": return (a, b) => text(a.work.authorName, b.work.authorName);
    case "author:desc": return (a, b) => text(b.work.authorName, a.work.authorName);
    case "length:desc": return (a, b) => b.work.textLength - a.work.textLength;
    case "length:asc": return (a, b) => a.work.textLength - b.work.textLength;
    case "media:desc": return (a, b) => b.work.assetCount - a.work.assetCount;
    case "media:asc": return (a, b) => a.work.assetCount - b.work.assetCount;
    case "version:desc": return (a, b) => b.work.currentVersion - a.work.currentVersion;
    case "version:asc": return (a, b) => a.work.currentVersion - b.work.currentVersion;
  }
}

/** 端末内の一覧はサーバーを通らないので、同じ順を手元で並べる。 */
export function sortOfflineWorks(items: OfflineWork[], sort: FixedSort): OfflineWork[] {
  return [...items].sort(offlineCompare(sort));
}

export interface SavedCondition {
  id: string;
  name: string;
  criteria: PcSearchCriteria;
}

export function loadSavedConditions(): SavedCondition[] {
  try {
    const raw = JSON.parse(window.localStorage.getItem(savedConditionsKey) ?? "[]");
    if (!Array.isArray(raw)) return [];
    return raw.filter((item) => typeof item?.id === "string" && typeof item?.name === "string" && typeof item?.criteria === "object" && item.criteria)
      .map((item) => ({ id: item.id, name: item.name, criteria: { ...defaultPcSearch, ...item.criteria } }));
  } catch { return []; }
}

export function saveSavedConditions(conditions: SavedCondition[]): void {
  try { window.localStorage.setItem(savedConditionsKey, JSON.stringify(conditions)); } catch { /* storage unavailable */ }
}

/** 名前を入れなかったときの呼び名。何の条件か一目で分かるものを並べる。 */
export function describeCondition(criteria: PcSearchCriteria): string {
  const parts: string[] = [];
  if (criteria.text) parts.push(`「${criteria.text}」`);
  if (criteria.source) parts.push(criteria.source === "fanbox" ? "FANBOX" : "pixiv");
  if (criteria.contentType) parts.push(contentTypeLabel(criteria.contentType));
  if (criteria.author.length) parts.push(joinSearchTerms(criteria.author));
  for (const tag of criteria.tag) parts.push(`#${tag}`);
  if (criteria.favorite) parts.push("お気に入り");
  if (criteria.sort !== "remembered" && criteria.sort !== "standard") parts.push(sortLabels[criteria.sort]);
  return parts.join("・") || "保存した条件";
}

export function hasConditionToSave(criteria: PcSearchCriteria): boolean {
  return Object.entries(defaultPcSearch).some(([key, value]) =>
    !(key === "tagMode" && !criteria.tag.length) && !(key === "dateField" && !criteria.dateFrom && !criteria.dateTo)
    && JSON.stringify(criteria[key as keyof PcSearchCriteria]) !== JSON.stringify(value));
}

// ---- 作者・シリーズの一覧の順。デスクトップ版の ENTITY_SORT_OPTIONS と同じ項目を持つ。

export type EntitySortField = "work_count" | "saved" | "published" | "updated" | "name" | "length" | "size" | "media" | "version";
export type EntitySort = `${EntitySortField}:${"asc" | "desc"}`;

const entityItems = (...sorts: EntitySort[]) => sorts.map((value) => ({ value, label: sortLabel(ENTITY_SORT_OPTIONS, value) }));

export const entitySortGroups: MenuGroup<EntitySort>[] = [
  { label: "作品数", items: entityItems("work_count:desc", "work_count:asc") },
  { label: "日付", items: entityItems("saved:desc", "saved:asc", "published:desc", "published:asc", "updated:desc", "updated:asc") },
  { label: "名前", items: entityItems("name:asc", "name:desc") },
  { label: "合計", items: entityItems("length:desc", "length:asc", "size:desc", "size:asc", "media:desc", "media:asc", "version:desc", "version:asc") },
];

const entitySortValues = new Map(entitySortGroups.flatMap((group) => group.items).map((item) => [item.value as string, item.label]));
export const entitySeriesKey = "entity-series";
const entitySortKey = (kind: string) => `piep.android.library-entity-sort.${kind}`;

export function entitySortLabel(sort: EntitySort): string {
  return entitySortValues.get(sort) ?? sort;
}

export function splitEntitySort(sort: EntitySort): { sort: string; order: string } {
  const [field, order] = sort.split(":");
  return { sort: field, order };
}

export function loadEntitySort(kind: "people" | "series" | typeof entitySeriesKey): EntitySort {
  const fallback: EntitySort = kind === entitySeriesKey ? "published:desc" : "work_count:desc";
  try {
    const value = window.localStorage.getItem(entitySortKey(kind));
    return value && entitySortValues.has(value) ? value as EntitySort : fallback;
  } catch { return fallback; }
}

export function saveEntitySort(kind: "people" | "series" | typeof entitySeriesKey, sort: EntitySort): void {
  try { window.localStorage.setItem(entitySortKey(kind), sort); } catch { /* storage unavailable */ }
}

// ---- 作者・シリーズの中の作品の順。

export type EntityWorkSort = FixedSort | "series_order:asc" | "series_order:desc";
const seriesOrderLabels = { "series_order:asc": sortLabel(SERIES_ORDER_SORT_OPTIONS, "series_order:asc"), "series_order:desc": sortLabel(SERIES_ORDER_SORT_OPTIONS, "series_order:desc") };
const entityWorkSortKey = (kind: string) => `piep.android.entity-work-sort.${kind}`;

export function entityWorkSortLabel(sort: EntityWorkSort): string {
  return sort === "series_order:asc" || sort === "series_order:desc" ? seriesOrderLabels[sort] : sortLabels[sort];
}

/**
 * 作者の中で作者名の順は意味がないので外す。シリーズの既定は読む順、作者の既定は公開日。
 * 関連度は検索語があるときだけ出す。
 */
export function entityWorkSortGroups(kind: "person" | "series", hasQuery: boolean): MenuGroup<PcWorkSort>[] {
  const groups: MenuGroup<PcWorkSort>[] = [];
  if (hasQuery) groups.push({ label: "この検索だけ", items: [{ value: "standard", label: relevanceLabel }] });
  if (kind === "series") groups.push({ label: "シリーズ内順", items: [{ value: "series_order:asc", label: seriesOrderLabels["series_order:asc"] }, { value: "series_order:desc", label: seriesOrderLabels["series_order:desc"] }] });
  for (const group of sortGroups) {
    const items = group.items.filter((sort) => sort !== "author:asc" && sort !== "author:desc");
    groups.push({ label: group.label, items: items.map((sort) => ({ value: sort, label: sortLabels[sort] })) });
  }
  return groups;
}

export function loadEntityWorkSort(kind: "person" | "series"): EntityWorkSort {
  const fallback: EntityWorkSort = kind === "series" ? "series_order:asc" : "published:desc";
  try {
    const value = window.localStorage.getItem(entityWorkSortKey(kind));
    if (value === "series_order:asc" || value === "series_order:desc") return kind === "series" ? value : fallback;
    if (value === "author:asc" || value === "author:desc") return fallback;
    return value && value in sortLabels ? value as FixedSort : fallback;
  } catch { return fallback; }
}

export function saveEntityWorkSort(kind: "person" | "series", sort: EntityWorkSort): void {
  try { window.localStorage.setItem(entityWorkSortKey(kind), sort); } catch { /* storage unavailable */ }
}
