import { getPcPage, type PcPage } from "./pcApi";

const maxPages = 16;
const maxAgeMs = 10 * 60_000;

type Entry = { workId: number; at: number; page: Promise<PcPage> };

const entries = new Map<string, Entry>();

/**
 * オンラインの本文を少しだけ覚える。ページを戻るとき、先に受け取った次のページを開くときに
 * 通信を待たない。作品を端末へ残す「保存」とは別の、メモリ上だけの短い控えである。
 * 版の違いは本文に添えられた版で呼び出し側が確かめる。
 */
export function fetchPage(workId: number, page: number, version: number | null = null): Promise<PcPage> {
  const key = `${workId}:${version ?? "latest"}:${page}`;
  const found = entries.get(key);
  if (found && Date.now() - found.at < maxAgeMs) {
    entries.delete(key);
    entries.set(key, found);
    return found.page;
  }
  const entry: Entry = { workId, at: Date.now(), page: getPcPage(workId, page, version) };
  entries.delete(key);
  entries.set(key, entry);
  // 失敗を覚えない。次に開いたときに取り直す。
  entry.page.catch(() => { if (entries.get(key) === entry) entries.delete(key); });
  while (entries.size > maxPages) entries.delete(entries.keys().next().value!);
  return entry.page;
}

export function prefetchPage(workId: number, page: number, version: number | null = null): void {
  fetchPage(workId, page, version).catch(() => { /* 読みに来たときにもう一度試す */ });
}

export function forgetWorkPages(workId: number): void {
  for (const [key, entry] of entries) if (entry.workId === workId) entries.delete(key);
}

/** 接続先が変わったら、前のサーバーの本文を残さない。 */
export function forgetAllPages(): void {
  entries.clear();
}
