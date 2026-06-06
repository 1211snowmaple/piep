import { Badge } from "@/components/ui/badge";
import { Card } from "@/components/ui/card";
import { cn } from "@/lib/utils";
import { getAssetUrl } from "@/services/dbApi";

export interface EntityFacetCardData {
  source: string;
  sourceKey: string;
  displayName: string;
  count: number;
  coverPath: string | null;
  description?: string | null;
  updatedAt?: string | null;
  latestDownloadedAt?: string | null;
  sampleTitle?: string | null;
}

interface EntityFacetGridCardProps {
  facet: EntityFacetCardData;
  type: "person" | "series";
  viewMode: "gallery" | "compact";
  onClick: () => void;
}

function formatDate(iso: string): string {
  try {
    return new Date(iso).toLocaleDateString("ja-JP", {
      year: "numeric",
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  } catch {
    return iso;
  }
}

function entityInitial(value: string): string {
  const trimmed = value.trim();
  return trimmed ? Array.from(trimmed)[0].toUpperCase() : "?";
}

export function EntityFacetGridCard({ facet, type, viewMode, onClick }: EntityFacetGridCardProps) {
  const coverUrl = getAssetUrl(facet.coverPath);
  const typeLabel = type === "person" ? "作者" : "シリーズ";
  const latestLabel = facet.latestDownloadedAt ? formatDate(facet.latestDownloadedAt) : null;
  const checkedLabel = facet.updatedAt ? formatDate(facet.updatedAt) : null;

  return (
    <Card
      className={cn(
        "entity-facet-card",
        viewMode === "gallery" ? "view-gallery" : "view-compact",
        `entity-facet-${type}`,
      )}
      onClick={onClick}
    >
      <div className="entity-facet-cover">
        {coverUrl ? (
          <img src={coverUrl} alt={facet.displayName} />
        ) : (
          <div className={cn("entity-facet-cover-fallback", facet.source)}>
            <span>{entityInitial(facet.displayName)}</span>
            <small>{facet.source === "pixiv" ? "Pixiv" : "FANBOX"}</small>
          </div>
        )}
      </div>
      <div className="entity-facet-body">
        <div className="entity-facet-row">
          <Badge className={cn("source-tag", facet.source)}>
            {facet.source === "pixiv" ? "Pixiv" : "FANBOX"}
          </Badge>
          <Badge variant="secondary" className="entity-facet-type">{typeLabel}</Badge>
        </div>
        <h4 className="entity-facet-title" title={facet.displayName}>
          {facet.displayName}
        </h4>
        {facet.description ? (
          <p className="entity-facet-description" title={facet.description}>{facet.description}</p>
        ) : null}
        {facet.sampleTitle ? (
          <p className="entity-facet-sample" title={facet.sampleTitle}>{facet.sampleTitle}</p>
        ) : null}
        <div className="entity-facet-stat-row">
          <span>{facet.count} 件</span>
          {latestLabel ? <span>最新 {latestLabel}</span> : null}
          {checkedLabel ? <span>確認 {checkedLabel}</span> : null}
        </div>
      </div>
    </Card>
  );
}
