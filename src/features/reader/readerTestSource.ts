import type { ReaderSource } from "@/features/reader/readerSource";
import type { ReaderContentPage, ReaderMetadata, ReaderOutlineEntry, ReaderSearchHit } from "@/types/library";

/**
 * 試験用の読む画面の読み先。見本の作品の形のまま、本文・目次・本文内検索を渡した関数から取る。
 * 位置の戻し方・送り方・検索の印など、読み先によらない画面の振る舞いを確かめる。
 */
export interface ReaderTestApi {
  getReaderMetadata: (id: number) => Promise<ReaderMetadata>;
  getReaderContentPage: (id: number, version: number | null, page: number) => Promise<ReaderContentPage>;
  getReaderOutline?: (id: number, version: number | null) => Promise<ReaderOutlineEntry[]>;
  searchReaderContent?: (id: number, term: string, version: number | null) => Promise<ReaderSearchHit[]>;
}

/** `base` は見本の読み先（試験が差し替える前の `previewReaderSource` の返すもの）。 */
export function readerTestSource(base: ReaderSource, api: ReaderTestApi): ReaderSource {
  const id = Number(base.key);
  return {
    ...base,
    head: { ...base.head, queryFn: () => api.getReaderMetadata(id) },
    page: (version, page) => api.getReaderContentPage(id, version, page),
    outline: (version) => api.getReaderOutline?.(id, version) ?? Promise.resolve([]),
    search: (term, version) => api.searchReaderContent?.(id, term, version) ?? Promise.resolve([]),
  };
}
