import type { DashboardSummary, DownloadEntry, EditorDocument, FilterFacets, ReaderDocument, SearchV2Result } from "@/types/library";

const now = new Date();
const isoDaysAgo = (days: number) => new Date(now.getTime() - days * 86_400_000).toISOString();

export const demoWorks: DownloadEntry[] = [
  {
    id: 101, source: "pixiv", sourceId: "116041921", title: "雨上がりの図書室で", authorName: "青葉しおり", authorId: "8001234",
    contentType: "novel", tags: ["創作", "青春", "短編"], excerpt: "雨音が止んだ午後、閉館前の図書室で二人はもう一度出会った。",
    coverPath: null, jsonPath: "demo/pixiv-101.json", assetCount: 3, fileSizeBytes: 2_480_000, downloadedAt: isoDaysAgo(1), sourceCreatedAt: isoDaysAgo(21), sourceUpdatedAt: isoDaysAgo(2), currentVersion: 3, watchUpdates: true, textLength: 18420, favorite: true,
    personId: "8001234", personName: "青葉しおり", seriesId: "220041", seriesTitle: "季節の栞", matchFields: [], scoreReasons: [], matchHighlights: [],
  },
  {
    id: 102, source: "fanbox", sourceId: "10483920", title: "制作ノート #24 — 光の設計", authorName: "mizu atelier", authorId: "mizu_atelier",
    contentType: "article", tags: ["制作記録", "背景", "メイキング"], excerpt: "今月制作した一枚の、ラフから仕上げまでをまとめました。",
    coverPath: null, jsonPath: "demo/fanbox-102.json", assetCount: 12, fileSizeBytes: 18_720_000, downloadedAt: isoDaysAgo(2), sourceCreatedAt: isoDaysAgo(8), sourceUpdatedAt: isoDaysAgo(8), currentVersion: 1, watchUpdates: true, textLength: 6280, favorite: false,
    personId: "mizu_atelier", personName: "mizu atelier", seriesId: null, seriesTitle: null, matchFields: [], scoreReasons: [], matchHighlights: [],
  },
  {
    id: 103, source: "pixiv", sourceId: "119238841", title: "星を編む人 第十二話", authorName: "遠野つむぎ", authorId: "4419281",
    contentType: "novel", tags: ["ファンタジー", "連載", "宇宙"], excerpt: "夜空に残された糸をたどり、ミナは星図の外側へ踏み出す。",
    coverPath: null, jsonPath: "demo/pixiv-103.json", assetCount: 1, fileSizeBytes: 1_120_000, downloadedAt: isoDaysAgo(4), sourceCreatedAt: isoDaysAgo(5), sourceUpdatedAt: isoDaysAgo(5), currentVersion: 1, watchUpdates: true, textLength: 32640, favorite: true,
    personId: "4419281", personName: "遠野つむぎ", seriesId: "778120", seriesTitle: "星を編む人", matchFields: [], scoreReasons: [], matchHighlights: [],
  },
  {
    id: 104, source: "fanbox", sourceId: "10229018", title: "春の配布素材まとめ", authorName: "こはるデザイン室", authorId: "koharu_design",
    contentType: "image", tags: ["素材", "配布", "春"], excerpt: "壁紙と配信用素材をまとめてダウンロードできます。",
    coverPath: null, jsonPath: "demo/fanbox-104.json", assetCount: 28, fileSizeBytes: 46_210_000, downloadedAt: isoDaysAgo(9), sourceCreatedAt: isoDaysAgo(14), sourceUpdatedAt: isoDaysAgo(10), currentVersion: 2, watchUpdates: false, textLength: 920, favorite: false,
    personId: "koharu_design", personName: "こはるデザイン室", seriesId: null, seriesTitle: null, matchFields: [], scoreReasons: [], matchHighlights: [],
  },
];

export const demoDashboard: DashboardSummary = {
  stats: { totalDownloads: 1284, pixivCount: 936, fanboxCount: 348, totalAssets: 8241, totalSizeBytes: 14_680_000_000 },
  favoriteCount: 86,
  watchedCount: 214,
  updateTargetCount: 43,
  indexedCount: 1284,
  pendingIndexCount: 0,
  topTags: [
    { name: "創作", count: 326 }, { name: "小説", count: 248 }, { name: "ファンタジー", count: 192 }, { name: "制作記録", count: 84 }, { name: "短編", count: 72 },
  ],
  topAuthors: [
    { name: "青葉しおり", count: 48 }, { name: "遠野つむぎ", count: 37 }, { name: "mizu atelier", count: 31 },
  ],
  recentDownloads: demoWorks,
  sourceBreakdown: [
    { source: "pixiv", count: 936, totalSizeBytes: 7_920_000_000 },
    { source: "fanbox", count: 348, totalSizeBytes: 6_760_000_000 },
  ],
  monthlyDownloads: [
    { bucket: "2026-03", count: 72, pixivCount: 51, fanboxCount: 21, totalSizeBytes: 912_000_000 },
    { bucket: "2026-04", count: 96, pixivCount: 68, fanboxCount: 28, totalSizeBytes: 1_084_000_000 },
    { bucket: "2026-05", count: 81, pixivCount: 59, fanboxCount: 22, totalSizeBytes: 806_000_000 },
    { bucket: "2026-06", count: 118, pixivCount: 83, fanboxCount: 35, totalSizeBytes: 1_260_000_000 },
    { bucket: "2026-07", count: 104, pixivCount: 72, fanboxCount: 32, totalSizeBytes: 1_118_000_000 },
    { bucket: "2026-08", count: 14, pixivCount: 10, fanboxCount: 4, totalSizeBytes: 142_000_000 },
  ],
};

