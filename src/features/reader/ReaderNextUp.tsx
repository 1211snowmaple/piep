import { Badge, Box, Text, UnstyledButton } from "@mantine/core";
import { WorkCover } from "@/components/WorkCover";
import { Icons, IconSize } from "@/lib/icons";
import { formatNumber } from "@/lib/format";
import { readReadingPosition } from "@/features/library/readingShelf";
import type { DownloadEntry } from "@/types/library";

/**
 * 黙読の速さの目安（字/分）。日本語の小説でよく言われる 400〜600字/分の
 * なかほど。人によって倍は違うので、「約」を付けた目安としてだけ使う。
 */
export const READING_CHARS_PER_MINUTE = 550;

/** 字数から読む時間の目安（分）。1分未満も「約1分」にする。 */
export function readingMinutes(chars: number): number {
  return Math.max(1, Math.round(chars / READING_CHARS_PER_MINUTE));
}

/** 「約N分」「約1時間20分」。長編で「約140分」と出されても量が掴めない。 */
export function formatReadingTime(chars: number): string {
  const minutes = readingMinutes(chars);
  if (minutes < 60) return `約${minutes}分`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest ? `約${hours}時間${rest}分` : `約${hours}時間`;
}

/**
 * 読み終えたあとに出す、隣の作品の札。
 *
 * 題だけのボタンだったころは、どれが何の話か、どれくらいの長さか、もう
 * 読みかけたものか、が押してみるまで分からなかった。表紙と作者と分量を
 * 並べ、読みかけなら「続きから」と名乗る。
 */
export function NeighborTile({ role, ordered, label, title, work, emphasized, onOpen }: {
  role: "previous" | "next";
  /** 読む順のある並びか。無ければ矢印を付けない（続きのように見せない）。 */
  ordered: boolean;
  label: string;
  title: string;
  /** 詳しい情報。まだ届いていなければ題だけで描く。 */
  work?: DownloadEntry;
  /** 読む順のある並びの「次」。いちばん押されるので目立たせる。 */
  emphasized: boolean;
  onOpen: () => void;
}) {
  const reading = work ? readReadingPosition(work.id, null) !== null : false;
  return (
    <UnstyledButton className="reader-neighbor" data-role={role} data-emphasized={emphasized || undefined} onClick={onOpen} aria-label={`${label}：${title}`}>
      <Box className="reader-neighbor__cover">
        <WorkCover work={work ?? { coverPath: null, source: "pixiv", sourceId: String(title), title, authorName: "" }} variant="compact" />
      </Box>
      <Box className="reader-neighbor__body">
        <Text size="xs" c={emphasized ? "piep" : "dimmed"} fw={700} className="reader-neighbor__label">
          {ordered && role === "previous" && <Icons.previous size={IconSize.inline} />}
          {label}
          {ordered && role === "next" && <Icons.next size={IconSize.inline} />}
        </Text>
        <Text size="sm" fw={700} className="reader-neighbor__title line-clamp-2">{title}</Text>
        {work && (
          <Text size="xs" c="dimmed" className="line-clamp-1">
            {[work.authorName || work.personName, `${formatNumber(work.textLength)}字`, formatReadingTime(work.textLength)].filter(Boolean).join(" · ")}
          </Text>
        )}
        {work?.excerpt && <Text size="xs" c="dimmed" className="reader-neighbor__excerpt line-clamp-2">{work.excerpt}</Text>}
        {reading && <Badge size="xs" variant="light" color="piep" className="reader-neighbor__badge">読みかけ・続きから</Badge>}
      </Box>
    </UnstyledButton>
  );
}

/** 隣が無い側。押せないボタンを並べるより、空いていることだけ伝える。 */
export function NeighborPlaceholder({ label }: { label: string }) {
  return <Box className="reader-neighbor reader-neighbor--empty"><Text size="xs" c="dimmed">{label}</Text></Box>;
}
