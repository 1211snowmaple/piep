import { ActionIcon, Group, Tooltip } from "@mantine/core";
import { Icons, IconSize } from "@/lib/icons";
import { sourceUrl } from "@/lib/providers";
import { formatDate } from "@/lib/format";
import { isTauriRuntime } from "@/services/dbApi";
import { openExternalUrl } from "@/services/openerApi";

/** The provider timestamp is stored in candidate subtitles as ISO 8601. */
export function formatUpdateSubtitle(subtitle: string): string {
  return subtitle.replace(/\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})/g, (date) => formatDate(date, true));
}

export function UpdateWorkLinks({ source, sourceId, downloadId, title, onOpenWork }: {
  source: string;
  sourceId: string;
  downloadId?: number | null;
  title: string;
  onOpenWork?: (downloadId: number) => void;
}) {
  const url = sourceUrl(source, sourceId);
  const openSource = () => {
    if (!url) return;
    if (isTauriRuntime()) void openExternalUrl(url);
    else window.open(url, "_blank", "noopener,noreferrer");
  };
  return <Group gap={2} wrap="nowrap" justify="flex-end" style={{ flex: "0 0 64px" }}>
    {downloadId && onOpenWork && <Tooltip label="保存した作品を開く"><ActionIcon variant="subtle" color="piep" aria-label={`${title}の保存した詳細を開く`} onClick={() => onOpenWork(downloadId)}><Icons.read size={IconSize.action} /></ActionIcon></Tooltip>}
    {url && <Tooltip label="取得元を開く"><ActionIcon variant="subtle" color="gray" aria-label={`${title}の取得元を開く`} onClick={openSource}><Icons.externalLink size={IconSize.action} /></ActionIcon></Tooltip>}
  </Group>;
}
