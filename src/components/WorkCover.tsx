import { Box, Image, Text } from "@mantine/core";
import { BookOpen } from "lucide-react";
import { contentTypeLabel } from "@/lib/format";
import { getProvider } from "@/lib/providers";
import { getAssetUrl } from "@/services/dbApi";
import type { DownloadEntry } from "@/types/library";

type WorkCoverData = Pick<DownloadEntry, "coverPath" | "contentType" | "source" | "title">;

export function WorkCover({ work, variant = "card", className }: { work: WorkCoverData; variant?: "compact" | "card" | "detail"; className?: string }) {
  const cover = getAssetUrl(work.coverPath);
  const provider = getProvider(work.source);

  return (
    <Box
      className={["work-cover", `work-cover--${variant}`, className].filter(Boolean).join(" ")}
      data-empty={!cover || undefined}
      style={{ "--work-cover-accent": provider.color }}
    >
      {cover ? (
        <Image src={cover} alt={`${work.title}の表紙`} className="work-cover__image" />
      ) : (
        <Box className="work-cover__empty" aria-label={`${work.title}（表紙なし）`}>
          <BookOpen className="work-cover__empty-icon" strokeWidth={1.35} />
          <Text className="work-cover__empty-kicker">{contentTypeLabel(work.contentType)}</Text>
          {variant === "detail" && <Text className="work-cover__empty-title line-clamp-3">{work.title}</Text>}
        </Box>
      )}
    </Box>
  );
}
