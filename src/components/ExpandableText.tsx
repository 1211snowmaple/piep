import { useEffect, useRef, useState, type CSSProperties, type MouseEvent, type ReactNode } from "react";
import { Anchor, Box, Group } from "@mantine/core";

/**
 * Enough to tell what a work or a profile is about without the page becoming
 * the caption. Four lines is roughly the first paragraph of the ones people
 * actually write; past that they run to hundreds of lines of release notes,
 * shop links and disclaimers.
 */
const DEFAULT_LINES = 4;

interface ExpandableTextProps {
  /** Sanitised markup, when the source is rich text. Wins over `children`. */
  html?: string;
  children?: ReactNode;
  /** How much shows while collapsed. */
  lines?: number;
  className?: string;
  style?: CSSProperties;
  maw?: number | string;
  mt?: string | number;
  onClick?: (event: MouseEvent<HTMLDivElement>) => void;
  /** Names the thing being expanded, for people who cannot see what moved. */
  label?: string;
}

/**
 * Long prose, folded to a readable height with a way to see the rest.
 *
 * The two ends of this were both wrong: a work's caption was clamped to three
 * lines and simply ended in an ellipsis, with the remainder reachable only on
 * the source site, while an author's profile was printed in full and pushed
 * their works off the bottom of the screen. Prolific creators write both kinds,
 * so neither a fixed clamp nor no clamp works.
 *
 * The control only appears when something is actually hidden - a two line
 * caption with a 続きを読む button under it invites a click that does nothing.
 */
export function ExpandableText({
  html,
  children,
  lines = DEFAULT_LINES,
  className,
  style,
  maw,
  mt,
  onClick,
  label = "全文",
}: ExpandableTextProps) {
  const ref = useRef<HTMLDivElement>(null);
  const [expanded, setExpanded] = useState(false);
  const [overflowing, setOverflowing] = useState(false);

  useEffect(() => {
    // Only meaningful while the clamp is on: expanded, the element is exactly
    // as tall as its contents and would report nothing hidden.
    if (expanded) return;
    const element = ref.current;
    if (!element) return;
    const measure = () => setOverflowing(element.scrollHeight - element.clientHeight > 2);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    // A narrower window reflows the same text into more lines, so whether
    // anything is hidden is a function of the width as much as of the text.
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [children, expanded, html]);

  const body = (
    <Box
      ref={ref}
      className={`expandable-text${className ? ` ${className}` : ""}`}
      data-collapsed={expanded ? undefined : true}
      style={{ "--expandable-lines": lines, ...style } as CSSProperties}
      onClick={onClick}
      {...(html === undefined ? { children } : { dangerouslySetInnerHTML: { __html: html } })}
    />
  );

  return (
    <Box maw={maw} mt={mt} className="expandable">
      {body}
      {/* Right-aligned, where the clipped last line ends, and written as a link
          rather than a button: it reveals text in place, which is what a link
          looks like, and a bordered control here read as an action on the work. */}
      {(overflowing || expanded) && (
        <Group justify="flex-end" mt={2}>
          <Anchor
            component="button"
            type="button"
            size="sm"
            className="expandable__toggle"
            aria-expanded={expanded}
            aria-label={expanded ? `${label}をたたむ` : `${label}を表示`}
            onClick={() => setExpanded((value) => !value)}
          >
            {expanded ? "たたむ" : "続きを読む"}
          </Anchor>
        </Group>
      )}
    </Box>
  );
}
