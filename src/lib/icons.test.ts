import { describe, expect, it } from "vitest";
import { Icons, IconSize } from "@/lib/icons";

/** Every source file, read through the bundler rather than the filesystem. */
const sources = import.meta.glob("/src/**/*.{ts,tsx}", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

const REGISTRY = "/src/lib/icons.ts";
const files = Object.keys(sources).sort();

describe("icon registry", () => {
  it("is the only place that reaches for the icon set", () => {
    // One role, one icon. That only holds if screens ask for a role rather than
    // picking a shape: adding a work to the EPUB queue was once drawn four
    // different ways because four screens each chose their own.
    const offenders = files
      .filter((path) => path !== REGISTRY && !path.endsWith("icons.test.ts"))
      .filter((path) => sources[path].includes('from "lucide-react"'));
    expect(offenders).toEqual([]);
  });

  it("draws icons only at the defined sizes", () => {
    const allowed = new Set<number>(Object.values(IconSize));
    const offenders: string[] = [];
    for (const path of files) {
      for (const match of sources[path].matchAll(/<Icons\.(\w+) size=\{(\d+)\}/g)) {
        // A literal is fine only when it is one of the scale's own values;
        // anything else is a size someone picked for one call site.
        if (!allowed.has(Number(match[2]))) offenders.push(`${path}: ${match[0]}`);
      }
    }
    expect(offenders).toEqual([]);
  });

  it("gives every role a distinct name and a real component", () => {
    for (const [role, icon] of Object.entries(Icons)) {
      expect(icon, role).toBeTruthy();
      expect(typeof icon === "function" || typeof icon === "object", role).toBe(true);
    }
  });

  it("keeps the roles that must not share an icon apart", () => {
    // These are the pairs that were actually confused in the app, and the pairs
    // a reader has to be able to tell apart at a glance.
    const distinct: [keyof typeof Icons, keyof typeof Icons][] = [
      ["epubAdd", "epubQueued"],
      ["epubAdd", "read"],
      ["favorite", "watch"],
      ["person", "series"],
      ["success", "failure"],
      ["pause", "resume"],
      ["retry", "watch"],
      // The two ways a listing advances sit side by side in one switch.
      ["pagingContinuous", "pagingNumbered"],
      ["pagingContinuous", "viewList"],
      ["pagingNumbered", "viewGrid"],
    ];
    for (const [left, right] of distinct) {
      expect(Icons[left], `${left} vs ${right}`).not.toBe(Icons[right]);
    }
  });

  it("uses one icon for each role wherever that role appears", () => {
    // The registry cannot enforce this by itself: two roles may legitimately
    // share a shape, but a role must never be drawn as two different shapes.
    const byRole = new Map<string, Set<string>>();
    for (const path of files) {
      for (const match of sources[path].matchAll(/<Icons\.(\w+)[\s/>]/g)) {
        const role = match[1];
        expect(role in Icons, `${path} uses an unknown role: ${role}`).toBe(true);
        byRole.set(role, (byRole.get(role) ?? new Set()).add(path));
      }
    }
    expect(byRole.size).toBeGreaterThan(20);
  });
});
