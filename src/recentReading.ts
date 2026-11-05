import { useEffect, useState } from "react";
import type { PcWork } from "./pcApi";

export interface RecentWork {
  work: PcWork;
  at: number;
  /** 0 から 1。 */
  progress: number;
  finished: boolean;
}

const storageKey = "piep.android.reading.recent.v1";
const changedEvent = "piep:recent-reading-changed";
const maxEntries = 120;
const writeIntervalMs = 4000;

function read(): RecentWork[] {
  try {
    const raw = JSON.parse(window.localStorage.getItem(storageKey) ?? "[]") as unknown;
    if (!Array.isArray(raw)) return [];
    return raw.filter((entry): entry is RecentWork => Boolean(entry?.work?.id) && Number.isFinite(entry.at) && Number.isFinite(entry.progress));
  } catch { return []; }
}

let lastWrite = { id: 0, at: 0 };
let trailing: number | undefined;

function write(work: PcWork, progress: number, finished: boolean, now: number): void {
  const entries = read();
  if (!entries.some((entry) => entry.work.id === work.id) && !finished && progress <= 0.002) return;
  lastWrite = { id: work.id, at: now };
  const next = [{ work, at: now, progress, finished }, ...entries.filter((entry) => entry.work.id !== work.id)].slice(0, maxEntries);
  try { window.localStorage.setItem(storageKey, JSON.stringify(next)); } catch { return; }
  window.dispatchEvent(new CustomEvent(changedEvent));
}

/**
 * 読みかけの棚の材料。サーバーの検索は「読みかけ」で絞れず、読書位置からは作品を引けないので、
 * この端末で読んだ作品を表示に要る情報ごと覚える。開いただけでは棚に並べない。
 * スクロールのたびには書かず、止まったところの値を少し遅れて残す。読み終えた瞬間は待たない。
 */
export function rememberReading(work: PcWork, progress: number, finished: boolean): void {
  const now = Date.now();
  window.clearTimeout(trailing);
  const wait = lastWrite.id === work.id ? writeIntervalMs - (now - lastWrite.at) : 0;
  if (finished || wait <= 0) { write(work, progress, finished, now); return; }
  trailing = window.setTimeout(() => write(work, progress, finished, Date.now()), wait);
}

/** 読み終えたものは棚から外す。新しく読んだ順。 */
export function useReadingShelf(): RecentWork[] {
  const [items, setItems] = useState(() => read().filter((entry) => !entry.finished));
  useEffect(() => {
    const update = () => setItems(read().filter((entry) => !entry.finished));
    window.addEventListener(changedEvent, update);
    return () => window.removeEventListener(changedEvent, update);
  }, []);
  return items;
}