export const demoFacets: FilterFacets = {
  tags: demoDashboard.topTags,
  authors: demoDashboard.topAuthors,
  authorEntities: [
    { source: "pixiv", sourceKey: "8001234", displayName: "青葉しおり", count: 48, coverPath: null, description: "季節と日常を題材にした短編小説。", latestDownloadedAt: isoDaysAgo(1) },
    { source: "pixiv", sourceKey: "4419281", displayName: "遠野つむぎ", count: 37, coverPath: null, description: "空想科学と冒険譚を中心に執筆。", latestDownloadedAt: isoDaysAgo(4) },
    { source: "fanbox", sourceKey: "mizu_atelier", displayName: "mizu atelier", count: 31, coverPath: null, description: "背景イラストと制作ノート。", latestDownloadedAt: isoDaysAgo(2) },
  ],
  series: [
    { source: "pixiv", sourceKey: "220041", displayName: "季節の栞", count: 18, coverPath: null, description: "四季をめぐる連作短編集。", latestDownloadedAt: isoDaysAgo(1) },
    { source: "pixiv", sourceKey: "778120", displayName: "星を編む人", count: 12, coverPath: null, description: "星図の外側を旅する長編シリーズ。", latestDownloadedAt: isoDaysAgo(4) },
  ],
  contentTypes: [{ name: "novel", count: 936 }, { name: "article", count: 276 }, { name: "image", count: 72 }],
  assetTypes: [{ name: "image", count: 7102 }, { name: "file", count: 1139 }],
};

export function searchDemoWorks(text = "", source?: string | null): SearchV2Result {
  const q = text.toLocaleLowerCase("ja-JP").trim();
  const items = demoWorks.filter((work) => {
    if (source && work.source !== source) return false;
    if (!q) return true;
    return [work.title, work.authorName, work.excerpt ?? "", ...work.tags].join(" ").toLocaleLowerCase("ja-JP").includes(q);
  });
  return {
    items,
    nextCursor: null,
    totalEstimate: items.length,
    searchMeta: { engine: "preview", query: q || null, totalEstimate: items.length, indexComplete: true, semanticIndexComplete: true, semanticModelReady: true },
    facetsVersion: 1,
  };
}

export function getDemoWork(id: number): DownloadEntry {
  return demoWorks.find((work) => work.id === id) ?? demoWorks[0];
}

const demoHtml = `
  <h2>雨上がりの図書室で</h2>
  <p>雨音が止んだのは、図書室の時計が午後五時を指す少し前だった。</p>
  <p>窓辺に細く残った水滴が、雲の切れ間から差し込む光を拾っている。栞は読みかけの本を閉じ、その向こうに立つ人影を見た。</p>
  <h3>1　青い傘</h3>
  <p>「まだ、ここにいたんだ」</p>
  <p>懐かしい声だった。何年も前から知っていて、けれど今朝までは二度と聞けないと思っていた声。</p>
  <p>開いた窓から、濡れた木々の匂いが流れ込んだ。栞は答える代わりに、机の向かい側の椅子を引いた。</p>
`;

export function getDemoReader(id: number): ReaderDocument {
  const download = getDemoWork(id);
  return {
    download,
    assets: [],
    versions: [{ id: 1, downloadId: download.id, version: download.currentVersion, contentHash: null, textLength: download.textLength, jsonPath: download.jsonPath, originalJsonPath: null, assetCount: download.assetCount, fileSizeBytes: download.fileSizeBytes, createdAt: download.downloadedAt, changeSummary: "初回保存" }],
    html: demoHtml,
    plainText: demoHtml.replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim(),
    isEdited: false,
    activeEditRevision: null,
  };
}

export function getDemoEditor(id: number): EditorDocument {
  const reader = getDemoReader(id);
  return {
    download: reader.download,
    assets: [],
    activeRevision: null,
    draftRevision: null,
    baseVersion: reader.download.currentVersion,
    blocks: [
      { id: 1, editRevisionId: 0, order: 0, blockType: "heading", text: "雨上がりの図書室で", assetId: null, attrsJson: null },
      { id: 2, editRevisionId: 0, order: 1, blockType: "paragraph", text: "雨音が止んだのは、図書室の時計が午後五時を指す少し前だった。", assetId: null, attrsJson: null },
      { id: 3, editRevisionId: 0, order: 2, blockType: "paragraph", text: "窓辺に細く残った水滴が、雲の切れ間から差し込む光を拾っている。", assetId: null, attrsJson: null },
    ],
  };
}
