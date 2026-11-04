import { downloadTargetKey, listPageNumber, nextPagedListUrl, type PageWorkCandidate } from "@/features/browser/downloadCandidates";

export const MAX_LIST_PAGES = 100;
export const MIN_LIST_INTERVAL_SECONDS = 3;
export const MAX_LIST_INTERVAL_SECONDS = 60;

export interface ListReadOptions {
  /** 表示中のページを含めて読むページ数。1 なら次のページを開かない。 */
  pages: number;
  /** 次のページを開くまでに待つ秒数。 */
  intervalSeconds: number;
  /** 棚にある作品だけのページに着いたら止める。 */
  stopAtSaved: boolean;
}

export const DEFAULT_LIST_READ_OPTIONS: ListReadOptions = { pages: 1, intervalSeconds: 5, stopAtSaved: true };

function clamp(value: unknown, min: number, max: number, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? Math.min(max, Math.max(min, Math.round(value))) : fallback;
}

/** 保存された値や入力途中の値を、使ってよい範囲へ収める。 */
export function normalizeListReadOptions(options: Partial<ListReadOptions> | null | undefined): ListReadOptions {
  return {
    pages: clamp(options?.pages, 1, MAX_LIST_PAGES, DEFAULT_LIST_READ_OPTIONS.pages),
    intervalSeconds: clamp(options?.intervalSeconds, MIN_LIST_INTERVAL_SECONDS, MAX_LIST_INTERVAL_SECONDS, DEFAULT_LIST_READ_OPTIONS.intervalSeconds),
    stopAtSaved: typeof options?.stopAtSaved === "boolean" ? options.stopAtSaved : DEFAULT_LIST_READ_OPTIONS.stopAtSaved,
  };
}

export interface ListPage {
  url: string;
  works: PageWorkCandidate[];
  truncated: boolean;
}

/**
 * 止まった理由。complete は指定のページ数を読み終えた、saved は棚にある作品だけの
 * ページに着いた、end は新しい作品が無くなった、moved は別のページへ移った、
 * cancelled は止められた。
 */
export type ListReadStop = "complete" | "saved" | "end" | "moved" | "cancelled";

export interface ListReadDeps {
  read(): Promise<ListPage>;
  currentUrl(): Promise<string>;
  navigate(url: string): Promise<void>;
  wait(milliseconds: number): Promise<void>;
  cancelled(): boolean;
  onPage(page: ListPage, pagesRead: number, total: number): void;
}

export interface ListReadResult {
  stop: ListReadStop;
  pagesRead: number;
  total: number;
  saved: number;
  truncated: boolean;
}

const WAIT_TICK_MS = 250;
const SETTLE_POLL_MS = 1000;
const SETTLE_POLLS = 15;
/** 開いたばかりのときは、ブラウザそのものの準備も待つ。 */
const OPENED_SETTLE_POLLS = 30;
/** 同じ中身がこの回数続いたら、描き終わったとみなす。 */
const SETTLED_READS = 3;

async function waitOrCancel(milliseconds: number, deps: ListReadDeps): Promise<boolean> {
  for (let left = milliseconds; left > 0; left -= WAIT_TICK_MS) {
    if (deps.cancelled()) return false;
    await deps.wait(Math.min(WAIT_TICK_MS, left));
  }
  return !deps.cancelled();
}

type Settled = { kind: "page"; page: ListPage } | { kind: "empty" } | { kind: "moved" } | { kind: "cancelled" };

/**
 * 開いたページが描き終わるのを待つ。読むのは文書だけなので、待つあいだの通信は
 * 増えない。取得元はどれも SPA で、URL が変わった直後は一覧がまだ無い。
 */
async function settle(arrival: (url: string) => "arrived" | "waiting" | "moved", polls: number, deps: ListReadDeps): Promise<Settled> {
  let last = "";
  let repeats = 0;
  let arrived = false;
  for (let poll = 0; poll < polls; poll += 1) {
    if (!(await waitOrCancel(SETTLE_POLL_MS, deps))) return { kind: "cancelled" };
    const page = await deps.read().catch(() => null);
    if (deps.cancelled()) return { kind: "cancelled" };
    if (!page) continue;
    const state = arrival(page.url);
    if (state === "moved") return { kind: "moved" };
    if (state === "waiting") continue;
    arrived = true;
    if (!page.works.length) continue;
    const ids = page.works.map((work) => work.sourceId).join(",");
    repeats = ids === last ? repeats + 1 : 1;
    last = ids;
    if (repeats >= SETTLED_READS) return { kind: "page", page };
  }
  return arrived ? { kind: "empty" } : { kind: "moved" };
}

/**
 * 表示中のページから順に読む。
 *
 * 次のページを開くのは取得元への通信なので、指定のページ数、間隔、止める条件に
 * 収める。ページ数が 1 なら通信は増えない。開く前に、見ているページが変わって
 * いないことを確かめる。利用者が別のページへ移ったのなら、連れ戻さずに止める。
 *
 * `opened` は一覧を開いたばかりのとき。描き終わるのを待ち、続きを開ける一覧に
 * 着かなければ（ログインの画面やトップへ飛ばされたら）何も読まない。
 */
export async function readListPages(options: ListReadOptions, deps: ListReadDeps, opened = false): Promise<ListReadResult> {
  const seen = new Set<string>();
  const savedIds = new Set<string>();
  let truncated = false;
  let pagesRead = 0;
  const finish = (stop: ListReadStop): ListReadResult => ({ stop, pagesRead, total: seen.size, saved: savedIds.size, truncated });
  if (deps.cancelled()) return finish("cancelled");
  let page: ListPage;
  if (opened) {
    const first = await settle((url) => nextPagedListUrl(url) ? "arrived" : "waiting", OPENED_SETTLE_POLLS, deps);
    if (first.kind === "cancelled") return finish("cancelled");
    if (first.kind === "moved") return finish("moved");
    if (first.kind === "empty") return finish("end");
    page = first.page;
  } else {
    page = await deps.read();
  }
  const base = page.url;
  for (;;) {
    if (deps.cancelled()) return finish("cancelled");
    if (!page.works.length) return finish("end");
    if (pagesRead > 0 && page.works.every((work) => seen.has(work.sourceId))) return finish("end");
    for (const work of page.works) {
      seen.add(work.sourceId);
      if (work.saved) savedIds.add(work.sourceId);
    }
    truncated ||= page.truncated;
    pagesRead += 1;
    deps.onPage(page, pagesRead, seen.size);
    if (pagesRead >= options.pages) return finish("complete");
    if (options.stopAtSaved && page.works.every((work) => work.saved)) return finish("saved");
    const nextUrl = nextPagedListUrl(page.url);
    if (!nextUrl) return finish("complete");
    if (!(await waitOrCancel(options.intervalSeconds * 1000, deps))) return finish("cancelled");
    const here = await deps.currentUrl().catch(() => "");
    if (deps.cancelled()) return finish("cancelled");
    if (downloadTargetKey(here) !== downloadTargetKey(base) || listPageNumber(here) !== listPageNumber(page.url)) return finish("moved");
    await deps.navigate(nextUrl);
    const nextPage = listPageNumber(nextUrl);
    const settled = await settle(
      (url) => downloadTargetKey(url) !== downloadTargetKey(base) ? "moved" : listPageNumber(url) === nextPage ? "arrived" : "waiting",
      SETTLE_POLLS,
      deps,
    );
    if (settled.kind === "cancelled") return finish("cancelled");
    if (settled.kind === "moved") return finish("moved");
    if (settled.kind === "empty") return finish("end");
    page = settled.page;
  }
}
