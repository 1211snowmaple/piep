import { useMemo, useState } from "react";
import { Box } from "@mantine/core";
import { WorkCover } from "@/components/WorkCover";
import { authorHue, sigilCells } from "@/lib/coverSigil";
import { getAssetUrl } from "@/services/dbApi";
import type { CollectionCoverTile, WorkCollectionSummary } from "@/types/collections";

type CollectionCoverData = Pick<
  WorkCollectionSummary,
  "id" | "name" | "coverMode" | "coverPath" | "coverImagePath" | "coverTiles" | "memberCount"
>;

/**
 * 束の表紙。
 *
 * 作れる材料はすでに全部あった。`cover_download_id` も、メンバーの `cover_path`
 * も保存されていて、`CollectionCard` が受け取って捨てていただけである。
 *
 * 一枚に決め打ちしないのは、束の性格が一定でないため。前後編の2作なら看板は
 * 1枚で足りるが、作者をまたぐ20作のテーマ束に代表作は無い。表紙の無い作品が
 * 混ざる棚でもあるので、**穴の空かない作り方**が既定でなければならない。
 * `WorkCover` をそのまま並べれば、表紙の無いマスには作品と同じ紋が入る。
 */
export function CollectionCover({
  collection,
  variant = "card",
  className,
}: {
  collection: CollectionCoverData;
  variant?: "card" | "detail";
  className?: string;
}) {
  const tiles = collection.coverTiles ?? [];
  const requestedMode = resolveMode(collection, tiles);
  const [failedImage, setFailedImage] = useState<string | null>(null);
  const imageSrc = requestedMode === "file"
    ? getAssetUrl(collection.coverImagePath)
    : requestedMode === "single"
      ? getAssetUrl(collection.coverPath)
      : null;
  const imageUnavailable = (requestedMode === "file" || requestedMode === "single")
    && (!imageSrc || failedImage === imageSrc);
  const mode = imageUnavailable ? (tiles.length === 0 ? "sigil" : "mosaic") : requestedMode;
  const classes = ["collection-cover", `collection-cover--${variant}`, `collection-cover--${mode}`, className]
    .filter(Boolean)
    .join(" ");

  if (mode === "file" || mode === "single") {
    const src = imageSrc;
    if (src && failedImage !== src) {
      return (
        <Box className={classes}>
          <img
            className="collection-cover__backdrop"
            src={src}
            alt=""
            aria-hidden
            loading={variant === "detail" ? "eager" : "lazy"}
            decoding="async"
            onError={(event) => { event.currentTarget.hidden = true; }}
          />
          <img
            className="collection-cover__image"
            src={src}
            alt={`${collection.name}の表紙`}
            loading={variant === "detail" ? "eager" : "lazy"}
            decoding="async"
            onError={() => setFailedImage(src)}
          />
        </Box>
      );
    }
  }

  if (mode === "sigil" || tiles.length === 0) {
    return <CollectionSigil collection={collection} className={classes} />;
  }

  // 2×2 まで。それ以上並べると1マスが小さくなりすぎて、何の絵か分からない。
  // 4枚に満たないときは席を詰めるのではなく、並べる枚数そのものを変える。
  const shown = tiles.slice(0, 4);
  return (
    <Box className={classes} data-tiles={shown.length} role="img" aria-label={`${collection.name}の複合表紙`}>
      {shown.map((tile, index) => {
        const backdrop = getAssetUrl(tile.coverPath);
        return (
          // 背表紙は奥へ向かってずれる。手前が並び順の先頭。ずらす量は外の枠に
          // 持たせて、`WorkCover` 自身は棚と同じものをそのまま使う。
          <span
            key={`${tile.source}:${tile.sourceId}`}
            className="collection-cover__slot"
            aria-hidden
            style={{ "--slot-index": index } as React.CSSProperties}
          >
            {/* 表紙は切り抜かない — 絵の下半分が消えるより、余白が出るほうが
                まだよい。ただし余白のままだと枠が欠けて見えるので、同じ絵を
                ぼかして敷いて埋める。切らずに、空きもない。 */}
            {backdrop && (
              <img
                className="collection-cover__backdrop"
                src={backdrop}
                alt=""
                aria-hidden
                loading="lazy"
                decoding="async"
                onError={(event) => { event.currentTarget.hidden = true; }}
              />
            )}
            <WorkCover work={tile} variant="compact" className="collection-cover__tile" />
          </span>
        );
      })}
    </Box>
  );
}

/**
 * 表紙が一枚も無い束の紋。
 *
 * 作品の紋と同じ考え方で、色は束の名前から、形は束の同一性から決める。
 * 束と作品で別の理屈を使うと、同じ棚に二種類の「表紙のなさ」が並ぶ。
 */
function CollectionSigil({ collection, className }: { collection: CollectionCoverData; className?: string }) {
  const hue = useMemo(() => authorHue(collection.name || collection.id), [collection.id, collection.name]);
  const cells = useMemo(() => sigilCells(`collection:${collection.id}`), [collection.id]);
  return (
    <Box
      className={className}
      role="img"
      aria-label={`${collection.name}（表紙なし）`}
      style={{ "--work-cover-hue": hue } as React.CSSProperties}
    >
      <span className="collection-cover__sigil" aria-hidden>
        {cells.map((filled, index) => <i key={index} data-on={filled || undefined} />)}
      </span>
    </Box>
  );
}

/** 指定された作り方が使えないときに、無理なく落ちる先を選ぶ。 */
function resolveMode(collection: CollectionCoverData, tiles: CollectionCoverTile[]) {
  if (collection.coverMode === "file" && collection.coverImagePath) return "file";
  if (collection.coverMode === "single" && collection.coverPath) return "single";
  if (collection.coverMode === "sigil") return "sigil";
  if (tiles.length === 0) return "sigil";
  if (collection.coverMode === "spine" && tiles.length >= 2) return "spine";
  return "mosaic";
}
