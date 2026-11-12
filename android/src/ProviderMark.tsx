import { BRAND_GLYPHS, providerLabel } from "@shared/brand/providers";

/** 取得元の印。字形と名前はデスクトップ版と同じ定義（shared/brand）を使う。 */
export function ProviderMark({ source, compact = false }: { source: string; compact?: boolean }) {
  const provider = source.toLowerCase();
  const label = providerLabel(source);
  const glyph = BRAND_GLYPHS[provider] ?? { kind: "word" as const, text: label.slice(0, 2) };
  return <span className={`provider-mark provider-mark--${provider === "fanbox" ? "fanbox" : provider === "pixiv" ? "pixiv" : "other"}${compact ? " provider-mark--compact" : ""}`} aria-label={label}>
    <span className="provider-mark__icon" aria-hidden="true">
      {glyph.kind === "path" ? <span className="brand-word-glyph brand-word-glyph--pixiv"><svg viewBox="0 0 18 18" role="presentation"><path d={glyph.d} /></svg></span>
        : <span className="brand-word-glyph" data-wide={glyph.wide || undefined} data-chars={Math.min(2, [...glyph.text].length)}>{glyph.text}</span>}
    </span>
    {!compact && <span className="provider-mark__label">{label}</span>}
  </span>;
}
