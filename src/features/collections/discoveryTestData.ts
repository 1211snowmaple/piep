import type { CollectionSuggestion } from "@/types/collections";

export function discoverySuggestion(id = "sug-1", name = "雨の連作"): CollectionSuggestion {
  return {
    id, proposedName: name, nameOptions: [{ name, source: "title", label: "題名の共通部分" }, { name: "作者の連作", source: "author", label: "作者" }],
    collectionKind: "ordered", track: "sequence", origin: "sweep", evidenceSummary: "題名の話数が続いています", score: 0.9, ruleVersion: "test", state: "pending", createdAt: "2026-09-01", updatedAt: "2026-09-01",
    members: [1, 2, 3].map((index) => ({ source: index === 3 ? "fanbox" : "pixiv", sourceId: String(index), downloadId: index, title: `作品${index}`, authorName: "作者", coverPath: null, textLength: 1000, proposedPosition: index - 1, score: 1, selected: true, evidence: [{ kind: "episode_order", label: `第${index}話`, contribution: 0 }] })),
  };
}
