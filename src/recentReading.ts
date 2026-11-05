import { useEffect, useState } from "react";
import type { PcWork } from "./pcApi";

export interface RecentWork {
  work: PcWork;
  at: number;
  /** 0 から 1。 */
  progress: number;
  finished: boolean;
  /** 読んだときの接続先。作品の番号はサーバーごとに振られるので、別のサーバーの棚に混ぜない。 */
  server?: string;
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

// まだ接続先が分からないあいだは棚を出さない。
let currentServer: string | null = null;

/** 接続先が変わったら呼ぶ。接続先を記録していない古い履歴は、最初に分かった接続先のものとして扱う。 */
export function setReadingServer(server: string | null): void {
  if (server === currentServer) return;
  currentServer = server;
  lastWrite = { id: 0, at: 0 };
  window.clearTimeout(trailing);
  window.dispatchEvent(new CustomEvent(changedEvent));
}

const fromThisServer = (entry: RecentWork) => currentServer !== null && (entry.server === undefined || entry.server === currentServer);

let lastWrite = { id: 0, at: 0 };
let trailing: number | undefined;

function write(work: PcWork, progress: number, finished: boolean, now: number): void {
  const entries = read();
  const same = (entry: RecentWork) => fromThisServer(entry) && entry.work.id === work.id;
  if (!entries.some(same) && !finished && progress <= 0.002) return;
  lastWrite = { id: work.id, at: now };
  const next = [{ work, at: now, progress, finished, server: currentServer ?? undefined }, ...entries.filter((entry) => !same(entry))].slice(0, maxEntries);
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
  const shelf = () => read().filter((entry) => !entry.finished && fromThisServer(entry));
  const [items, setItems] = useState(shelf);
  useEffect(() => {
    const update = () => setItems(shelf());
    window.addEventListener(changedEvent, update);
    return () => window.removeEventListener(changedEvent, update);
  }, []);
  return items;
}
