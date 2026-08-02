import type { ReactNode } from "react";
import { Box, Text, type BoxProps } from "@mantine/core";
import { ExternalLink, Share2 } from "lucide-react";

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
  icon: ReactNode;
}

function WordGlyph({ children, wide = false }: { children: ReactNode; wide?: boolean }) {
  return <span className="brand-word-glyph" data-wide={wide || undefined}>{children}</span>;
}

const pixivGlyph = <span className="brand-word-glyph brand-word-glyph--pixiv"><svg viewBox="0 0 18 18" role="presentation"><path d="M5.2 14.7V3.3h4.3c2.7 0 4.5 1.7 4.5 4.2s-1.8 4.2-4.5 4.2H7.8v3H5.2Zm2.6-5.3h1.5c1.3 0 2.1-.7 2.1-1.9s-.8-1.9-2.1-1.9H7.8v3.8Z" /></svg></span>;
const fanboxGlyph = <WordGlyph wide>F</WordGlyph>;

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
    icon: <Share2 size={16} />,
  };
}

interface ProviderMarkProps extends BoxProps {
  provider: string;
  compact?: boolean;
}

export function ProviderMark({ provider, compact = false, ...others }: ProviderMarkProps) {
  const item = getProvider(provider);
  return (
    <Box
      component="span"
      className="provider-mark"
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

const EXTERNAL_BRANDS = [
  { hosts: ["x.com", "twitter.com"], label: "X", color: "#111111", glyph: "X", provider: null },
  { hosts: ["skeb.jp"], label: "Skeb", color: "#30b596", glyph: "Sk", provider: null },
  { hosts: ["fanbox.cc"], label: "FANBOX", color: "#c49300", glyph: "F", provider: "fanbox" },
  { hosts: ["pixiv.net"], label: "pixiv", color: "#0096fa", glyph: "p", provider: "pixiv" },
] as const;

export function ProviderGlyph({ provider }: { provider: string }) {
  return <span className="provider-mark__icon" aria-hidden>{getProvider(provider).icon}</span>;
}

export function externalBrand(url: string) {
  try {
    const host = new URL(url).hostname.replace(/^www\./, "").toLowerCase();
    return EXTERNAL_BRANDS.find((brand) => brand.hosts.some((candidate) => host === candidate || host.endsWith(`.${candidate}`))) ?? { label: host, color: "#68707a", glyph: "↗", provider: null };
  } catch {
    return { label: "Web", color: "#68707a", glyph: "↗", provider: null };
  }
}

export function ExternalServiceMark({ url }: { url: string }) {
  const brand = externalBrand(url);
  return <Box component="span" className="external-service-mark" style={{ "--external-brand": brand.color }}>{brand.provider ? <ProviderGlyph provider={brand.provider} /> : <span className="external-service-mark__glyph">{brand.glyph}</span>}<Text component="span" size="xs" fw={700}>{brand.label}</Text><ExternalLink size={11} /></Box>;
}
