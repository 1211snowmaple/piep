import { IconSize } from "@/lib/icons";

/**
 * EPUB の書き出しに入れる印。入っていないときの形は `Icons.epubAdd`（lucide の
 * BookPlus）と同じ。
 *
 * 色の付け方はハートに合わせ、入っているときだけ形を塗る。本は四角いので、
 * 輪郭ごと塗るとただの四角になる。塗るのは表紙だけにして、厚みの帯は抜き、
 * 枠と十字の線を上に重ねて、本だと読める形を残す。BookPlus は十字の縦線を
 * 本より先に描くので、塗っても隠れないよう描く順番を変えてある。
 */
export function EpubQueueGlyph({ queued, size = IconSize.action }: { queued: boolean; size?: number }) {
  return (
    <svg
      className="epub-glyph"
      data-on={queued || undefined}
      xmlns="http://www.w3.org/2000/svg"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path className="epub-glyph__cover" stroke="none" d="M4 19.5v-15A2.5 2.5 0 0 1 6.5 2H19a1 1 0 0 1 1 1v14H6.5A2.5 2.5 0 0 0 4 19.5Z" />
      <path className="epub-glyph__pages" stroke="none" d="M6.5 17H20v4a1 1 0 0 1-1 1H6.5a2.5 2.5 0 0 1 0-5Z" />
      <path className="epub-glyph__book" d="M4 19.5v-15A2.5 2.5 0 0 1 6.5 2H19a1 1 0 0 1 1 1v18a1 1 0 0 1-1 1H6.5a1 1 0 0 1 0-5H20" />
      <path className="epub-glyph__plus" d="M12 7v6" />
      <path className="epub-glyph__plus" d="M9 10h6" />
    </svg>
  );
}
