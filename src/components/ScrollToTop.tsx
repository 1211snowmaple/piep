import { useEffect, useState } from "react";
import { ActionIcon, Tooltip } from "@mantine/core";
import { Icons, IconSize } from "@/lib/icons";

/** Far enough down that the button is wanted, not in the way. */
const REVEAL_AFTER_PX = 900;

/**
 * Returns to the top of a long listing.
 *
 * A library of a few thousand works is many screens tall, and the scroll bar is
 * a poor target after paging through it. The button only exists once there is
 * something to come back from, and it sits bottom-right, clear of the selection
 * bar that centres itself along the bottom edge.
 */
export function ScrollToTop({ containerId = "main-content", offsetBottom }: { containerId?: string; offsetBottom?: number }) {
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    const container = document.getElementById(containerId);
    if (!container) return;
    let frame = 0;
    const evaluate = () => setVisible(container.scrollTop > REVEAL_AFTER_PX);
    const onScroll = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(evaluate);
    };
    evaluate();
    container.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      cancelAnimationFrame(frame);
      container.removeEventListener("scroll", onScroll);
    };
  }, [containerId]);

  if (!visible) return null;
  return (
    <Tooltip label="最上部へ戻る" position="left" withArrow>
      <ActionIcon
        className="scroll-to-top"
        style={offsetBottom ? { bottom: `${offsetBottom}px` } : undefined}
        variant="default"
        radius="xl"
        size={44}
        aria-label="最上部へ戻る"
        onClick={() => {
          const container = document.getElementById(containerId);
          container?.scrollTo({ top: 0, behavior: "smooth" });
        }}
      >
        <Icons.up size={IconSize.nav} />
      </ActionIcon>
    </Tooltip>
  );
}
