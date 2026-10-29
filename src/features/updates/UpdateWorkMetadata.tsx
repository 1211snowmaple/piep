import { Text } from "@mantine/core";
import { formatDate, parseDate } from "@/lib/format";
import type { UpdateWorkMetadata as Metadata } from "@/services/updateJobApi";
import { formatUpdateSubtitle } from "./UpdateWorkLinks";

export function updateWorkMetadataText(metadata?: Metadata, fallback?: string) {
  const valid = (date: string | null | undefined) => date && Number.isFinite(parseDate(date).getTime()) ? date : null;
  const published = valid(metadata?.publishedAt), updated = valid(metadata?.updatedAt), saved = valid(metadata?.savedAt);
  const parts = [metadata?.authorName];
  if (published) parts.push(`公開 ${formatDate(published)}`);
  if (updated && (!published || parseDate(updated).getTime() !== parseDate(published).getTime())) parts.push(`更新 ${formatDate(updated)}`);
  if (!published && !updated && saved) parts.push(`保存 ${formatDate(saved)}`);
  return parts.filter(Boolean).join(" · ") || (fallback ? formatUpdateSubtitle(fallback) : "作品情報未取得");
}

export function UpdateWorkMetadata({ metadata, fallback }: { metadata?: Metadata; fallback?: string }) {
  const label = updateWorkMetadataText(metadata, fallback);
  return <Text size="xs" c="dimmed" className="identity-name" title={label}>{label}</Text>;
}
