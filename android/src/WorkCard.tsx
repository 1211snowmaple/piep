import { useEffect, useState, type MouseEvent, type ReactNode } from "react";
import { Check, Download, LoaderCircle, RefreshCw } from "lucide-react";
import { Icons } from "./icons";
import { WorkFavoriteButton } from "./FavoriteButton";
import { pcErrorMessage, saveOfflineWork, type PcWork } from "./pcApi";
import { WorkThumbnail } from "./WorkThumbnail";
import { ProviderMark } from "./ProviderMark";
import { MobileTagRow } from "./MobileTagRow";
import { entityCoverUrl, imageRecentlyFailed, savedEntityCoverUrl, SmoothImage } from "./images";
import { useAppNavigation } from "./appHistory";
import { displayDate } from "./workText";
import { useLongPress } from "./longPress";

export function AuthorAvatar({ work, offline, offlineAuthorCover, snapshotGeneration }: { work: PcWork; offline: boolean; offlineAuthorCover: boolean; snapshotGeneration?: number }) {
  const src = offline
    ? offlineAuthorCover ? savedEntityCoverUrl(work.id, "people", snapshotGeneration ?? 0) : null
    : work.authorId ? entityCoverUrl("people", work.source, work.authorId) : null;
  const [failed, setFailed] = useState(() => imageRecentlyFailed(src));
  useEffect(() => { setFailed(imageRecentlyFailed(src)); }, [src]);
  return <span className="work-author-avatar" aria-hidden="true"><Icons.person />{src && !failed && <SmoothImage src={src} alt="" onFailed={() => setFailed(true)} />}</span>;
}

export type SavedState = { changed: boolean };

/**
 * 保存の状態と操作を一つの印にまとめる。デスクトップ版の EPUB の印と同じく、
 * 済んだものは色の付いた印、まだのものは灰色の押せる印にする。
 */
function CardSave({ work, state, onSaved, onProblem }: { work: PcWork; state: SavedState | null; onSaved?: () => void; onProblem: (message: string | null) => void }) {
  const [saving, setSaving] = useState(false);
  if (state && !state.changed && !saving) return <span className="card-action" data-tone="ok" role="img" aria-label="端末に保存済み"><Check aria-hidden="true" /></span>;
  if (!onSaved) return null;
  const save = async () => {
    setSaving(true);
    onProblem(null);
    try { await saveOfflineWork(work.id); onSaved(); }
    catch (error) { onProblem(pcErrorMessage(error)); }
    finally { setSaving(false); }
  };
  const Icon = saving ? LoaderCircle : state?.changed ? RefreshCw : Download;
  const label = saving ? `${work.title}を保存しています` : state?.changed ? `${work.title}の端末の保存を更新` : `${work.title}を端末に保存`;
  return <button type="button" className="card-action" data-tone={state?.changed ? "warn" : undefined} data-busy={saving || undefined} disabled={saving} aria-label={label} onClick={() => void save()}><Icon aria-hidden="true" /></button>;
}

/** カードの余白を押したら読む。作者やタグなど中の操作はそれぞれの行き先を優先する。 */
function isCardControl(target: EventTarget): boolean {
  return target instanceof Element && target.closest("button, a, input, select, summary") !== null;
}

