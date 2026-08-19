import type { ReactNode } from "react";
import { Box, Text, type BoxProps } from "@mantine/core";
import { Icons, IconSize } from "@/lib/icons";

export type ProviderId = "pixiv" | "fanbox" | (string & {});

export interface ProviderDefinition {
  id: ProviderId;
  label: string;
  shortLabel: string;
  color: string;
  softColor: string;
  homeUrl: string | null;
  description: string;
  capability: "available" | "planned";
  /**
   * この保存元に「プロフィール画像」という概念があるか。
   *
   * ある場合、置いていない人には保存元自身が「画像なし」の一枚を出す。piep も
   * それに倣う。概念そのものが無い保存元では、無いものを「無い画像」として
   * 見せても嘘になるので、人型やシリーズの記号のままにする。
   */
  hasProfileImages: boolean;
  icon: ReactNode;
}

/**
 * A service's mark, described as data rather than as markup.
 *
 * The same mark has to be drawn by React on the author and save screens and by
 * plain DOM inside saved documents, where a link card is built by hand. Two
 * hand-written copies is how pixiv ended up as its own glyph in one place and a
 * generic arrow in the other.
 */
export type BrandGlyph =
  | { kind: "path"; d: string }
  | { kind: "word"; text: string; wide?: boolean };

export const BRAND_GLYPHS: Record<string, BrandGlyph> = {
  pixiv: { kind: "path", d: "M5.2 14.7V3.3h4.3c2.7 0 4.5 1.7 4.5 4.2s-1.8 4.2-4.5 4.2H7.8v3H5.2Zm2.6-5.3h1.5c1.3 0 2.1-.7 2.1-1.9s-.8-1.9-2.1-1.9H7.8v3.8Z" },
  fanbox: { kind: "word", text: "F", wide: true },
};

/** The glyph markup, for anywhere that builds DOM instead of React. */
export function brandGlyphElement(document: Document, glyph: BrandGlyph): HTMLElement {
  const host = document.createElement("span");
  host.className = "brand-word-glyph";
  if (glyph.kind === "word") {
    if (glyph.wide) host.dataset.wide = "true";
    host.textContent = glyph.text;
    return host;
  }
  host.classList.add("brand-word-glyph--pixiv");
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 18 18");
  svg.setAttribute("role", "presentation");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", glyph.d);
  svg.append(path);
  host.append(svg);
  return host;
}

function renderGlyph(glyph: BrandGlyph) {
  if (glyph.kind === "word") {
    return <span className="brand-word-glyph" data-wide={glyph.wide || undefined}>{glyph.text}</span>;
  }
  return <span className="brand-word-glyph brand-word-glyph--pixiv"><svg viewBox="0 0 18 18" role="presentation"><path d={glyph.d} /></svg></span>;
}

const pixivGlyph = renderGlyph(BRAND_GLYPHS.pixiv);
const fanboxGlyph = renderGlyph(BRAND_GLYPHS.fanbox);

export const providers: Record<string, ProviderDefinition> = {
  pixiv: {
    id: "pixiv",
    label: "pixiv",
    shortLabel: "px",
    color: "#0096FA",
    softColor: "#E8F5FF",
    homeUrl: "https://www.pixiv.net/",
    description: "小説・シリーズ・作者ページ",
    capability: "available",
    hasProfileImages: true,
    icon: pixivGlyph,
  },
  fanbox: {
    id: "fanbox",
    label: "FANBOX",
    shortLabel: "fb",
    color: "#F2C624",
    softColor: "#FFF8D9",
    homeUrl: "https://www.fanbox.cc/",
    description: "投稿・クリエイターページ",
    capability: "available",
    hasProfileImages: true,
    icon: fanboxGlyph,
  },
};

export function getProvider(id: string): ProviderDefinition {
  return providers[id.toLowerCase()] ?? {
    id,
    label: id || "その他",
    shortLabel: (id || "?").slice(0, 2),
    color: "#68707A",
    softColor: "#F1F3F5",
    homeUrl: null,
    description: "外部ソース",
    capability: "planned",
    // 知らない保存元に何があるかは分からない。無い前提で扱う。
    hasProfileImages: false,
    icon: <Icons.link size={IconSize.action} />,
  };
}

interface ProviderMarkProps extends BoxProps {
  provider: string;
  compact?: boolean;
}

export function ProviderMark({ provider, compact = false, className, ...others }: ProviderMarkProps) {
  const item = getProvider(provider);
  return (
    <Box
      component="span"
      className={["provider-mark", className].filter(Boolean).join(" ")}
      style={{ "--provider-color": item.color, "--provider-soft": item.softColor }}
      aria-label={item.label}
      {...others}
    >
      <ProviderGlyph provider={provider} />
      {!compact && <Text component="span" size="xs" fw={700}>{item.label}</Text>}
    </Box>
  );
}

