import { useCallback, useEffect, useRef } from "react";
import type { MouseEvent } from "react";
import { useAppNavigate } from "@/app/router";
import { contentLinkTarget, normalizeContentLinkUrl } from "@shared/contentLinkTarget";
import type { ContentLinkTarget } from "@shared/contentLinkTarget";
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
/** 手元にあると分かったもの。作品だけは、行き先が画面によって変わる。 */
type ResolvedLink =
  | { kind: "work"; downloadId: number; title: string }
  | { kind: "path"; path: string }
  | null;

/** Whether the library actually holds it, without turning a miss into an error. */
async function resolveTarget(target: ContentLinkTarget): Promise<ResolvedLink> {
  try {
    if (target.kind === "work") {
      const saved = await getDownloadBySource<{ id: number; title?: string }>(target.source, target.sourceId);
      return saved ? { kind: "work", downloadId: saved.id, title: saved.title ?? "" } : null;
    }
    const route = target.kind === "person" ? "people" : "series";
    const path = `/${route}/${encodeURIComponent(target.source)}/${encodeURIComponent(target.sourceKey)}`;
    const entry = target.kind === "person"
      ? await getPerson<unknown>(target.source, target.sourceKey)
      : await getSeries<unknown>(target.source, target.sourceKey);
    return entry ? { kind: "path", path } : null;
  } catch {
    // Not saved, or the lookup failed - either way the outside address is still
    // a working answer, so this is not worth an error message.
    return null;
  }
}

export function contentLinkKey(target: ContentLinkTarget): string {
  return target.kind === "work"
    ? `work:${target.source}:${target.sourceId}`
    : `${target.kind}:${target.source}:${target.sourceKey}`;
}

/**
 * 一度引いた答えを憶えておく置き場。
 *
 * 1つの投稿に同じ作者へのリンクが何本も並ぶことがあり、印を付けるためだけに
 * その数だけ端末へ問い合わせていては、本文が出るのが遅くなる。棚は読んでいる
 * 間にも変わりうるので、しばらく経った答えは引き直す。
 */
const RESOLVE_TTL_MS = 120_000;
const resolveCache = new Map<string, { at: number; value: Promise<ResolvedLink> }>();

function cachedResolve(target: ContentLinkTarget): Promise<ResolvedLink> {
  const key = contentLinkKey(target);
  const hit = resolveCache.get(key);
  if (hit && Date.now() - hit.at < RESOLVE_TTL_MS) return hit.value;
  const value = resolveTarget(target);
  resolveCache.set(key, { at: Date.now(), value });
  // 引けなかったものは憶えておかない。次に開いたときにもう一度試す。
  void value.catch(() => resolveCache.delete(key));
  return value;
}

/** 棚が変わったら忘れる。保存した直後のリンクが、外部リンクのままにならないように。 */
export function forgetContentLinkResolutions(): void {
  resolveCache.clear();
}

async function resolveInApp(target: ContentLinkTarget, workRoute: (id: number) => string): Promise<string | null> {
  const resolved = await cachedResolve(target);
  if (!resolved) return null;
  return resolved.kind === "work" ? workRoute(resolved.downloadId) : resolved.path;
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

const SAVED_BADGE_CLASS = "link-card-saved";

/**
 * 手元にあるものを指しているリンクに、そう見える印を付ける。
 *
 * FANBOX の投稿は、続きの回・別の作者・関連する記事をカードで指す。その宛先が
 * 棚にあるなら、それは外の住所ではなく**この道具の中の場所**なのに、押して
 * みるまで区別が付かなかった。保存できているという手応えは、ここに出る。
 */
export function useLibraryLinkMarks(
  container: React.RefObject<HTMLElement | null>,
  /** 中身が入れ替わったことを知るための印。HTML そのもので構わない。 */
  revision: unknown,
): void {
  useEffect(() => {
    const root = container.current;
    if (!root || !isTauriRuntime()) return;
    let cancelled = false;
    const anchors = [...root.querySelectorAll<HTMLAnchorElement>("a[href], a[data-content-href]")];
    const pending = new Map<string, HTMLAnchorElement[]>();
    for (const anchor of anchors) {
      const raw = anchor.dataset.contentHref ?? anchor.getAttribute("href") ?? "";
      const target = raw ? contentLinkTarget(raw) : null;
      if (!target) continue;
      const key = contentLinkKey(target);
      const group = pending.get(key);
      if (group) group.push(anchor);
      else pending.set(key, [anchor]);
      // 同じ宛先は一度だけ引く。同じ作者を10回指す投稿でも、問い合わせは1回。
      if (!group) {
        void cachedResolve(target).then((resolved) => {
          if (cancelled || !resolved) return;
          for (const element of pending.get(key) ?? []) {
            if (!element.isConnected) continue;
            element.dataset.inLibrary = "1";
            if (element.classList.contains("novel-link-card")) {
              const info = element.querySelector(".link-card-info");
              // 題を持たない埋め込みは、カードに住所そのものが並ぶ。手元に
              // あるものなら、その作品の題を出す ―― `.../show.php?id=90000005`
              // では、それが何なのか読み手には分からない。
              const heading = element.querySelector(".link-card-title");
              if (resolved.kind === "work" && resolved.title && heading && /^https?:\/\/\S+$/.test((heading.textContent ?? "").trim())) {
                heading.textContent = resolved.title;
              }
              if (info && !info.querySelector(`.${SAVED_BADGE_CLASS}`)) {
                const badge = document.createElement("span");
                badge.className = SAVED_BADGE_CLASS;
                badge.textContent = "ライブラリにあります";
                info.append(badge);
              }
            } else {
              element.classList.add("novel-inline-link--saved");
            }
            element.title = "ライブラリに保存済み — アプリ内で開きます";
          }
        });
      }
    }
    return () => { cancelled = true; };
  }, [container, revision]);
}

export { contentLinkTarget };
export type { ContentLinkTarget };
