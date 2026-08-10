import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Badge, Group, Popover, Text } from "@mantine/core";
import { useAppNavigate } from "@/app/router";

const GAP = 5;

function tagHref(tag: string) {
  return `/library?q=${encodeURIComponent(`tag:${tag}`)}`;
}

/**
 * Shows as many tags as the row can actually hold, with the remainder behind a
 * "+N" chip. A fixed cap wasted the space left over on wide cards and clipped
 * tags on narrow ones, so the count is measured from the rendered widths and
 * re-measured whenever the row resizes.
 */
export function TagRow({ tags, interactive = true }: { tags: string[]; interactive?: boolean }) {
  const navigate = useAppNavigate();
  const rowRef = useRef<HTMLDivElement>(null);
  const [hiddenTags, setHiddenTags] = useState<string[]>([]);

  const measure = useCallback(() => {
    const row = rowRef.current;
    if (!row) return;
    const items = [...row.querySelectorAll<HTMLElement>("[data-tag]")];
    const chip = row.querySelector<HTMLElement>("[data-tag-overflow]");
    if (!items.length) return;
    // Everything has to be laid out before it can be measured.
    items.forEach((item) => item.removeAttribute("data-hidden"));
    chip?.removeAttribute("data-hidden");
    const available = row.clientWidth;
    // Before first layout (and in environments without one) every width reads
    // as 0, which would otherwise collapse the row to a single tag.
    if (available <= 0) return;
    const chipWidth = chip ? chip.offsetWidth + GAP : 0;
    const widths = items.map((item) => item.offsetWidth);

    let used = 0;
    let fits = 0;
    for (let index = 0; index < widths.length; index += 1) {
      const next = used + (index ? GAP : 0) + widths[index];
      const isLast = index === widths.length - 1;
      if (next + (isLast ? 0 : chipWidth) > available) break;
      used = next;
      fits += 1;
    }

    const show = (count: number) => {
      items.forEach((item, index) => {
        if (index >= count) item.setAttribute("data-hidden", "");
        else item.removeAttribute("data-hidden");
      });
      if (count >= items.length) chip?.setAttribute("data-hidden", "");
      else chip?.removeAttribute("data-hidden");
    };

    // Always keep one tag so the row never collapses to just a counter.
    let visible = Math.max(1, fits);
    show(visible);
    // The arithmetic above can drift - most often because the widths were read
    // with a fallback font that the real one later replaces - so the result is
    // checked against the laid-out row and tightened until nothing is clipped.
    let guard = items.length;
    while (visible > 1 && row.scrollWidth > row.clientWidth + 1 && guard > 0) {
      visible -= 1;
      guard -= 1;
      show(visible);
    }

    setHiddenTags((current) => {
      const next = tags.slice(visible);
      return next.length === current.length && next.every((tag, i) => tag === current[i]) ? current : next;
    });
  }, [tags]);

  useLayoutEffect(measure, [measure]);
  useEffect(() => {
    const row = rowRef.current;
    if (!row) return;
    const observer = new ResizeObserver(measure);
    observer.observe(row);
    // Text metrics change when the real font arrives, which happens after the
    // first layout pass.
    document.fonts?.ready.then(measure).catch(() => undefined);
    return () => observer.disconnect();
  }, [measure]);

  if (!tags.length) return null;

  return (
    <div className="work-card__tags" ref={rowRef}>
      {tags.map((tag) => (
        <Badge
          key={tag}
          data-tag
          component="a"
          href={`#${tagHref(tag)}`}
          variant="light"
          color="gray"
          size="xs"
          className="work-card__tag"
          tabIndex={interactive ? undefined : -1}
          aria-hidden={interactive ? undefined : true}
          onClick={(event) => { event.preventDefault(); event.stopPropagation(); navigate(tagHref(tag)); }}
        >
          {tag}
        </Badge>
      ))}
      <TagOverflow tags={hiddenTags} interactive={interactive} />
    </div>
  );
}

function TagOverflow({ tags, interactive }: { tags: string[]; interactive: boolean }) {
  const navigate = useAppNavigate();
  const [opened, setOpened] = useState(false);
  useEffect(() => { if (!interactive) setOpened(false); }, [interactive]);

  return (
    <Popover opened={opened} onChange={setOpened} position="top-end" withArrow shadow="md" withinPortal>
      <Popover.Target>
        <Badge
          data-tag-overflow
          component="button"
          type="button"
          className="work-card__tag-overflow"
          variant="outline"
          color="piep"
          size="xs"
          aria-label={tags.length ? `残りのタグ ${tags.join("、")}` : "残りのタグ"}
          aria-expanded={opened}
          aria-hidden={interactive ? undefined : true}
          tabIndex={interactive ? undefined : -1}
          disabled={!interactive}
          onClick={(event) => { event.stopPropagation(); setOpened((value) => !value); }}
          onKeyDown={(event) => event.stopPropagation()}
        >
          +{tags.length}
        </Badge>
      </Popover.Target>
      <Popover.Dropdown className="work-card__tag-popover" onClick={(event) => event.stopPropagation()}>
        <Text size="xs" c="dimmed" mb={7}>タグで絞り込む</Text>
        <Group gap={6} maw={320}>
          {tags.map((tag) => (
            <Badge component="button" type="button" key={tag} size="sm" variant="light" color="piep" onClick={() => { setOpened(false); navigate(tagHref(tag)); }}>
              #{tag}
            </Badge>
          ))}
        </Group>
      </Popover.Dropdown>
    </Popover>
  );
}