export function sourceUrl(source: string, sourceId: string, contentType?: string, creatorId?: string | null): string | null {
  if (source === "pixiv") {
    if (contentType === "author" || contentType === "person") return `https://www.pixiv.net/users/${sourceId}`;
    if (contentType === "series") return `https://www.pixiv.net/novel/series/${sourceId}`;
    return `https://www.pixiv.net/novel/show.php?id=${sourceId}`;
  }
  if (source === "fanbox") {
    if (contentType === "author" || contentType === "person") return `https://www.fanbox.cc/@${sourceId}`;
    if (creatorId) return `https://www.fanbox.cc/@${creatorId}/posts/${sourceId}`;
    return `https://www.fanbox.cc/posts/${sourceId}`;
  }
  return null;
}

/**
 * Every service the app puts a mark next to, in one list.
 *
 * There used to be two: the providers above, for the services works are saved
 * from, and a separate table here for links found in a profile. pixiv and
 * FANBOX appeared in both, and FANBOX carried a different colour in each - the
 * same service wearing two identities depending on which screen you were on.
 * Anything that is already a provider now takes its label, colour and glyph
 * from that one definition, and only services that are not providers are
 * described here.
 */
const LINK_ONLY_BRANDS = [
  { hosts: ["x.com", "twitter.com"], label: "X", color: "#111111", glyph: "X" },
  { hosts: ["skeb.jp"], label: "Skeb", color: "#30b596", glyph: "Sk" },
  { hosts: ["peing.net"], label: "peing.net", color: "#6b7684", glyph: "Pe" },
  { hosts: ["marshmallow-qa.com"], label: "マシュマロ", color: "#f06292", glyph: "M" },
  { hosts: ["booth.pm"], label: "BOOTH", color: "#fc4d50", glyph: "B" },
  { hosts: ["ci-en.net", "ci-en.dlsite.com"], label: "Ci-en", color: "#e8a13a", glyph: "Ci" },
  { hosts: ["youtube.com", "youtu.be"], label: "YouTube", color: "#ff0033", glyph: "Yt" },
  { hosts: ["misskey.io"], label: "Misskey", color: "#86b300", glyph: "Mi" },
  { hosts: ["bsky.app"], label: "Bluesky", color: "#0a7aff", glyph: "Bs" },
  { hosts: ["note.com"], label: "note", color: "#41c9b4", glyph: "n" },
  { hosts: ["privatter.net"], label: "Privatter", color: "#5c9ee7", glyph: "Pv" },
  { hosts: ["odaibako.net"], label: "お題箱", color: "#7f9cc0", glyph: "Od" },
  { hosts: ["amazon.co.jp", "amazon.jp"], label: "ほしいものリスト", color: "#ff9900", glyph: "Az" },
] as const;

/** Providers first, so a service that the app can save from keeps one identity. */
const PROVIDER_BRAND_HOSTS: Record<string, string[]> = {
  pixiv: ["pixiv.net"],
  fanbox: ["fanbox.cc"],
};

export function ProviderGlyph({ provider }: { provider: string }) {
  return <span className="provider-mark__icon" aria-hidden>{getProvider(provider).icon}</span>;
}

export interface BrandMarkDefinition {
  label: string;
  color: string;
  glyph: string;
  /** Set when this service is one the app can save from. */
  provider: string | null;
  /** The drawn mark, when the service has one of its own. */
  brandGlyph: BrandGlyph | null;
}

function matchesHost(host: string, candidates: readonly string[]): boolean {
  return candidates.some((candidate) => host === candidate || host.endsWith(`.${candidate}`));
}

export function externalBrand(url: string): BrandMarkDefinition {
  let host: string;
  try {
    host = new URL(url).hostname.replace(/^www\./, "").toLowerCase();
  } catch {
    return { label: "Web", color: "#68707a", glyph: "↗", provider: null, brandGlyph: null };
  }
  for (const [id, hosts] of Object.entries(PROVIDER_BRAND_HOSTS)) {
    if (!matchesHost(host, hosts)) continue;
    const provider = getProvider(id);
    // One definition: the same label and colour the save screens use.
    return {
      label: provider.label,
      color: provider.color,
      glyph: provider.shortLabel,
      provider: id,
      brandGlyph: BRAND_GLYPHS[id] ?? null,
    };
  }
  const brand = LINK_ONLY_BRANDS.find((candidate) => matchesHost(host, candidate.hosts));
  return brand
    ? { label: brand.label, color: brand.color, glyph: brand.glyph, provider: null, brandGlyph: { kind: "word", text: brand.glyph } }
    : { label: host, color: "#68707a", glyph: "↗", provider: null, brandGlyph: null };
}

export function ExternalServiceMark({ url }: { url: string }) {
  const brand = externalBrand(url);
  return <Box component="span" className="external-service-mark" style={{ "--external-brand": brand.color }}>{brand.provider ? <ProviderGlyph provider={brand.provider} /> : <span className="external-service-mark__glyph">{brand.glyph}</span>}<Text component="span" size="xs" fw={700}>{brand.label}</Text><Icons.externalLink size={IconSize.inline} /></Box>;
}
