import type { SidebarDownloadType, SidebarItem } from "@/features/browser/downloadCandidates";

export type SaveSource = "pixiv" | "fanbox";

export interface SaveDraft {
  items: SidebarItem[];
  downloadType: SidebarDownloadType | null;
  lastAnalysisUrl: string | null;
  lastAnalysisKey: string | null;
  browserUrl: string | null;
}

const emptyDraft = (): SaveDraft => ({
  items: [],
  downloadType: null,
  lastAnalysisUrl: null,
  lastAnalysisKey: null,
  browserUrl: null,
});

// Keep the candidate list for this app session. It can contain full post
// payloads, so storing it in localStorage would duplicate large private data.
const drafts: Record<SaveSource, SaveDraft> = {
  pixiv: emptyDraft(),
  fanbox: emptyDraft(),
};
const analysisRevision: Record<SaveSource, number> = { pixiv: 0, fanbox: 0 };
const listeners = new Set<() => void>();

export function readSaveDraft(source: SaveSource): SaveDraft {
  return drafts[source];
}

export function subscribeSaveDraft(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function updateSaveDraft(
  source: SaveSource,
  updater: (current: SaveDraft) => SaveDraft,
): void {
  const next = updater(drafts[source]);
  if (next === drafts[source]) return;
  drafts[source] = next;
  listeners.forEach((listener) => listener());
}

export function beginSaveDraftAnalysis(source: SaveSource): number {
  return ++analysisRevision[source];
}

export function isLatestSaveDraftAnalysis(source: SaveSource, revision: number): boolean {
  return analysisRevision[source] === revision;
}

export function resetSaveDraftsForTest(): void {
  drafts.pixiv = emptyDraft();
  drafts.fanbox = emptyDraft();
  analysisRevision.pixiv = 0;
  analysisRevision.fanbox = 0;
  listeners.forEach((listener) => listener());
}
