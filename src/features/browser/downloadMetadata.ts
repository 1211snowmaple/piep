/**
 * Older backend builds and FANBOX response variants can leave one or more
 * `{ body: post }` envelopes around a post. Only unwrap when the outer object
 * is not itself a post, so the actual article `body` remains untouched.
 */
export function normalizeFanboxPostPayload<T>(value: unknown): T {
  let current = value as Record<string, unknown> | null;
  for (let depth = 0; depth < 8; depth += 1) {
    if (!current || typeof current !== "object" || Array.isArray(current) || current.id != null || current.title != null) break;
    const nested = current.post ?? current.data ?? current.result ?? current.body;
    if (!nested || typeof nested !== "object" || Array.isArray(nested)) break;
    current = nested as Record<string, unknown>;
  }
  return current as T;
}
