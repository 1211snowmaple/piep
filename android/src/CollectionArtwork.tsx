import { useEffect, useState, type CSSProperties } from "react";
import { authorHue, sigilCells } from "@shared/coverSigil";
import { collectionCoverUrl, savedCollectionCoverUrl, savedCoverUrl, SmoothImage } from "./images";
import type { OfflineWork, PcCollection } from "./pcApi";

function CoverImage({ src, alt, onFailed }: { src: string | null; alt: string; onFailed?: () => void }) {
  if (!src) return null;
  return <><img className="collection-art-backdrop" src={src} alt="" aria-hidden="true" /><SmoothImage className="collection-art-image" src={src} alt={alt} onFailed={onFailed} /></>;
}

function Sigil({ seed, hue }: { seed: string; hue: number }) {
  return <span className="collection-art-sigil" style={{ "--work-cover-hue": hue } as CSSProperties} aria-hidden="true">{sigilCells(seed).map((filled, index) => <i key={index} data-on={filled || undefined} />)}</span>;
}

function Tile({ collection, index, offlineWorks }: { collection: PcCollection; index: number; offlineWorks?: OfflineWork[] }) {
  const tile = collection.coverTiles![index];
  const saved = offlineWorks?.find((item) => item.work.source === tile.source && item.work.sourceId === tile.sourceId && item.work.hasCover);
  const src = saved ? savedCoverUrl(saved.work.id, saved.savedAtMs) : collectionCoverUrl(collection, `tile-${index}`, collection.coverRevision ?? 0);
  return <span className="collection-art-tile" style={{ "--slot-index": index } as CSSProperties}>
    <span className="collection-art-tile-fallback" style={{ "--work-cover-hue": authorHue(tile.authorName || tile.source) } as CSSProperties}>
      <Sigil seed={`${tile.source}:${tile.sourceId}`} hue={authorHue(tile.authorName || tile.source)} />
    </span>
    {tile.hasCover && (!offlineWorks || saved) && <CoverImage src={src} alt="" />}
  </span>;
}

export function CollectionArtwork({ collection, offlineWorks }: { collection: PcCollection; offlineWorks?: OfflineWork[] }) {
  const [mainFailed, setMainFailed] = useState(false);
  const tiles = collection.coverTiles?.slice(0, 4) ?? [];
  const requested = collection.coverMode ?? "sigil";
  // 表紙に選んだ作品の表紙は、棚から `single` で引く（保存した作品だけの画面では出さない）。
  const shelfSingle = Boolean(collection.shelfLibrary && !offlineWorks);
  const revision = collection.coverRevision ?? 0;
  const savedFile = offlineWorks?.find((item) => item.collectionCoverIds?.includes(collection.id)
    && item.collections?.some((entry) => entry.id === collection.id && entry.coverRevision === collection.coverRevision));
  const hasMain = (requested === "file" && collection.hasFileCover && (!offlineWorks || savedFile))
    || (requested === "single" && collection.hasSingleCover && shelfSingle);
  const main = hasMain && !mainFailed;
  // 画像のない枠にも作品ごとの印が残る。コレクションで選んだ並べ方は、枠があるかぎり保つ。
  const mode = main ? requested : requested === "sigil" || tiles.length === 0 ? "sigil" : requested === "spine" && tiles.length >= 2 ? "spine" : "mosaic";
  const mainSrc = !main ? null
    : requested === "file"
      ? savedFile ? savedCollectionCoverUrl(savedFile.work.id, collection.id, savedFile.savedAtMs) : collectionCoverUrl(collection, "file", revision)
      : collectionCoverUrl(collection, "single", revision);

  useEffect(() => { setMainFailed(false); }, [collection.id, revision, requested]);

  return <span className={`collection-art collection-art--${mode}`} data-tiles={tiles.length} role="img" aria-label={`${collection.name}の表紙`} style={{ "--work-cover-hue": authorHue(collection.name || collection.id) } as CSSProperties}>
    {main ? <CoverImage src={mainSrc} alt="" onFailed={() => setMainFailed(true)} />
      : mode === "sigil" ? <Sigil seed={`collection:${collection.id}`} hue={authorHue(collection.name || collection.id)} />
        : tiles.map((_, index) => <Tile key={`${collection.id}:${index}`} collection={collection} index={index} offlineWorks={offlineWorks} />)}
  </span>;
}
