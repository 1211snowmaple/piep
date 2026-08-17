/**
 * The mark drawn on a work that has no cover image.
 *
 * Two rules, both deterministic: the pattern comes from the work's own
 * identity, so no two tiles in a shelf repeat; the colour comes from its
 * author, so one author's works read as a set. Nothing here depends on
 * render order, time, or machine - the same work always draws the same tile.
 */

/** Hues sit on a 12-step wheel rather than anywhere in 360. */
const HUE_STEPS = 12;
/** Rotated off the primaries so no tile lands on a pure red or green. */
const HUE_OFFSET = 12;

/**
 * FNV-1a, 32-bit.
 *
 * A plain `hash * 31 + code` fold keeps its low bits correlated, and Japanese
 * names sit in a narrow range of code points - nine demo authors landed on
 * four neighbouring greens. The xor-multiply mixes the high bits back down,
 * which is what makes `% 12` spread evenly.
 */
function fnv1a(seed: string): number {
  let hash = 0x811c9dc5;
  for (let index = 0; index < seed.length; index += 1) {
    hash ^= seed.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash;
}

/** One of 12 evenly spaced hues. Saturation and lightness stay fixed in CSS. */
export function authorHue(seed: string): number {
  return (fnv1a(seed) % HUE_STEPS) * (360 / HUE_STEPS) + HUE_OFFSET;
}

/**
 * A 5x5 grid, mirrored left to right so it reads as a mark rather than noise.
 * Only the left three columns carry bits; the outer two are their reflection.
 */
export function sigilCells(seed: string): boolean[] {
  const bits = fnv1a(`${seed}:sigil`);
  const cells: boolean[] = [];
  for (let row = 0; row < 5; row += 1) {
    for (let column = 0; column < 5; column += 1) {
      const mirrored = column > 2 ? 4 - column : column;
      cells.push(((bits >> (row * 3 + mirrored)) & 1) === 1);
    }
  }
  // One work in 32,768 hashes to an empty grid. An empty tile is not a mark.
  if (!cells.some(Boolean)) cells[12] = true;
  return cells;
}

/** Longer titles step down a size rather than clipping mid-word. */
export function titleScale(title: string): "lg" | "md" | "sm" {
  if (title.length <= 12) return "lg";
  if (title.length <= 24) return "md";
  return "sm";
}

export function coverSigil(work: { source: string; sourceId: string; authorName: string; personName?: string | null }) {
  return {
    hue: authorHue(work.personName || work.authorName || work.source),
    cells: sigilCells(`${work.source}:${work.sourceId}`),
  };
}
