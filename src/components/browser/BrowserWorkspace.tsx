import type { RefObject } from "react";
import { DownloadCandidateSheet } from "@/components/browser/DownloadCandidateSheet";

type SidebarMode = "empty" | "loading" | "analysis" | "downloadProgress" | "downloadDone";

interface SidebarItem {
  id: string;
  title: string;
  subtitle?: string;
  selected: boolean;
  status?: "pending" | "downloading" | "success" | "skipped" | "failed";
}

interface ToastMessage {
  id: number;
  text: string;
  type: "success" | "error" | "info";
}

interface BrowserWorkspaceProps {
  browserRef: RefObject<HTMLDivElement | null>;
  tauriAvailable: boolean;
  showDownloadSidebar: boolean;
  sidebarTitle: string;
  sidebarLoading: boolean;
  sidebarItems: SidebarItem[];
  sidebarProgress: { current: number; total: number } | null;
  sidebarStatusText: string;
  sidebarMode: SidebarMode;
  sidebarEmptyMessage: string;
  isDownloading: boolean;
  toasts: ToastMessage[];
  onSelectAll: (selected: boolean) => void;
  onToggleItem: (id: string) => void;
  onExecute: () => void;
  onOpenLibraryAfterDone: () => void;
}

export function BrowserWorkspace({
  browserRef,
  tauriAvailable,
  showDownloadSidebar,
  sidebarTitle,
  sidebarLoading,
  sidebarItems,
  sidebarProgress,
  sidebarStatusText,
  sidebarMode,
  sidebarEmptyMessage,
  isDownloading,
  toasts,
  onSelectAll,
  onToggleItem,
  onExecute,
  onOpenLibraryAfterDone,
}: BrowserWorkspaceProps) {
  return (
    <div className={`browser-layout-wrapper ${showDownloadSidebar ? "sidebar-open" : ""}`}>
      <div ref={browserRef} className="browser-placeholder">
        {!tauriAvailable && (
          <div className="browser-preview-empty">
            <h3>内蔵ブラウザはTauriアプリ内で利用できます</h3>
            <p>開発ブラウザではレイアウトだけを確認できます。Pixiv/FANBOXの表示、URL追従、保存候補の取得はデスクトップアプリで動作します。</p>
          </div>
        )}
      </div>

      {showDownloadSidebar && (
        <DownloadCandidateSheet
          title={sidebarTitle}
          loading={sidebarLoading}
          items={sidebarItems}
          progress={sidebarProgress}
          statusText={sidebarStatusText}
          mode={sidebarMode}
          emptyMessage={sidebarEmptyMessage}
          isDownloading={isDownloading}
          toasts={toasts}
          onSelectAll={onSelectAll}
          onToggleItem={onToggleItem}
          onExecute={onExecute}
          onOpenLibraryAfterDone={onOpenLibraryAfterDone}
        />
      )}
    </div>
  );
}
