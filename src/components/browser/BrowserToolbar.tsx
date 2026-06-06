import {
  ChevronLeftIcon,
  ChevronRightIcon,
  DownloadIcon,
  HeartIcon,
  PaletteIcon,
  RefreshIcon,
} from "@/components/icons/Icons";
import { goBackEmbeddedBrowser, goForwardEmbeddedBrowser, reloadEmbeddedBrowser } from "@/services/browserApi";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

interface BrowserToolbarProps {
  viewMode: "pixiv" | "fanbox";
  currentUrl: string;
  tauriAvailable: boolean;
  isPixivAuthed: boolean;
  isFanboxAuthed: boolean;
  showDownloadSidebar: boolean;
  isDownloading: boolean;
  downloadButtonText: string;
  onOpenSettings: () => void;
  onDownloadClick: () => void;
}

export function BrowserToolbar({
  viewMode,
  currentUrl,
  tauriAvailable,
  isPixivAuthed,
  isFanboxAuthed,
  showDownloadSidebar,
  isDownloading,
  downloadButtonText,
  onOpenSettings,
  onDownloadClick,
}: BrowserToolbarProps) {
  const unauthed = (viewMode === "pixiv" && !isPixivAuthed) || (viewMode === "fanbox" && !isFanboxAuthed);
  return (
    <div className="browser-toolbar">
      <Button type="button" variant="outline" size="icon" className="toolbar-nav-btn" title="戻る" onClick={() => goBackEmbeddedBrowser()} disabled={!tauriAvailable}>
        <ChevronLeftIcon />
      </Button>
      <Button type="button" variant="outline" size="icon" className="toolbar-nav-btn" title="進む" onClick={() => goForwardEmbeddedBrowser()} disabled={!tauriAvailable}>
        <ChevronRightIcon />
      </Button>
      <Button type="button" variant="outline" size="icon" className="toolbar-nav-btn" title="再読み込み" onClick={() => reloadEmbeddedBrowser()} disabled={!tauriAvailable}>
        <RefreshIcon />
      </Button>
      <div className="url-display">
        {viewMode === "pixiv" ? <PaletteIcon /> : <HeartIcon />}
        {!tauriAvailable ? "Tauriアプリ内でブラウザを表示します" : currentUrl || "読み込み中..."}
      </div>
      {unauthed && (
        <Button type="button" variant="outline" size="sm" className="toolbar-auth-warning" onClick={onOpenSettings} title={`${viewMode === "pixiv" ? "Pixiv" : "FANBOX"}が未連携です。クリックして設定画面で連携してください。`}>
          <Badge variant="secondary">未連携</Badge>
        </Button>
      )}
      <Button
        type="button"
        className={cn("toolbar-download-btn ready", showDownloadSidebar && "active")}
        onClick={onDownloadClick}
        disabled={!tauriAvailable || (isDownloading && !showDownloadSidebar)}
        title={!tauriAvailable ? "保存候補の取得はTauriアプリ内で利用できます" : showDownloadSidebar ? "保存候補を閉じる、または現在のページで更新" : "現在のページから保存候補を取得"}
      >
        <DownloadIcon />
        {downloadButtonText}
      </Button>
    </div>
  );
}
