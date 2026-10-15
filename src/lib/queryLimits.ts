/**
 * Infinite lists are intentionally bounded. Twenty-five normal pages keep a
 * generous scroll history (500 cards at the default page size) without
 * allowing a single long-running view to retain an entire large library.
 * Numbered paging remains available for reaching rows that have been evicted.
 */
export const INFINITE_LIST_MAX_PAGES = 25;

export const boundedInfiniteListOptions = {
  maxPages: INFINITE_LIST_MAX_PAGES,
  // The UI only walks forwards. Supplying this explicitly also documents that
  // an evicted leading page is revisited through numbered paging or a new
  // search, rather than by growing the cursor cache in both directions.
  getPreviousPageParam: () => undefined,
} as const;