/** 本文の抜き出しで、探した語に印を付ける。英字の大文字・小文字は区別しない。 */
function markTerms(text: string, terms: string[]): ReactNode[] {
  const words = terms.filter(Boolean).map((term) => term.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  if (!words.length) return [text];
  return text.split(new RegExp(`(${words.join("|")})`, "giu")).map((part, index) => index % 2 ? <mark key={index}>{part}</mark> : part);
}

/**
 * デスクトップ版 WorkCard と同じ並び。表紙の横に、シリーズ・題名・作者と取得元・タグ・
 * 事実と操作を置く。デスクトップ版の下の段は端末では高さを取りすぎるので、
 * 本文の列の最後の行に収める。長押しで詳細を開く。
 */
export function WorkCard({ work, onRead, onDetail, offline = false, offlineAuthorCover = false, snapshotGeneration, savedState = null, onSaved, prefix, omit, textHit }: {
  work: PcWork;
  onRead: () => void;
  onDetail: () => void;
  offline?: boolean;
  offlineAuthorCover?: boolean;
  snapshotGeneration?: number;
  savedState?: SavedState | null;
  /** 渡すとカードから保存できる。サーバーにつながっているときだけ。 */
  onSaved?: () => void;
  prefix?: string;
  /** 作者やシリーズの画面では、その名前を全カードで繰り返さない。 */
  omit?: "author" | "series";
  /** 本文で見つかった箇所。押すとそのページを開く。 */
  textHit?: { snippet: string; page: number; pages: number; terms: string[]; onOpen: () => void };
}) {
  const navigation = useAppNavigation();
  const [problem, setProblem] = useState<string | null>(null);
  const press = useLongPress(onDetail);
  const browsable = navigation.canBrowseServer;
  const date = displayDate(work.sourceCreatedAt);
  const authorInner = <><AuthorAvatar work={work} offline={offline} offlineAuthorCover={offlineAuthorCover} snapshotGeneration={snapshotGeneration} /><span>{work.authorName}</span></>;

  return <article className="work-card" {...press} onClick={(event: MouseEvent) => { if (!isCardControl(event.target)) onRead(); }}>
    <div className="work-card-inner">
      <button className="work-card-cover" type="button" tabIndex={-1} aria-label={`${work.title}を読む`} onClick={onRead}><WorkThumbnail work={work} offline={offline} snapshotGeneration={snapshotGeneration} /></button>
      <div className="work-card-body">
        <div className="work-card-main">
          {omit !== "series" && work.seriesTitle && (browsable && work.seriesId
            ? <button className="work-card-series" type="button" aria-label={`シリーズ ${work.seriesTitle} を開く`} onClick={() => navigation.openEntity("series", work.source, work.seriesId!, work.seriesTitle!)}>{work.seriesTitle}</button>
            : <span className="work-card-series">{work.seriesTitle}</span>)}
          <button className="work-card-title" type="button" onClick={onRead} aria-label={`${work.title}を読む`}>{work.title}</button>
          {omit !== "author"
            ? <div className="work-card-identity">
              {browsable && work.authorId
                ? <button className="work-card-author" type="button" aria-label={`作者 ${work.authorName} を開く`} onClick={() => navigation.openEntity("person", work.source, work.authorId, work.authorName)}>{authorInner}</button>
                : <span className="work-card-author">{authorInner}</span>}
              <span className="work-card-divider" aria-hidden="true" />
              <ProviderMark source={work.source} compact />
            </div>
            : <div className="work-card-identity"><ProviderMark source={work.source} compact /></div>}
          {(work.tags?.length ?? 0) > 0 && <div className="work-card-tags"><MobileTagRow tags={work.tags ?? []} onTag={browsable ? navigation.openTag : undefined} onMore={onDetail} /></div>}
          {textHit && <button className="work-card-hit" type="button" aria-label={`本文の ${textHit.page + 1} ページ目を開く`} onClick={textHit.onOpen}>
            <span className="work-card-hit-page">{textHit.page + 1} ページ目{textHit.pages > 1 ? `ほか ${textHit.pages - 1} ページ` : ""}</span>
            <span className="work-card-hit-text">{markTerms(textHit.snippet, textHit.terms)}</span>
          </button>}
        </div>
        <div className="work-card-bottom">
          {problem ? <p className="work-card-problem" role="alert">{problem}</p> : <div className="work-card-facts">
            {prefix && <span className="work-meta work-meta--strong">{prefix}</span>}
            {date && <span className="work-meta" title="公開日"><Icons.publishedDate aria-hidden="true" />{date}</span>}
            <span className="work-meta" title="文字数"><Icons.textLength aria-hidden="true" />{work.textLength.toLocaleString("ja-JP")}字</span>
            {work.assetCount > 0 && <span className="work-meta" title="画像"><Icons.assets aria-hidden="true" />{work.assetCount.toLocaleString("ja-JP")}</span>}
            {work.currentVersion > 1 && <span className="work-version">履歴 {work.currentVersion}版</span>}
          </div>}
          <div className="work-card-actions">
            <button className="card-action" type="button" onClick={onDetail} aria-label={`${work.title}の作品情報`}><Icons.workInfo aria-hidden="true" /></button>
            {!offline && <CardSave work={work} state={savedState} onSaved={onSaved} onProblem={setProblem} />}
            <WorkFavoriteButton work={work} className="card-action" readOnly={offline} />
          </div>
        </div>
      </div>
    </div>
  </article>;
}

export function WorkCardSkeleton() {
  return <div className="work-card work-card--skeleton" aria-hidden="true"><div className="work-card-inner">
    <span className="work-card-cover"><span className="skeleton-block skeleton-cover" /></span>
    <div className="work-card-body"><div className="work-card-main"><span className="skeleton-block skeleton-line skeleton-line--short" /><span className="skeleton-block skeleton-line skeleton-line--title" /><span className="skeleton-block skeleton-line" /><span className="skeleton-block skeleton-line skeleton-line--tags" /></div>
      <div className="work-card-bottom"><span className="skeleton-block skeleton-line skeleton-line--short" /></div></div>
  </div></div>;
}
