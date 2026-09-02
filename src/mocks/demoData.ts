import type {
  CollectionSuggestion,
  CollectionSuggestionMember,
  WorkCollection,
  WorkCollectionMember,
  WorkCollectionSummary,
} from "@/types/collections";
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
    coverPath: "data:image/svg+xml;utf8,%3Csvg%20xmlns%3D%27http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%27%20viewBox%3D%270%200%20480%20720%27%3E%3Crect%20width%3D%27480%27%20height%3D%27720%27%20fill%3D%27%2523f5e9e4%27%2F%3E%3Ccircle%20cx%3D%27240%27%20cy%3D%27240%27%20r%3D%2796%27%20fill%3D%27%2523a8552e%27%2F%3E%3Crect%20x%3D%2780%27%20y%3D%27360%27%20width%3D%27320%27%20height%3D%27240%27%20rx%3D%2734%27%20fill%3D%27%2523a8552e%27%2F%3E%3Ctext%20x%3D%27240%27%20y%3D%27710%27%20font-size%3D%2751%27%20text-anchor%3D%27middle%27%20fill%3D%27%2523a8552e%27%3E2%3A3%3C%2Ftext%3E%3C%2Fsvg%3E", jsonPath: "demo/pixiv-103.json", assetCount: 1, fileSizeBytes: 1_120_000, downloadedAt: isoDaysAgo(4), sourceCreatedAt: isoDaysAgo(5), sourceUpdatedAt: isoDaysAgo(5), currentVersion: 1, watchUpdates: true, textLength: 32640, favorite: true,
    personId: "4419281", personName: "遠野つむぎ", seriesId: "778120", seriesTitle: "星を編む人", matchFields: [], scoreReasons: [], matchHighlights: [],
  },
  {
    id: 104, source: "fanbox", sourceId: "10229018", title: "春の配布素材まとめ", authorName: "こはるデザイン室", authorId: "koharu_design",
    contentType: "image", tags: ["素材", "配布", "春"], excerpt: "壁紙と配信用素材をまとめてダウンロードできます。",
    coverPath: null, jsonPath: "demo/fanbox-104.json", assetCount: 28, fileSizeBytes: 46_210_000, downloadedAt: isoDaysAgo(9), sourceCreatedAt: isoDaysAgo(14), sourceUpdatedAt: isoDaysAgo(10), currentVersion: 2, watchUpdates: false, textLength: 920, favorite: false,
    personId: "koharu_design", personName: "こはるデザイン室", seriesId: null, seriesTitle: null, matchFields: [], scoreReasons: [], matchHighlights: [],
  },
  {
    id: 105, source: "pixiv", sourceId: "118772304", title: "灯台守の休日", authorName: "七瀬あかり", authorId: "6620117",
    contentType: "novel", tags: ["日常", "ほのぼの", "短編"], excerpt: "灯りを落とした朝、灯台守は初めて島の外を見に行く支度をした。",
    coverPath: "data:image/svg+xml;utf8,%3Csvg%20xmlns%3D%27http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%27%20viewBox%3D%270%200%20800%20450%27%3E%3Crect%20width%3D%27800%27%20height%3D%27450%27%20fill%3D%27%2523e9f2ea%27%2F%3E%3Ccircle%20cx%3D%27400%27%20cy%3D%27150%27%20r%3D%2790%27%20fill%3D%27%25233d6349%27%2F%3E%3Crect%20x%3D%27133%27%20y%3D%27225%27%20width%3D%27533%27%20height%3D%27150%27%20rx%3D%2732%27%20fill%3D%27%25233d6349%27%2F%3E%3Ctext%20x%3D%27400%27%20y%3D%27440%27%20font-size%3D%2732%27%20text-anchor%3D%27middle%27%20fill%3D%27%25233d6349%27%3E16%3A9%3C%2Ftext%3E%3C%2Fsvg%3E", jsonPath: "demo/pixiv-105.json", assetCount: 2, fileSizeBytes: 1_640_000, downloadedAt: isoDaysAgo(3), sourceCreatedAt: isoDaysAgo(11), sourceUpdatedAt: isoDaysAgo(11), currentVersion: 1, watchUpdates: true, textLength: 12980, favorite: false,
    personId: "6620117", personName: "七瀬あかり", seriesId: null, seriesTitle: null, matchFields: [], scoreReasons: [], matchHighlights: [],
  },
  {
    id: 106, source: "pixiv", sourceId: "119104552", title: "星を編む人 第十一話", authorName: "遠野つむぎ", authorId: "4419281",
    contentType: "novel", tags: ["ファンタジー", "連載", "宇宙"], excerpt: "観測塔に残されたのは、誰も読めない一枚の星図だけだった。",
    coverPath: "data:image/svg+xml;utf8,%3Csvg%20xmlns%3D%27http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%27%20viewBox%3D%270%200%20600%20900%27%3E%3Crect%20width%3D%27600%27%20height%3D%27900%27%20fill%3D%27%2523eef0f6%27%2F%3E%3Ccircle%20cx%3D%27300%27%20cy%3D%27300%27%20r%3D%27120%27%20fill%3D%27%252340506e%27%2F%3E%3Crect%20x%3D%27100%27%20y%3D%27450%27%20width%3D%27400%27%20height%3D%27300%27%20rx%3D%2742%27%20fill%3D%27%252340506e%27%2F%3E%3Ctext%20x%3D%27300%27%20y%3D%27890%27%20font-size%3D%2764%27%20text-anchor%3D%27middle%27%20fill%3D%27%252340506e%27%3E2%3A3%3C%2Ftext%3E%3C%2Fsvg%3E", jsonPath: "demo/pixiv-106.json", assetCount: 1, fileSizeBytes: 1_080_000, downloadedAt: isoDaysAgo(5), sourceCreatedAt: isoDaysAgo(19), sourceUpdatedAt: isoDaysAgo(19), currentVersion: 2, watchUpdates: true, textLength: 30110, favorite: false,
    personId: "4419281", personName: "遠野つむぎ", seriesId: "778120", seriesTitle: "星を編む人", matchFields: [], scoreReasons: [], matchHighlights: [],
  },
  {
    id: 107, source: "fanbox", sourceId: "10502377", title: "今月のラフ集", authorName: "mizu atelier", authorId: "mizu_atelier",
    contentType: "image", tags: ["ラフ", "制作記録", "限定"], excerpt: "採用されなかった構図もまとめて置いておきます。",
    coverPath: null, jsonPath: "demo/fanbox-107.json", assetCount: 34, fileSizeBytes: 62_400_000, downloadedAt: isoDaysAgo(6), sourceCreatedAt: isoDaysAgo(6), sourceUpdatedAt: isoDaysAgo(6), currentVersion: 1, watchUpdates: true, textLength: 640, favorite: true,
    personId: "mizu_atelier", personName: "mizu atelier", seriesId: null, seriesTitle: null, matchFields: [], scoreReasons: [], matchHighlights: [],
  },
  {
    id: 108, source: "pixiv", sourceId: "117903466", title: "機械仕掛けの海 第三章", authorName: "白鳥ケイ", authorId: "5528830",
    contentType: "novel", tags: ["SF", "連載", "長編"], excerpt: "潜水都市の照明が落ちる三分間だけ、彼女は本当のことを話した。",
    coverPath: null, jsonPath: "demo/pixiv-108.json", assetCount: 1, fileSizeBytes: 2_020_000, downloadedAt: isoDaysAgo(7), sourceCreatedAt: isoDaysAgo(24), sourceUpdatedAt: isoDaysAgo(12), currentVersion: 4, watchUpdates: true, textLength: 41250, favorite: true,
    personId: "5528830", personName: "白鳥ケイ", seriesId: "641902", seriesTitle: "機械仕掛けの海", matchFields: [], scoreReasons: [], matchHighlights: [],
  },
  {
    id: 109, source: "fanbox", sourceId: "10318744", title: "配信環境をつくり直した話", authorName: "こはるデザイン室", authorId: "koharu_design",
    contentType: "article", tags: ["制作記録", "配信", "機材"], excerpt: "机の上をやり直したら、配信の準備が十分で終わるようになりました。",
    coverPath: null, jsonPath: "demo/fanbox-109.json", assetCount: 9, fileSizeBytes: 12_880_000, downloadedAt: isoDaysAgo(12), sourceCreatedAt: isoDaysAgo(16), sourceUpdatedAt: isoDaysAgo(16), currentVersion: 1, watchUpdates: false, textLength: 5410, favorite: false,
    personId: "koharu_design", personName: "こはるデザイン室", seriesId: null, seriesTitle: null, matchFields: [], scoreReasons: [], matchHighlights: [],
  },
  {
    id: 110, source: "pixiv", sourceId: "115880210", title: "夏、置き去りの自転車", authorName: "青葉しおり", authorId: "8001234",
    contentType: "novel", tags: ["創作", "青春", "短編"], excerpt: "河原に一台だけ残された自転車を、その夏の終わりまで誰も動かさなかった。",
    coverPath: null, jsonPath: "demo/pixiv-110.json", assetCount: 4, fileSizeBytes: 2_960_000, downloadedAt: isoDaysAgo(15), sourceCreatedAt: isoDaysAgo(38), sourceUpdatedAt: isoDaysAgo(31), currentVersion: 2, watchUpdates: true, textLength: 21740, favorite: true,
    personId: "8001234", personName: "青葉しおり", seriesId: "220041", seriesTitle: "季節の栞", matchFields: [], scoreReasons: [], matchHighlights: [],
  },
  {
    id: 111, source: "pixiv", sourceId: "114620935", title: "猫と喫茶店の午後", authorName: "三日月ゆの", authorId: "7714002",
    contentType: "novel", tags: ["日常", "ほのぼの", "猫"], excerpt: "看板猫が席を選ぶので、この店では座る場所を客が決められない。",
    coverPath: null, jsonPath: "demo/pixiv-111.json", assetCount: 2, fileSizeBytes: 1_380_000, downloadedAt: isoDaysAgo(18), sourceCreatedAt: isoDaysAgo(45), sourceUpdatedAt: isoDaysAgo(45), currentVersion: 1, watchUpdates: false, textLength: 9860, favorite: false,
    personId: "7714002", personName: "三日月ゆの", seriesId: null, seriesTitle: null, matchFields: [], scoreReasons: [], matchHighlights: [],
  },
  {
    id: 112, source: "fanbox", sourceId: "10088451", title: "壁紙セット 2026 夏", authorName: "こはるデザイン室", authorId: "koharu_design",
    contentType: "image", tags: ["素材", "配布", "夏"], excerpt: "デスクトップとスマートフォン向けに、同じ絵柄を並べて書き出しました。",
    coverPath: null, jsonPath: "demo/fanbox-112.json", assetCount: 16, fileSizeBytes: 28_940_000, downloadedAt: isoDaysAgo(23), sourceCreatedAt: isoDaysAgo(27), sourceUpdatedAt: isoDaysAgo(27), currentVersion: 1, watchUpdates: false, textLength: 780, favorite: false,
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
    { name: "創作", count: 326 }, { name: "小説", count: 248 }, { name: "ファンタジー", count: 192 }, { name: "日常", count: 138 }, { name: "SF", count: 96 },
    { name: "制作記録", count: 84 }, { name: "短編", count: 72 }, { name: "素材", count: 41 },
    { name: "イラスト", count: 38 }, { name: "連載", count: 31 }, { name: "設定資料", count: 22 },
    { name: "習作", count: 14 }, { name: "背景", count: 13 }, { name: "キャラクター", count: 12 },
    { name: "ラフ", count: 11 }, { name: "水彩", count: 10 }, { name: "風景", count: 9 },
    { name: "季節", count: 8 }, { name: "日記", count: 7 }, { name: "告知", count: 6 },
    { name: "线画", count: 5 }, { name: "習慣", count: 4 }, { name: "旅", count: 3 },
    { name: "音楽", count: 2 },
  ],
  topAuthors: [
    { name: "青葉しおり", count: 48 }, { name: "遠野つむぎ", count: 37 }, { name: "mizu atelier", count: 31 }, { name: "白鳥ケイ", count: 24 }, { name: "こはるデザイン室", count: 19 },
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
    { source: "pixiv", sourceKey: "5528830", displayName: "白鳥ケイ", count: 24, coverPath: null, description: "海と機械をめぐる長編SF。", latestDownloadedAt: isoDaysAgo(7) },
    { source: "fanbox", sourceKey: "koharu_design", displayName: "こはるデザイン室", count: 19, coverPath: null, description: "配布素材と制作環境のノート。", latestDownloadedAt: isoDaysAgo(9) },
    { source: "pixiv", sourceKey: "6620117", displayName: "七瀬あかり", count: 14, coverPath: null, description: "島と海辺を舞台にした掌編。", latestDownloadedAt: isoDaysAgo(3) },
  ],
  series: [
    { source: "pixiv", sourceKey: "220041", displayName: "季節の栞", count: 18, coverPath: null, description: "四季をめぐる連作短編集。", latestDownloadedAt: isoDaysAgo(1) },
    { source: "pixiv", sourceKey: "778120", displayName: "星を編む人", count: 12, coverPath: null, description: "星図の外側を旅する長編シリーズ。", latestDownloadedAt: isoDaysAgo(4) },
    { source: "pixiv", sourceKey: "641902", displayName: "機械仕掛けの海", count: 9, coverPath: null, description: "潜水都市を舞台にした連載。", latestDownloadedAt: isoDaysAgo(7) },
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
  const work = demoWorks.find((item) => item.id === id);
  if (!work) throw new Error("作品が見つかりません");
  return work;
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

/**
 * ブラウザプレビューで見るコレクション。
 *
 * デスクトップ版でしか動かない画面を、プレビューでも「まだありません」以外の
 * 姿で見られるようにしておく。作品も作者もシリーズもデモを持っているのに、
 * コレクションだけ空だと**レイアウトの崩れが本番でしか見つからない**。
 * 実データで起きた見え方 — 表紙のあるもの、無いもの、名前の長いもの — を
 * ひととおり並べてある。
 */
function demoMember(work: DownloadEntry, position: number): WorkCollectionMember {
  return {
    collectionId: "demo-collection",
    source: work.source,
    sourceId: work.sourceId,
    downloadId: work.id,
    title: work.title,
    authorName: work.authorName,
    coverPath: work.coverPath,
    textLength: work.textLength,
    position,
    memberRole: "main",
    addedBy: "manual",
    pinned: false,
    note: null,
    missing: false,
    createdAt: isoDaysAgo(3),
    updatedAt: isoDaysAgo(1),
    work,
    editions: [],
  };
}

function demoCollection(
  id: string,
  name: string,
  members: DownloadEntry[],
  overrides: Partial<WorkCollectionSummary> = {},
): WorkCollection {
  return {
    id,
    name,
    description: null,
    collectionKind: "ordered",
    coverDownloadId: members[0]?.id ?? null,
    coverPath: members[0]?.coverPath ?? null,
    coverMode: "mosaic",
    coverImagePath: null,
    coverTiles: members.slice(0, 4).map((work) => ({
      source: work.source,
      sourceId: work.sourceId,
      title: work.title,
      authorName: work.authorName,
      coverPath: work.coverPath,
    })),
    nameSource: "manual",
    track: "manual",
    revision: 1,
    memberCount: members.length,
    availableCount: members.length,
    totalTextLength: members.reduce((total, work) => total + work.textLength, 0),
    createdAt: isoDaysAgo(9),
    updatedAt: isoDaysAgo(1),
    ...overrides,
    members: members.map(demoMember),
  };
}

export const demoCollections: WorkCollection[] = [
  demoCollection("demo-series", "星を編む人 第十一話・第十二話", [demoWorks[5], demoWorks[2]], {
    description: "本文のリンクで2作がつながっています",
    track: "sequence",
  }),
  // 名前が長い束。カードの行送りが崩れないかを、ここで見る。
  demoCollection(
    "demo-long",
    "同人女の感情 ハイスペイケメン女子の綾城さんにキモデブが溺愛されて界隈の姫になる話 関連作品",
    [demoWorks[0], demoWorks[4], demoWorks[1]],
    { description: "題名が連番になっている3作です", track: "sequence" },
  ),
  demoCollection("demo-theme", "創作 / 短編 / 青春", [demoWorks[0], demoWorks[4]], {
    description: "「創作」を共有し、本文も近い2作です",
    collectionKind: "unordered",
    track: "theme",
  }),
  // 表紙も作品も無い束。紋だけで成り立つかを見る。
  demoCollection("demo-empty", "あとで読む", [], { description: null }),
];

/**
 * プレビューで見る、走査の候補。
 *
 * 束のカードと同じ理由でここに置く。**空のままだと、カードの崩れが
 * デスクトップ版でしか見つからない。** 実際、2作の束で構成作品が読めない
 * ことに気づいたのは、実機の画面を見てからだった。
 *
 * 意図的に極端なものを混ぜてある。2作だけの束（「＋N」が出ない）、題名が
 * 長い束（行送りと省略）、12作の束（畳んだ一覧）、表紙の無い束。
 */
function demoSuggestionMember(work: DownloadEntry, position: number): CollectionSuggestionMember {
  return {
    source: work.source,
    sourceId: work.sourceId,
    downloadId: work.id,
    title: work.title,
    authorName: work.authorName,
    coverPath: work.coverPath,
    textLength: work.textLength,
    proposedPosition: position,
    score: 1,
    selected: true,
    evidence: [],
  };
}

function demoSuggestion(
  id: string,
  proposedName: string,
  members: DownloadEntry[],
  overrides: Partial<CollectionSuggestion> = {},
): CollectionSuggestion {
  return {
    id,
    proposedName,
    nameOptions: [
      { source: "title", name: proposedName, label: "題名の共通部分" },
      { source: "tags", name: members.flatMap((work) => work.tags).slice(0, 3).join(" / "), label: "共有タグ" },
      { source: "author", name: `${members[0]?.authorName ?? "作者"}のまとまり`, label: "作者" },
    ],
    collectionKind: "ordered",
    track: "sequence",
    origin: "sweep",
    evidenceSummary: `題名が連番になっている${members.length}作です`,
    score: 0.9,
    ruleVersion: "demo",
    state: "pending",
    members: members.map(demoSuggestionMember),
    createdAt: isoDaysAgo(1),
    updatedAt: isoDaysAgo(1),
    ...overrides,
  };
}

export const demoSuggestions: CollectionSuggestion[] = [
  // 2作だけ。「＋N」が出ないので、ここで構成作品が読めなければ判断できない。
  //
  // 題名は**わざと長く、書き出しを同じにしてある**。実データの束はこうなる
  // （同じ連載の続きが束になるので当然である）。切って出すと二作とも同じ
  // 文字列になり、見分けるための一文字が一つ残らず省略の向こうへ行く。
  {
    ...demoSuggestion("demo-sug-pair", "鉄壁の聖騎士さまが催眠ねっちょりポリネシアンセックスで防御スキルを剥がされる話", [demoWorks[5], demoWorks[2]], {
      evidenceSummary: "本文のリンクで2作がつながっています",
    }),
    members: [
      { ...demoSuggestionMember(demoWorks[5], 0), title: "鉄壁の聖騎士さまが催眠ねっちょりポリネシアンセックスで防御スキルを剥がされる話・前編" },
      { ...demoSuggestionMember(demoWorks[2], 1), title: "鉄壁の聖騎士さまが催眠ねっちょりポリネシアンセックスで防御スキルを剥がされる話・後編" },
    ],
  },
  // 題名も名前案も長い束。カードからはみ出さないかを、ここで見る。
  demoSuggestion(
    "demo-sug-long",
    "同人女の感情 ハイスペイケメン女子の綾城さんにキモデブが溺愛されて界隈の姫になる話 関連作品",
    [demoWorks[0], demoWorks[4], demoWorks[1]],
    { evidenceSummary: "題名が連番になっている3作です（作者2人）" },
  ),
  // 12作。畳んだ一覧の側を見る。
  demoSuggestion(
    "demo-sug-many",
    "創作 / 短編 / 青春",
    [...demoWorks, ...demoWorks].slice(0, 12),
    {
      track: "theme",
      collectionKind: "unordered",
      evidenceSummary: "「創作」を共有し、本文も近い12作です（作者4人）",
      score: 0.66,
    },
  ),
];

export function getDemoCollection(id: string): WorkCollection {
  const collection = demoCollections.find((item) => item.id === id);
  if (!collection) throw new Error("コレクションが見つかりません");
  return collection;
}
