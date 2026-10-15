const STORAGE_KEY = "piep.dense";

/**
 * Display density is a root attribute the stylesheet reacts to. The settings
 * switch previously only wrote a flag that nothing read, so toggling it had no
 * visible effect at all.
 */
export function applyDensity(dense: boolean) {
  document.documentElement.dataset.density = dense ? "compact" : "comfortable";
  try {
    localStorage.setItem(STORAGE_KEY, String(dense));
  } catch {
    /* Preference is best-effort; the current session still reflects the choice. */
  }
}

export function isDense(): boolean {
  try {
    return localStorage.getItem(STORAGE_KEY) === "true";
  } catch {
    return false;
  }
}

/** Applied once at start-up so the choice survives a restart. */
export function applyStoredDensity() {
  applyDensity(isDense());
}
