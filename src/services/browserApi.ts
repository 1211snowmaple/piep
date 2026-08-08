import { invoke } from "@tauri-apps/api/core";

export interface EmbeddedBrowserBounds {
  x: number;
  y: number;
  width: number;
  height: number;
  userAgent?: string;
}

export function openEmbeddedBrowser(url: string, bounds: EmbeddedBrowserBounds): Promise<void> {
  return invoke<void>("open_embedded_browser", { url, ...bounds, userAgent: bounds.userAgent });
}

/** Moves/resizes the child WebView without touching the page it is showing. */
export function setEmbeddedBrowserBounds(bounds: Omit<EmbeddedBrowserBounds, "userAgent">): Promise<boolean> {
  return invoke<boolean>("set_embedded_browser_bounds", bounds);
}

/**
 * The child WebView is a native layer that always paints above the DOM, so it
 * has to be hidden while an overlay needs to draw over that area.
 */
export function setEmbeddedBrowserVisible(visible: boolean): Promise<boolean> {
  return invoke<boolean>("set_embedded_browser_visible", { visible });
}

export function navigateEmbeddedBrowser(url: string): Promise<void> {
  return invoke<void>("navigate_embedded_browser", { url });
}

export function getEmbeddedBrowserUrl(): Promise<string> {
  return invoke<string>("get_embedded_browser_url");
}

export function closeEmbeddedBrowser(): Promise<void> {
  return invoke<void>("close_embedded_browser");
}

export function destroyEmbeddedBrowser(): Promise<void> {
  return invoke<void>("destroy_embedded_browser");
}

export function goBackEmbeddedBrowser(): Promise<void> {
  return invoke<void>("go_back_embedded_browser");
}

export function goForwardEmbeddedBrowser(): Promise<void> {
  return invoke<void>("go_forward_embedded_browser");
}

export function reloadEmbeddedBrowser(): Promise<void> {
  return invoke<void>("reload_embedded_browser");
}
