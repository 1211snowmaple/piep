import { invoke } from "@tauri-apps/api/core";
import type { PageWorkCandidate } from "@/features/browser/downloadCandidates";

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

export interface BrowserPageWorks {
  url: string;
  works: PageWorkCandidate[];
  truncated: boolean;
}

/** 表示中のページから、読み込まれている分の作品を読み取る。通信は増えない。 */
export function readBrowserPageWorkCandidates(browser: "embedded" | "standalone", source: "pixiv" | "fanbox"): Promise<BrowserPageWorks> {
  return invoke<BrowserPageWorks>("read_browser_page_work_candidates", { browser, source });
}

export interface PageUserCandidate {
  userId: string;
  name: string;
  watched: boolean;
}

export interface BrowserPageUsers {
  url: string;
  users: PageUserCandidate[];
  truncated: boolean;
}

/** 表示中のフォロー一覧から、読み込まれている分の作者を読み取る。通信は増えない。 */
export function readBrowserPageUserCandidates(browser: "embedded" | "standalone"): Promise<BrowserPageUsers> {
  return invoke<BrowserPageUsers>("read_browser_page_user_candidates", { browser });
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

export interface StandaloneBrowserOptions {
  source: "pixiv" | "fanbox";
  userAgent?: string;
}

/**
 * Opens a large native Tauri WebView window. One window is reused per source,
 * so repeated clicks focus/navigate it instead of producing duplicates.
 * Returns true when an existing window was reused.
 */
export function openStandaloneBrowser(url: string, options: StandaloneBrowserOptions): Promise<boolean> {
  return invoke<boolean>("open_standalone_browser", {
    url,
    source: options.source,
    userAgent: options.userAgent,
  });
}

export function closeStandaloneBrowser(source: StandaloneBrowserOptions["source"]): Promise<boolean> {
  return invoke<boolean>("close_standalone_browser", { source });
}

/**
 * The page the large window is showing, or null when it is not open. Lets the
 * save workspace pick the handover back up after being remounted.
 */
export function getStandaloneBrowserUrl(source: StandaloneBrowserOptions["source"]): Promise<string | null> {
  return invoke<string | null>("get_standalone_browser_url", { source });
}

export interface StandaloneBrowserUrlEvent {
  source: "pixiv" | "fanbox";
  url: string;
}

export interface StandaloneBrowserClosedEvent {
  source: "pixiv" | "fanbox";
  /** Last URL read directly from the closing renderer. */
  url?: string | null;
}

export interface BrowserAcceleratorEvent {
  action: "save" | "close";
  browser: "embedded" | "standalone";
  source: "pixiv" | "fanbox";
  url: string;
}
