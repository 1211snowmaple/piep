// デスクトップ版 src/lib/providers.tsx の取得元マークと同じ字形・色を使う。
const pixivPath = "M5.2 14.7V3.3h4.3c2.7 0 4.5 1.7 4.5 4.2s-1.8 4.2-4.5 4.2H7.8v3H5.2Zm2.6-5.3h1.5c1.3 0 2.1-.7 2.1-1.9s-.8-1.9-2.1-1.9H7.8v3.8Z";

export function ProviderMark({ source, compact = false }: { source: string; compact?: boolean }) {
  const provider = source.toLowerCase();
  const label = provider === "fanbox" ? "FANBOX" : provider === "pixiv" ? "pixiv" : source || "その他";
  return <span className={`provider-mark provider-mark--${provider === "fanbox" ? "fanbox" : provider === "pixiv" ? "pixiv" : "other"}${compact ? " provider-mark--compact" : ""}`} aria-label={label}>
    <span className="provider-mark__icon" aria-hidden="true">
      {provider === "pixiv" ? <span className="brand-word-glyph brand-word-glyph--pixiv"><svg viewBox="0 0 18 18" role="presentation"><path d={pixivPath} /></svg></span>
        : <span className="brand-word-glyph" data-wide={provider === "fanbox" || undefined} data-chars={provider === "fanbox" ? 1 : Math.min(2, [...label].length)}>{provider === "fanbox" ? "F" : label.slice(0, 2)}</span>}
    </span>
    {!compact && <span className="provider-mark__label">{label}</span>}
  </span>;
}
