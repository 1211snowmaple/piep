// Settle the colour scheme before the first paint. Mantine does the same once
// React is running, which is too late for the screen shown while it loads.
(function () {
  var stored = null;
  try {
    stored = window.localStorage.getItem("mantine-color-scheme-value");
  } catch (_error) {
    // Storage can be unavailable; the system setting still gives the right colour.
  }
  var dark = stored === "dark"
    || (stored !== "light" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  document.documentElement.setAttribute("data-mantine-color-scheme", dark ? "dark" : "light");
})();
