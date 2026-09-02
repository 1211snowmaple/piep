export type TextDiffKind = "equal" | "added" | "removed";

export interface TextDiffPart {
  kind: TextDiffKind;
  value: string;
}

export interface TextDiffResult {
  parts: TextDiffPart[];
  addedCharacters: number;
  removedCharacters: number;
  changed: boolean;
  granularity: "word" | "range";
}

export type DisplayDiffPart = TextDiffPart | { kind: "omitted"; value: string; characters: number };

const MAX_EDIT_DISTANCE = 2_048;

/**
 * Compare Japanese prose without assuming that spaces separate words.
 * `Intl.Segmenter` keeps punctuation and whitespace, so concatenating the
 * result always reconstructs the source exactly. Myers handles ordinary small
 * revisions efficiently; an intentionally bounded range fallback prevents a
 * wholly replaced long work from freezing the detail page.
 */
export function createTextDiff(beforeText: string, afterText: string): TextDiffResult {
  const before = normalizeNewlines(beforeText);
  const after = normalizeNewlines(afterText);
  if (before === after) return result([{ kind: "equal", value: before }], "word");

  const beforeTokens = segment(before);
  const afterTokens = segment(after);
  const parts = myers(beforeTokens, afterTokens, MAX_EDIT_DISTANCE);
  return parts ? result(coalesce(parts), "word") : result(rangeFallback(before, after), "range");
}

/** Keep enough unchanged prose to understand each edit without showing the entire work. */
export function compactTextDiff(parts: TextDiffPart[], contextCharacters = 180): DisplayDiffPart[] {
  const output: DisplayDiffPart[] = [];
  const last = parts.length - 1;
  parts.forEach((part, index) => {
    if (part.kind !== "equal") {
      output.push(part);
      return;
    }
    const characters = Array.from(part.value);
    if (index === 0 && characters.length > contextCharacters) {
      output.push(omitted(characters.length - contextCharacters));
      output.push({ ...part, value: characters.slice(-contextCharacters).join("") });
    } else if (index === last && characters.length > contextCharacters) {
      output.push({ ...part, value: characters.slice(0, contextCharacters).join("") });
      output.push(omitted(characters.length - contextCharacters));
    } else if (characters.length > contextCharacters * 2) {
      output.push({ ...part, value: characters.slice(0, contextCharacters).join("") });
      output.push(omitted(characters.length - contextCharacters * 2));
      output.push({ ...part, value: characters.slice(-contextCharacters).join("") });
    } else {
      output.push(part);
    }
  });
  return output;
}

function normalizeNewlines(value: string): string {
  return value.replace(/\r\n?/g, "\n");
}

function segment(value: string): string[] {
  const Segmenter = (Intl as unknown as {
    Segmenter?: new (locale: string, options: { granularity: "word" }) => { segment(input: string): Iterable<{ segment: string }> };
  }).Segmenter;
  if (Segmenter) {
    const segmenter = new Segmenter("ja", { granularity: "word" });
    return Array.from(segmenter.segment(value), (entry) => entry.segment);
  }
  return Array.from(value);
}

function myers(before: string[], after: string[], limit: number): TextDiffPart[] | null {
  const max = before.length + after.length;
  const distanceLimit = Math.min(max, limit);
  const frontier = new Map<number, number>([[1, 0]]);
  const trace: Map<number, number>[] = [];

  for (let distance = 0; distance <= distanceLimit; distance += 1) {
    trace.push(new Map(frontier));
    for (let diagonal = -distance; diagonal <= distance; diagonal += 2) {
      const down = diagonal === -distance
        || (diagonal !== distance && (frontier.get(diagonal - 1) ?? -1) < (frontier.get(diagonal + 1) ?? -1));
      let x = down ? frontier.get(diagonal + 1) ?? 0 : (frontier.get(diagonal - 1) ?? 0) + 1;
      let y = x - diagonal;
      while (x < before.length && y < after.length && before[x] === after[y]) {
        x += 1;
        y += 1;
      }
      frontier.set(diagonal, x);
      if (x >= before.length && y >= after.length) return backtrack(trace, before, after);
    }
  }
  return null;
}

function backtrack(trace: Map<number, number>[], before: string[], after: string[]): TextDiffPart[] {
  const reversed: TextDiffPart[] = [];
  let x = before.length;
  let y = after.length;

  for (let distance = trace.length - 1; distance >= 0; distance -= 1) {
    const frontier = trace[distance];
    const diagonal = x - y;
    const down = diagonal === -distance
      || (diagonal !== distance && (frontier.get(diagonal - 1) ?? -1) < (frontier.get(diagonal + 1) ?? -1));
    const previousDiagonal = down ? diagonal + 1 : diagonal - 1;
    const previousX = frontier.get(previousDiagonal) ?? 0;
    const previousY = previousX - previousDiagonal;

    while (x > previousX && y > previousY) {
      reversed.push({ kind: "equal", value: before[x - 1] });
      x -= 1;
      y -= 1;
    }
    if (distance === 0) break;
    if (x === previousX) {
      reversed.push({ kind: "added", value: after[y - 1] });
      y -= 1;
    } else {
      reversed.push({ kind: "removed", value: before[x - 1] });
      x -= 1;
    }
  }
  return reversed.reverse();
}

function rangeFallback(before: string, after: string): TextDiffPart[] {
  const oldCharacters = Array.from(before);
  const newCharacters = Array.from(after);
  let prefix = 0;
  while (prefix < oldCharacters.length && prefix < newCharacters.length && oldCharacters[prefix] === newCharacters[prefix]) prefix += 1;
  let suffix = 0;
  while (
    suffix < oldCharacters.length - prefix
    && suffix < newCharacters.length - prefix
    && oldCharacters[oldCharacters.length - 1 - suffix] === newCharacters[newCharacters.length - 1 - suffix]
  ) suffix += 1;

  const parts: TextDiffPart[] = [];
  if (prefix) parts.push({ kind: "equal", value: oldCharacters.slice(0, prefix).join("") });
  const removed = oldCharacters.slice(prefix, oldCharacters.length - suffix).join("");
  const added = newCharacters.slice(prefix, newCharacters.length - suffix).join("");
  if (removed) parts.push({ kind: "removed", value: removed });
  if (added) parts.push({ kind: "added", value: added });
  if (suffix) parts.push({ kind: "equal", value: oldCharacters.slice(oldCharacters.length - suffix).join("") });
  return parts;
}

function coalesce(parts: TextDiffPart[]): TextDiffPart[] {
  const output: TextDiffPart[] = [];
  for (const part of parts) {
    const previous = output[output.length - 1];
    if (previous?.kind === part.kind) previous.value += part.value;
    else output.push({ ...part });
  }
  return output;
}

function result(parts: TextDiffPart[], granularity: TextDiffResult["granularity"]): TextDiffResult {
  const addedCharacters = parts.reduce((count, part) => count + (part.kind === "added" ? Array.from(part.value).length : 0), 0);
  const removedCharacters = parts.reduce((count, part) => count + (part.kind === "removed" ? Array.from(part.value).length : 0), 0);
  return { parts, addedCharacters, removedCharacters, changed: addedCharacters > 0 || removedCharacters > 0, granularity };
}

function omitted(characters: number): DisplayDiffPart {
  return { kind: "omitted", value: `… ${characters.toLocaleString("ja-JP")}字省略 …`, characters };
}
