import { useEffect, useMemo, useState, type CSSProperties } from "react";
import type { PcWork } from "./pcApi";
import { coverSigil, titleScale } from "./coverSigil";
import { imageRecentlyFailed, savedCoverUrl, SmoothImage, workCoverUrl } from "./images";

/** 枠の大きさは画像の有無で変えない。読み込みで一覧が動かないようにするため。 */
export function WorkThumbnail({ work, offline = false, snapshotGeneration, variant = "card" }: { work: PcWork; offline?: boolean; snapshotGeneration?: number; variant?: "card" | "detail" }) {
  const src = !work.hasCover ? null : offline ? savedCoverUrl(work.id, snapshotGeneration ?? 0) : workCoverUrl(work, variant === "detail");
  const [failed, setFailed] = useState(() => imageRecentlyFailed(src));
  const sigil = useMemo(() => coverSigil(work), [work.source, work.sourceId, work.authorName]);
  useEffect(() => { setFailed(imageRecentlyFailed(src)); }, [src]);
  const showImage = src !== null && !failed;
  // 枠（2:3）に近い比率だけ切り抜く。詳細では表紙の全体を見せる。
  const [fill, setFill] = useState(false);

  return <span className="work-cover" data-image={showImage ? "true" : undefined} data-detail={variant === "detail" ? "true" : undefined} style={{ "--work-cover-hue": sigil.hue, "--work-cover-accent": work.source === "fanbox" ? "#f2c624" : "#0096fa" } as CSSProperties}>
    {showImage ? <>
      {variant === "detail" && <img className="work-cover-backdrop" src={src} alt="" aria-hidden="true" />}
      <SmoothImage className={fill && variant === "card" ? "work-cover-image work-cover-image--fill" : "work-cover-image"} src={src} alt={`${work.title}の表紙`} onFailed={() => setFailed(true)} onLoaded={(event) => { const { naturalWidth, naturalHeight } = event.currentTarget; setFill(naturalHeight > 0 && naturalWidth / naturalHeight > .58 && naturalWidth / naturalHeight < .76); }} />
    </> : <span className="cover-empty" role="img" aria-label={`${work.title}（表紙なし）`}>
      <span className="cover-sigil" aria-hidden="true">{sigil.cells.map((filled, index) => <i key={index} data-on={filled || undefined} />)}</span>
      <span className="cover-caption">{work.seriesTitle && <small>{work.seriesTitle}</small>}<span data-scale={titleScale(work.title)}>{work.title}</span></span>
    </span>}
  </span>;
}
