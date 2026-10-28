import type { RevisionAsset, RevisionMetadata } from "@/services/workInspectionApi";
import { parseDate } from "./format";

export interface MetadataChange { label: string; before: string; after: string }
export interface AssetChange { key: string; kind: string; change: "added" | "removed" | "changed" | "availability"; before?: RevisionAsset; after?: RevisionAsset }

/** Preserve link destinations as well as visible prose when comparing captions. */
export function captionText(html: string): string {
  if (typeof DOMParser === "undefined") return html;
  const document = new DOMParser().parseFromString(html, "text/html");
  document.querySelectorAll("script,style").forEach((node) => node.remove());
  document.querySelectorAll("a[href]").forEach((anchor) => {
    const href = anchor.getAttribute("href") ?? "";
    if (anchor.textContent !== href) anchor.append(` (${href})`);
  });
  document.querySelectorAll("br").forEach((node) => node.replaceWith("\n"));
  document.querySelectorAll("p,div,li").forEach((node) => node.append("\n"));
  return document.body.textContent?.trim() ?? "";
}

function sameDate(left: string, right: string) {
  const l = parseDate(left).getTime(), r = parseDate(right).getTime();
  return Number.isFinite(l) && Number.isFinite(r) ? l === r : left === right;
}

export function compareRevisionMetadata(before: RevisionMetadata, after: RevisionMetadata) {
  const fields: MetadataChange[] = [];
  const unknown: string[] = [];
  for (const [key, label] of [["title", "タイトル"], ["caption", "説明"], ["author", "作者"], ["series", "シリーズ"], ["bodyStructure", "本文の構成・リンク・書式"], ["publishedAt", "公開日時"], ["updatedAt", "更新日時"]] as const) {
    const left = before[key] ?? null, right = after[key] ?? null;
    if (left === null || right === null) { if (left !== right) unknown.push(label); continue; }
    if (key.endsWith("At") ? sameDate(left, right) : left === right) continue;
    const l = key === "caption" ? captionText(left) : left;
    const r = key === "caption" ? captionText(right) : right;
    fields.push({ label: key === "caption" && l === r ? "説明の書式" : label, before: l === r ? left : l, after: l === r ? right : r });
  }
  if (before.tags === null || after.tags === null) {
    if (before.tags !== after.tags) unknown.push("タグ");
  } else if (JSON.stringify(before.tags) !== JSON.stringify(after.tags)) {
    const sameSet = before.tags.length === after.tags.length && before.tags.every((tag) => after.tags!.includes(tag));
    fields.push({ label: sameSet ? "タグの並び順" : "タグ", before: before.tags.join(" · "), after: after.tags.join(" · ") });
  }
  const oldAssets = new Map(before.assets.map((asset) => [asset.key, asset]));
  const newAssets = new Map(after.assets.map((asset) => [asset.key, asset]));
  const assets: AssetChange[] = [];
  for (const key of new Set([...oldAssets.keys(), ...newAssets.keys()])) {
    const left = oldAssets.get(key), right = newAssets.get(key);
    const kind = (right ?? left)!.kind;
    if (!left) assets.push({ key, kind, change: "added", after: right });
    else if (!right) assets.push({ key, kind, change: "removed", before: left });
    else {
      const sameContent = left.contentHash && right.contentHash ? left.contentHash === right.contentHash : !left.source || !right.source || left.source === right.source;
      if (!sameContent || (kind === "files" && left.label !== right.label)) assets.push({ key, kind, change: "changed", before: left, after: right });
      else if (left.available !== null && right.available !== null && left.available !== right.available) assets.push({ key, kind, change: "availability", before: left, after: right });
    }
  }
  return { fields, assets, unknown };
}
