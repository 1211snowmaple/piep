import { useEffect, useMemo, useState } from "react";
import { Box, Image, Text } from "@mantine/core";
import { getProvider } from "@/lib/providers";
import { getAssetUrl } from "@/services/dbApi";
import { coverSigil, titleScale } from "@/lib/coverSigil";
import type { DownloadEntry } from "@/types/library";

type WorkCoverData = Pick<DownloadEntry, "coverPath" | "source" | "sourceId" | "title" | "authorName"> &
  Partial<Pick<DownloadEntry, "personName" | "seriesTitle">>;

/**
 * A work without a cover still has to be findable in a shelf of two thousand.
 *
 * The tile is drawn from the two things every work has: its title, which is
 * what the eye actually reads, and its identity, which becomes a small
 * deterministic mark. The mark's shape comes from the work (so no two tiles
 * repeat) and its colour from the author (so one author's works sit together).
 * At thumbnail size the text is unreadable, so only the mark is drawn.
 */
export function WorkCover({ work, variant = "card", className }: { work: WorkCoverData; variant?: "compact" | "card" | "detail"; className?: string }) {
  const cover = getAssetUrl(work.coverPath);
  const [failed, setFailed] = useState(false);
  const provider = getProvider(work.source);
  const sigil = useMemo(() => coverSigil(work), [work]);

  useEffect(() => setFailed(false), [cover]);

  return (
    <Box
      className={["work-cover", `work-cover--${variant}`, className].filter(Boolean).join(" ")}
      data-empty={!cover || failed || undefined}
      style={{ "--work-cover-accent": provider.color }}
    >
      {cover && !failed ? (
        <Image
          src={cover}
          alt={`${work.title}の表紙`}
          className="work-cover__image"
          loading={variant === "detail" ? "eager" : "lazy"}
          decoding="async"
          onError={() => setFailed(true)}
        />
      ) : (
        <Box
          className="work-cover__empty"
          role="img"
          aria-label={`${work.title}（表紙なし）`}
          style={{ "--work-cover-hue": sigil.hue }}
        >
          <span className="work-cover__sigil" aria-hidden>
            {sigil.cells.map((filled, index) => <i key={index} data-on={filled || undefined} />)}
          </span>
          {/* The author is in the colour, not in a second line of text: the card
              already prints the name right beside this tile. */}
          {variant !== "compact" && (
            <span className="work-cover__caption">
              {work.seriesTitle && <Text className="work-cover__empty-series line-clamp-1">{work.seriesTitle}</Text>}
              <Text className="work-cover__empty-title" data-scale={titleScale(work.title)}>{work.title}</Text>
            </span>
          )}
        </Box>
      )}
    </Box>
  );
}
