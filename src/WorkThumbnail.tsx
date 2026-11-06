import { useEffect, useMemo, useState, type CSSProperties } from "react";
import type { PcWork } from "./pcApi";
import { coverSigil, titleScale } from "./coverSigil";
import { imageRecentlyFailed, savedCoverUrl, SmoothImage, workCoverUrl } from "./images";

/**
 * 作品の表紙。デスクトップ版の `WorkCover` と同じ決まりで描く。
 *
 * - 一覧の枠は `112 / 158`。画像は枠を切らずに収め、余りは表紙の地の色で埋める。
 *   縦長も横長も、描かれたものを欠けさせない。
 * - 詳細は画像本来の比率で見せる。読み込むまでは一覧と同じ枠で場所を取っておく。
 * - 表紙のない作品は、作者の色と作品ごとの紋で見分けられるようにする。
 */
export function WorkThumbnail({ work, offline = false, snapshotGeneration, variant = "card" }: { work: PcWork; offline?: boolean; snapshotGeneration?: number; variant?: "card" | "detail" }) {
  const src = !work.hasCover ? null : offline ? savedCoverUrl(work.id, snapshotGeneration ?? 0) : workCoverUrl(work, variant === "detail");
  const [failed, setFailed] = useState(() => imageRecentlyFailed(src));
  const [shape, setShape] = useState<string | null>(null);
  const sigil = useMemo(() => coverSigil(work), [work.source, work.sourceId, work.authorName]);
  useEffect(() => { setFailed(imageRecentlyFailed(src)); setShape(null); }, [src]);
  const showImage = src !== null && !failed;
  const style = {
    "--work-cover-hue": sigil.hue,
    "--work-cover-accent": work.source === "fanbox" ? "#f2c624" : "#0096fa",
    ...(variant === "detail" && shape ? { aspectRatio: shape } : {}),
  } as CSSProperties;

  return <span className="work-cover" data-image={showImage ? "true" : undefined} data-detail={variant === "detail" ? "true" : undefined} style={style}>
    {showImage
      ? <SmoothImage className="work-cover-image" src={src} alt={`${work.title}の表紙`} onFailed={() => setFailed(true)} onLoaded={(event) => {
        if (variant !== "detail") return;
        const { naturalWidth, naturalHeight } = event.currentTarget;
        // 極端に細長い絵で詳細が画面を埋めないよう、比率には幅を持たせて収める。
        if (naturalWidth > 0 && naturalHeight > 0) setShape(`${naturalWidth} / ${Math.min(naturalHeight, naturalWidth * 2)}`);
      }} />
      : <span className="cover-empty" role="img" aria-label={`${work.title}（表紙なし）`}>
        <span className="cover-sigil" aria-hidden="true">{sigil.cells.map((filled, index) => <i key={index} data-on={filled || undefined} />)}</span>
        <span className="cover-caption">{work.seriesTitle && <small>{work.seriesTitle}</small>}<span data-scale={titleScale(work.title)}>{work.title}</span></span>
      </span>}
  </span>;
}
