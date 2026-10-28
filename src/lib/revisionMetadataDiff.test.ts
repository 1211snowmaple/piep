import { expect, it } from "vitest";
import { compareRevisionMetadata } from "./revisionMetadataDiff";
import type { RevisionAsset, RevisionMetadata } from "@/services/workInspectionApi";

const metadata: RevisionMetadata = { title: "題名", caption: "説明", tags: ["一", "二"], publishedAt: "2026-10-01T00:00:00Z", updatedAt: null, assets: [] };
const image: RevisionAsset = { key: "cover", kind: "cover", label: "cover.jpg", source: "https://i.pximg.net/cover.jpg", localPath: null, available: true, contentHash: "same" };

it("identifies caption links, tags, covers and attachments even when the body is identical", () => {
  const before = { ...metadata, caption: '<a href="https://example.com/old">続き</a>', assets: [image] };
  const after = { ...metadata, caption: '<a href="https://example.com/new">続き</a>', tags: ["三"], assets: [{ ...image, contentHash: "new" }, { ...image, key: "files:manuscript", kind: "files" }] };
  const changes = compareRevisionMetadata(before, after);
  expect(changes.fields.map((change) => change.label)).toEqual(["説明", "タグ"]);
  expect(changes.fields[0].before).toContain("https://example.com/old");
  expect(changes.assets.map((asset) => asset.change)).toEqual(["changed", "added"]);
});

it("uses saved image bytes rather than path or URL changes and reports repairs separately", () => {
  const changes = compareRevisionMetadata({ ...metadata, assets: [image] }, { ...metadata, assets: [{ ...image, source: "https://new.example/a.jpg", localPath: "other/path" }] });
  expect(changes.assets).toEqual([]);
  const repair = compareRevisionMetadata({ ...metadata, assets: [{ ...image, available: false, contentHash: null, source: "" }] }, { ...metadata, assets: [image] });
  expect(repair.assets[0].change).toBe("availability");
});

it("does not invent old metadata or confuse the same instant with a date change", () => {
  const changes = compareRevisionMetadata({ ...metadata, title: null }, { ...metadata, publishedAt: "2026-10-01T09:00:00+09:00" });
  expect(changes.unknown).toEqual(["タイトル"]);
  expect(changes.fields).toEqual([]);
});

it("shows removed files, tag order and FANBOX formatting changes", () => {
  const changes = compareRevisionMetadata({ ...metadata, assets: [image], bodyStructure: "段落" }, { ...metadata, tags: ["二", "一"], bodyStructure: "段落 · 太字" });
  expect(changes.assets[0].change).toBe("removed");
  expect(changes.fields.map((field) => field.label)).toEqual(["本文の構成・リンク・書式", "タグの並び順"]);
});
