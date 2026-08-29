import { useCallback, useRef } from "react";
import type { MouseEvent } from "react";
import { useAppNavigate } from "@/app/router";
import { normalizeContentLinkUrl } from "@/features/browser/downloadCandidates";
import { getDownloadBySource, getPerson, getSeries, isTauriRuntime } from "@/services/dbApi";
import { openExternalUrl } from "@/services/openerApi";

/**
 * Something in the library that a link inside saved text is pointing at.
 *
 * Captions are full of them: the series this chapter belongs to, the author's
 * other account, the earlier part. When the reader has saved that thing, the
 * address is not really an outside address at all - it names a screen in this
 * app, and sending them to a browser to look at their own library is the wrong
 * answer.
 */
export type ContentLinkTarget =
  | { kind: "work"; source: string; sourceId: string }
  | { kind: "person"; source: string; sourceKey: string }
  | { kind: "series"; source: string; sourceKey: string };

/** The creator subdomain, when a FANBOX address carries one. */
function fanboxCreator(url: URL): string | null {
  const host = url.hostname.replace(/^www\./, "");
  if (host.endsWith(".fanbox.cc")) return host.slice(0, -".fanbox.cc".length) || null;
  const inPath = url.pathname.match(/^\/@([^/]+)/);
  return inPath?.[1] ?? null;
}

/**
 * What a link inside saved text names, if it names anything the app holds.
 *
 * pixiv writes its own captions with `pixiv://` deep links for the app, so the
 * addresses that most want resolving are the ones a browser cannot open either.
 */
export function contentLinkTarget(raw: string): ContentLinkTarget | null {
  let url: URL;
  try {
    url = new URL(normalizeContentLinkUrl(raw.trim()));
  } catch {
    return null;
  }
  // 取得元の内部リンクとして扱うのは、ブラウザで同じサービスへ安全に渡せる
  // 形だけ。`http:` や資格情報付きの見た目だけ FANBOX の URL を、手元の作品へ
  // 解決して本物らしく見せない。
  if (url.protocol !== "https:" || url.port || url.username || url.password) return null;
  const host = url.hostname.replace(/^www\./, "");

  if (host === "pixiv.net") {
    const series = url.pathname.match(/\/novel\/series\/(\d+)/);
    if (series) return { kind: "series", source: "pixiv", sourceKey: series[1] };
    const user = url.pathname.match(/^\/(?:[a-z]{2}\/)?users\/(\d+)/);
    if (user) return { kind: "person", source: "pixiv", sourceKey: user[1] };
    const novelId = url.pathname.match(/^\/(?:[a-z]{2}\/)?novel\/show\.php$/) ? url.searchParams.get("id") : null;
    const novelPath = url.pathname.match(/^\/(?:[a-z]{2}\/)?novels?\/(\d+)/);
    const id = novelId ?? novelPath?.[1];
    if (id && /^\d+$/.test(id)) return { kind: "work", source: "pixiv", sourceId: id };
    return null;
  }

  if (host === "fanbox.cc" || host.endsWith(".fanbox.cc")) {
    const post = url.pathname.match(/\/posts\/(\d+)/);
    if (post) return { kind: "work", source: "fanbox", sourceId: post[1] };
    const creator = fanboxCreator(url);
    if (creator) return { kind: "person", source: "fanbox", sourceKey: creator };
    return null;
  }

  return null;
}

/** Whether the library actually holds it, without turning a miss into an error. */
async function resolveInApp(target: ContentLinkTarget, workRoute: (id: number) => string): Promise<string | null> {
  try {
    if (target.kind === "work") {
      const saved = await getDownloadBySource<{ id: number }>(target.source, target.sourceId);
      return saved ? workRoute(saved.id) : null;
    }
    const route = target.kind === "person" ? "people" : "series";
    const path = `/${route}/${encodeURIComponent(target.source)}/${encodeURIComponent(target.sourceKey)}`;
    const entry = target.kind === "person"
      ? await getPerson<unknown>(target.source, target.sourceKey)
      : await getSeries<unknown>(target.source, target.sourceKey);
    return entry ? path : null;
  } catch {
    // Not saved, or the lookup failed - either way the outside address is still
    // a working answer, so this is not worth an error message.
    return null;
  }
}

/**
 * Follows a link inside saved text.
 *
 * Attached to the container rather than to each anchor, because the markup is
 * set as HTML and has no React handlers of its own.
 */
export function useContentLinkNavigation(options: {
  /**
   * Which screen a saved work opens on. The reader stays in the reader: someone
   * following "前編はこちら" mid-chapter wants to keep reading, not to be handed
   * a detail screen.
   */
  workRoute?: (id: number) => string;
} = {}) {
  const navigate = useAppNavigate();
  // 既定の行き先は毎レンダー新しい関数になる。そのまま依存に置くと、
  // 返すハンドラの同一性が毎回変わり、貼り替えが起きつづける。最新の値だけを
  // 参照で持ち回る。
  const workRouteRef = useRef(options.workRoute);
  workRouteRef.current = options.workRoute;
  return useCallback(async (event: MouseEvent<HTMLElement>) => {
    const anchor = (event.target as HTMLElement).closest<HTMLAnchorElement>("a[href], a[data-content-href]");
    if (!anchor) return;
    // pixiv's own deep links cannot be given an href - no browser would accept
    // the scheme - so the address they carry is kept beside it.
    const raw = anchor.dataset.contentHref ?? anchor.href;
    if (!raw) return;
    event.preventDefault();
    const runtime = isTauriRuntime();
    const target = contentLinkTarget(raw);
    if (target && runtime) {
      const workRoute = workRouteRef.current ?? ((id: number) => `/works/${id}`);
      const path = await resolveInApp(target, workRoute);
      if (path) {
        navigate(path);
        return;
      }
    }
    const external = normalizeContentLinkUrl(raw);
    if (!/^https?:/i.test(external)) return;
    if (runtime) await openExternalUrl(external); else window.open(external, "_blank", "noopener,noreferrer");
  }, [navigate]);
}
