// 生成物。手で直さない。
// 元は crates/piep-mobile-api/src/lib.rs。直すときはそちらを直して、
// `cargo run --manifest-path tools/api-types/Cargo.toml -- --repo .` で生成し直す。

export interface MobileWork {
  id: number;
  source: string;
  sourceId: string;
  title: string;
  authorName: string;
  authorId: string;
  contentType: string;
  tags: string[];
  excerpt: string | null;
  textLength: number;
  assetCount: number;
  hasCover: boolean;
  currentVersion: number;
  contentHash: string | null;
  favorite: boolean;
  seriesTitle: string | null;
  seriesId: string | null;
  sourceCreatedAt: string | null;
  downloadedAt: string;
}

export interface MobileSearchPage {
  items: MobileWork[];
  nextCursor: string | null;
  totalEstimate: number | null;
  indexComplete: boolean;
}

export interface MobileEntity {
  source: string;
  sourceKey: string;
  displayName: string;
  count: number;
  description: string | null;
  sampleTitle: string | null;
  hasCover: boolean;
  isConcluded: boolean | null;
}

export interface MobileEntityPage {
  items: MobileEntity[];
  nextOffset: number | null;
  total: number;
}

/** 作者の中のシリーズ一覧。続きは不透明なカーソルで辿る。 */
export interface MobileEntitySeriesPage {
  items: MobileEntity[];
  nextCursor: string | null;
  total: number;
}

export interface MobileFacet {
  name: string;
  count: number;
}

/** 作者・シリーズの画面を一度で描くための情報。保存先のパスは含めない。 */
export interface MobileEntityDetail {
  source: string;
  sourceKey: string;
  displayName: string;
  description: string | null;
  workCount: number;
  hasCover: boolean;
  hasBanner: boolean;
  isConcluded: boolean | null;
  publishedCount: number | null;
  links: string[];
  tags: MobileFacet[];
  seriesCount: number | null;
  collections: MobileCollection[];
}

export interface MobileCollection {
  id: string;
  name: string;
  description: string | null;
  collectionKind: string;
  memberCount: number;
  availableCount: number;
  coverWorkId: number | null;
  coverMode: string;
  coverRevision: number;
  hasSingleCover: boolean;
  hasFileCover: boolean;
  coverTiles: MobileCollectionCoverTile[];
  position?: number;
}

export interface MobileCollectionCoverTile {
  source: string;
  sourceId: string;
  title: string;
  authorName: string;
  hasCover: boolean;
}

export interface MobileCollectionMember {
  title: string;
  authorName: string;
  position: number;
  work: MobileWork | null;
}

export interface MobileCollectionDetail extends MobileCollection {
  members: MobileCollectionMember[];
}

export interface MobileVersion {
  version: number;
  contentHash: string | null;
  textLength: number;
  assetCount: number;
  createdAt: string;
}

export interface MobilePerson {
  source: string;
  key: string;
  role: string;
  displayName: string;
}

export interface MobileSeriesMembership {
  source: string;
  key: string;
  title: string;
  contentOrder: number | null;
}

export interface MobileMetadata {
  work: MobileWork;
  people: MobilePerson[];
  series: MobileSeriesMembership[];
  versions: MobileVersion[];
  isEdited: boolean;
  activeEditId: number | null;
  activeEditHash: string | null;
}

export interface MobilePage {
  page: number;
  pageCount: number;
  html: string;
  totalPlainTextChars: number;
  sourcePageStarts: number[];
  /** この本文がどの版のものか。端末が本文の前後でメタデータを取り直さずに済む。 */
  /** サーバーは必ず入れる。端末に残っている古い保存分には無い。 */
  revision?: MobileRevision;
}

/** 本文の版を見分ける三つ組み。`MobileMetadata` の同じ値と一致する。 */
export interface MobileRevision {
  currentVersion: number;
  contentHash: string | null;
  activeEditHash: string | null;
}

/** 新しい棚の作品の一件（棚の UUID で指し、旧棚の番号を持たない）。項目は `MobileWork` に揃える。 */
export interface MobileShelfWork {
  /** 作品の UUID。 */
  id: string;
  /** 今の保存版の UUID。版が変わると変わるので、表紙を取り直す目印になる。 */
  snapshotId: string;
  source: string;
  sourceId: string;
  title: string;
  authorName: string;
  /** 取得元の作者の ID。分からなければ空。 */
  authorId: string;
  contentType: string;
  tags: string[];
  excerpt: string | null;
  textLength: number;
  assetCount: number;
  hasCover: boolean;
  currentVersion: number;
  /** 反映中の編集があるか。 */
  edited: boolean;
  favorite: boolean;
  seriesTitle: string | null;
  seriesId: string | null;
  /** 公開日（ISO 8601）。 */
  sourceCreatedAt: string | null;
  /** 保存日（ISO 8601）。 */
  downloadedAt: string;
}

/** 新しい棚の一覧の一ページ。続きは `next_cursor` を変えずに次の要求へ渡す。 */
export interface MobileShelfPage {
  items: MobileShelfWork[];
  nextCursor: string | null;
  totalEstimate: number | null;
  indexComplete: boolean;
}

/** 新しい棚の作品の保存した版の一つ。 */
export interface MobileShelfVersion {
  version: number;
  textLength: number | null;
  /** 保存日（ISO 8601）。 */
  savedAt: string;
}

/** 新しい棚の作品の画面。`work.excerpt` は取得元の説明。 */
export interface MobileShelfWorkDetail {
  work: MobileShelfWork;
  people: MobilePerson[];
  series: MobileSeriesMembership[];
  /** 古い順。 */
  versions: MobileShelfVersion[];
  /** 反映中の編集の基準の版。編集がなければ空。 */
  editBaseVersion: number | null;
}
