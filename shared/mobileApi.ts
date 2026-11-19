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
  /** 反映中の編集の UUID。編集し直すと変わるので、読書位置と保存の印を分ける目印になる。 */
  editId: string | null;
  favorite: boolean;
  /** この利用者のお気に入りの更新世代（十進の文字列）。付け外しの `expected` に渡す。 */
  favoriteGeneration: string;
  seriesTitle: string | null;
  seriesId: string | null;
  /** 公開日（ISO 8601）。 */
  sourceCreatedAt: string | null;
  /** 保存日（ISO 8601）。 */
  downloadedAt: string;
}

/** 新しい棚の作品のお気に入りを付け外しする。`request_id` は端末が作る UUID で、送り直しても */
/** 一度だけ効く。`expected` は手元の更新世代（十進の文字列）。違えば 409 で今の値が返る。 */
export interface MobileShelfFavoriteUpdate {
  requestId: string;
  expected: string;
  favorite: boolean;
}

/** お気に入りの今の値と更新世代（十進の文字列）。 */
export interface MobileShelfFavorite {
  favorite: boolean;
  generation: string;
}

/** 新しい棚の読書位置の場所。`view_kind` は `snapshot`（版の写し）か `edit`（編集）で、 */
/** `view_id` はその UUID。ページは本文の区切り（pixiv の `[newpage]`、編集の改ページ）で数えた */
/** 0 からの番号、`ratio_ppm` はそのページの中のスクロールの割合（百万分率）。 */
export interface MobileShelfReadingLocation {
  viewKind: string;
  viewId: string;
  page: number;
  ratioPpm: number;
}

/** 新しい棚の作品の読書位置（認証した利用者のもの）。`location` が空なのは、消した位置か、 */
/** ページで表せない位置。`mine` はこの端末が書いた位置か。`updated_at` は棚が受けた時刻 */
/** （ミリ秒）。`generation` は更新世代（十進の文字列。まだ位置が無ければ "0"）。 */
export interface MobileShelfReading {
  work: string;
  location: MobileShelfReadingLocation | null;
  finished: boolean;
  mine: boolean;
  updatedAt: number | null;
  generation: string;
}

/** 読書位置の一覧の一ページ。初めは `cursor` を空で頼み、返った `cursor` を次へ渡す。 */
/** `more` なら続きがある。続きが無くなったあとも、同じ `cursor` で変わった分だけを受け取れる。 */
export interface MobileShelfReadingPage {
  items: MobileShelfReading[];
  cursor: string;
  more: boolean;
}

/** 新しい棚の作品の読書位置を書く。`request_id` は端末が作る UUID で、送り直しても一度だけ */
/** 効く。`expected` は手元の更新世代（十進の文字列）。違えば 409 で今の位置が返る。 */
export interface MobileShelfReadingUpdate {
  requestId: string;
  expected: string;
  location: MobileShelfReadingLocation;
  finished: boolean;
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
  /** その版の写しの ID。以前の版を読むときはこの写しの本を引く。 */
  snapshotId: string;
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

/** 新しい棚の検索欄の候補（旧 API の `search/suggest` の一件と同じ形）。作者・シリーズは */
/** `source`・`source_key` が取得元と ID、題は `value` が取得元の ID で `source_key` が棚の */
/** 作品の ID、タグは `value` がタグ。 */
export interface MobileSearchSuggestion {
  kind: string;
  label: string;
  value: string;
  count: number | null;
  exactMatch: boolean;
  source: string | null;
  sourceKey: string | null;
}

/** 新しい棚の検索欄の候補の一覧。 */
export interface MobileSearchSuggestions {
  items: MobileSearchSuggestion[];
}

/** 新しい棚のコレクションの一回。棚に無い（未保存の）回は `work` が空で、入れた時の題と作者。 */
export interface MobileShelfCollectionMember {
  title: string;
  authorName: string;
  position: number;
  work: MobileShelfWork | null;
}

/** 新しい棚のコレクションの画面。`id` は棚の UUID、表紙は `cover_mode` の枠ごとに */
/** `/collections/{id}/cover/{single|file|tile-N}` から引く。 */
export interface MobileShelfCollectionDetail extends MobileCollection {
  members: MobileShelfCollectionMember[];
}
