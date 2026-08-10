import { useEffect, useRef } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { isTauriRuntime } from "@/services/dbApi";
import { upsertSavedSearch } from "@/services/shelfApi";

const LEGACY_KEY = "piep.saved-searches.v2";
const DONE_KEY = "piep.saved-searches.migrated";

interface LegacyEntry {
  name?: unknown;
  query?: unknown;
  tab?: unknown;
  filters?: unknown;
  sortBy?: unknown;
}

/**
 * Moves saved searches out of browser storage and into the library, once.
 *
 * They were per-install and invisible outside one menu; as sidebar entries they
 * have to live with the works they describe. The legacy key is left in place
 * after a successful move, so an older build opened against the same profile
 * still finds what it wrote.
 */
export function useSavedSearchMigration() {
  const queryClient = useQueryClient();
  const started = useRef(false);

  useEffect(() => {
    if (started.current || !isTauriRuntime()) return;
    started.current = true;

    let store: Storage;
    try {
      store = window.localStorage;
    } catch {
      return;
    }
    if (store.getItem(DONE_KEY) === "1") return;

    let legacy: unknown;
    try {
      legacy = JSON.parse(store.getItem(LEGACY_KEY) ?? "[]");
    } catch {
      store.setItem(DONE_KEY, "1");
      return;
    }
    if (!Array.isArray(legacy) || legacy.length === 0) {
      store.setItem(DONE_KEY, "1");
      return;
    }

    void (async () => {
      let moved = 0;
      // Sequential: the backend enforces a unique name, and racing entries with
      // the same generated name would fight over one row.
      for (const item of legacy.slice(0, 100)) {
        if (!item || typeof item !== "object") continue;
        const entry = item as LegacyEntry;
        const name = typeof entry.name === "string" ? entry.name.trim().slice(0, 80) : "";
        if (!name) continue;
        try {
          await upsertSavedSearch({
            name,
            query: typeof entry.query === "string" && entry.query ? entry.query : null,
            paramsJson: JSON.stringify({ tab: entry.tab, filters: entry.filters, sortBy: entry.sortBy }),
          });
          moved += 1;
        } catch {
          // One unusable entry must not strand the rest, and a failed run has
          // to be retried next time rather than marked done.
          return;
        }
      }
      store.setItem(DONE_KEY, "1");
      if (moved > 0) queryClient.invalidateQueries({ queryKey: ["saved-searches"] });
    })();
  }, [queryClient]);
}
