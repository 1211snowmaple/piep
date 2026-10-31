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
const MAX_LINE_EDIT_DISTANCE = 1_024;

/**
 * 空白で語が区切られると仮定せずに、日本語の文章を比べる。
 *
 * `Intl.Segmenter` は句読点も空白も残すので、結果をつなげれば必ず元の文に戻る。
 * ふつうの改稿なら、全文に Myers を掛けるのが最小の差分になる。作者が作品全体に
 * 手を入れた（「……」を全部書き換えた、など）ときはその手数の上限を超えるので、
 * 先に行をそろえ、変わった行のまとまりを一つずつ比べる。それでも違いすぎる
 * まとまりは、上限のある範囲の置き換えとして見せる。丸ごと書き直された長い作品で
 * 詳細ページが固まらないように。
 */
export function createTextDiff(beforeText: string, afterText: string): TextDiffResult {
  const before = normalizeNewlines(beforeText);
  const after = normalizeNewlines(afterText);
  if (before === after) return result([{ kind: "equal", value: before }], "word");

  const whole = myers(segment(before), segment(after), MAX_EDIT_DISTANCE);
  if (whole) return result(coalesce(whole), "word");

  const lineParts = diffLines(splitLines(before), splitLines(after));
  const parts: TextDiffPart[] = [];
  let granularity: TextDiffResult["granularity"] = "word";
  let removed: string[] = [];
  let added: string[] = [];
  const refine = (oldText: string, newText: string) => {
    const refined = !oldText || !newText ? null : myers(segment(oldText), segment(newText), MAX_EDIT_DISTANCE);
    if (refined) return parts.push(...refined);
    if (oldText && newText) granularity = "range";
    parts.push(...rangeFallback(oldText, newText));
  };
  const flush = () => {
    if (!removed.length && !added.length) return;
    const oldText = removed.join("");
    const newText = added.join("");
    const whole = !oldText || !newText ? null : myers(segment(oldText), segment(newText), MAX_EDIT_DISTANCE);
    if (whole) parts.push(...whole);
    // Too many edits for one block, but as many lines on each side: every
    // paragraph was edited in place, so compare them one by one.
    else if (removed.length > 1 && removed.length === added.length) removed.forEach((line, index) => refine(line, added[index]));
    else refine(oldText, newText);
    removed = [];
    added = [];
  };
  for (const part of lineParts) {
    // A removed line after added ones starts the next edit, so lines paired
    // one by one are refined pair by pair rather than as one oversized block.
    if (part.kind === "removed" && added.length) flush();
    if (part.kind === "removed") removed.push(part.value);
    else if (part.kind === "added") added.push(part.value);
    else {
      flush();
      parts.push(part);
    }
  }
  flush();
  return result(coalesce(parts), granularity);
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

/** 行末の "\n" は残す。つなげれば元の文に戻るように。 */
function splitLines(value: string): string[] {
  return value.match(/[^\n]*\n|[^\n]+$/g) ?? [];
}

/**
 * Patience diff。両側にちょうど一度ずつ現れる行を錨にしてそろえ、錨のあいだを
 * 再帰的に比べる。
 *
 * 文章の段落はほとんどが一つきりなので、いくつ変わっても速いままである。錨の
 * 無い隙間は、上限のある行単位の Myers に任せる。
 */
function diffLines(before: string[], after: string[]): TextDiffPart[] {
  const output: TextDiffPart[] = [];
  const visit = (beforeStart: number, beforeEnd: number, afterStart: number, afterEnd: number) => {
    let prefixEnd = beforeStart;
    while (prefixEnd < beforeEnd && afterStart + prefixEnd - beforeStart < afterEnd && before[prefixEnd] === after[afterStart + prefixEnd - beforeStart]) prefixEnd += 1;
    for (let index = beforeStart; index < prefixEnd; index += 1) output.push({ kind: "equal", value: before[index] });
    afterStart += prefixEnd - beforeStart;
    beforeStart = prefixEnd;
    let suffix = 0;
    while (beforeEnd - suffix > beforeStart && afterEnd - suffix > afterStart && before[beforeEnd - suffix - 1] === after[afterEnd - suffix - 1]) suffix += 1;
    beforeEnd -= suffix;
    afterEnd -= suffix;

    const anchors = uniqueCommonLines(before, beforeStart, beforeEnd, after, afterStart, afterEnd);
    if (anchors.length) {
      let nextBefore = beforeStart;
      let nextAfter = afterStart;
      for (const [beforeIndex, afterIndex] of anchors) {
        visit(nextBefore, beforeIndex, nextAfter, afterIndex);
        output.push({ kind: "equal", value: before[beforeIndex] });
        nextBefore = beforeIndex + 1;
        nextAfter = afterIndex + 1;
      }
      visit(nextBefore, beforeEnd, nextAfter, afterEnd);
    } else if (beforeStart < beforeEnd || afterStart < afterEnd) {
      const oldLines = before.slice(beforeStart, beforeEnd);
      const newLines = after.slice(afterStart, afterEnd);
      const parts = oldLines.length && newLines.length ? myers(oldLines, newLines, MAX_LINE_EDIT_DISTANCE) : null;
      if (parts) output.push(...parts);
      else if (oldLines.length === newLines.length) {
        // Every paragraph edited in place (an indent added throughout, say)
        // leaves only repeated blank lines to anchor on; pair lines by position.
        oldLines.forEach((value, index) => {
          if (value === newLines[index]) output.push({ kind: "equal", value });
          else output.push({ kind: "removed", value }, { kind: "added", value: newLines[index] });
        });
      } else {
        output.push(...oldLines.map((value) => ({ kind: "removed" as const, value })));
        output.push(...newLines.map((value) => ({ kind: "added" as const, value })));
      }
    }

    for (let index = beforeEnd; index < beforeEnd + suffix; index += 1) output.push({ kind: "equal", value: before[index] });
  };
  visit(0, before.length, 0, after.length);
  return output;
}

/** 両側に一度ずつ現れる行のうち、順序を保って並ぶ最長の列。 */
function uniqueCommonLines(before: string[], beforeStart: number, beforeEnd: number, after: string[], afterStart: number, afterEnd: number): [number, number][] {
  const counts = new Map<string, { before: number; beforeIndex: number; after: number; afterIndex: number }>();
  for (let index = beforeStart; index < beforeEnd; index += 1) {
    const entry = counts.get(before[index]) ?? { before: 0, beforeIndex: index, after: 0, afterIndex: -1 };
    entry.before += 1;
    counts.set(before[index], entry);
  }
  for (let index = afterStart; index < afterEnd; index += 1) {
    const entry = counts.get(after[index]);
    if (!entry) continue;
    entry.after += 1;
    entry.afterIndex = index;
  }
  const pairs = Array.from(counts.values())
    .filter((entry) => entry.before === 1 && entry.after === 1)
    .map((entry): [number, number] => [entry.beforeIndex, entry.afterIndex])
    .sort((left, right) => left[0] - right[0]);

  // Patience sorting finds the longest increasing run of after-indices.
  const tails: number[] = [];
  const previous = Array.from({ length: pairs.length }, () => -1);
  pairs.forEach(([, afterIndex], index) => {
    let low = 0;
    let high = tails.length;
    while (low < high) {
      const middle = (low + high) >> 1;
      if (pairs[tails[middle]][1] < afterIndex) low = middle + 1;
      else high = middle;
    }
    if (low > 0) previous[index] = tails[low - 1];
    tails[low] = index;
  });
  const chain: [number, number][] = [];
  for (let index = tails.length ? tails[tails.length - 1] : -1; index !== -1; index = previous[index]) chain.push(pairs[index]);
  return chain.reverse();
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
