import { DownloadIcon, LibraryIcon } from "@/components/icons/Icons";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { Progress } from "@/components/ui/progress";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

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

interface DownloadCandidateSheetProps {
  title: string;
  loading: boolean;
  items: SidebarItem[];
  progress: { current: number; total: number } | null;
  statusText: string;
  mode: SidebarMode;
  emptyMessage: string;
  isDownloading: boolean;
  toasts: ToastMessage[];
  onSelectAll: (selected: boolean) => void;
  onToggleItem: (id: string) => void;
  onExecute: () => void;
  onOpenLibraryAfterDone: () => void;
}

function itemStatusLabel(status: SidebarItem["status"]): string {
  if (status === "pending") return "待機中";
  if (status === "downloading") return "保存中";
  if (status === "success") return "保存完了";
  if (status === "skipped") return "保存済み";
  if (status === "failed") return "失敗";
  return "";
}

function itemStatusVariant(status: SidebarItem["status"]): "default" | "secondary" | "outline" | "destructive" {
  if (status === "failed") return "destructive";
  if (status === "success") return "default";
  if (status === "downloading" || status === "pending") return "secondary";
  return "outline";
}

export function DownloadCandidateSheet({
  title,
  loading,
  items,
  progress,
  statusText,
  mode,
  emptyMessage,
  isDownloading,
  toasts,
  onSelectAll,
  onToggleItem,
  onExecute,
  onOpenLibraryAfterDone,
}: DownloadCandidateSheetProps) {
  const selectedCount = items.filter(item => item.selected).length;
  const progressValue = progress && progress.total > 0 ? Math.round((progress.current / progress.total) * 100) : 0;

  return (
    <aside className="download-option-sidebar">
      <div className="sidebar-option-header">
        <div className="sidebar-option-title-row">
          <h3 className="sidebar-option-title" title={title}>{title || "読み込み中..."}</h3>
          {items.length > 0 ? <Badge variant="outline">{selectedCount} / {items.length}</Badge> : null}
        </div>
        {!loading && items.length > 0 && !progress && (
          <div className="sidebar-option-actions">
            <Button type="button" variant="outline" size="sm" onClick={() => onSelectAll(true)}>すべて選択</Button>
            <Button type="button" variant="outline" size="sm" onClick={() => onSelectAll(false)}>すべて解除</Button>
          </div>
        )}
      </div>

      <div className="sidebar-option-list-container">
        {loading ? (
          <div className="skeleton-list">
            {[1, 2, 3, 4].map(n => (
              <div className="skeleton-item" key={n}>
                <Skeleton className="skeleton-checkbox" />
                <div className="skeleton-info">
                  <Skeleton className="skeleton-title" />
                  <Skeleton className="skeleton-subtitle" />
                </div>
              </div>
            ))}
          </div>
        ) : items.length === 0 ? (
          <div className="sidebar-empty-state">
            <p>{emptyMessage || "作品が見つかりませんでした。"}</p>
          </div>
        ) : (
          items.map(item => (
            <Card
              key={item.id}
              role="button"
              tabIndex={0}
              className={cn("sidebar-option-item", item.selected && "checked", item.status && `status-${item.status}`)}
              onClick={() => !isDownloading && onToggleItem(item.id)}
              onKeyDown={(event) => {
                if ((event.key === "Enter" || event.key === " ") && !isDownloading) {
                  event.preventDefault();
                  onToggleItem(item.id);
                }
              }}
            >
              <div className="sidebar-option-checkbox-wrapper">
                <Checkbox className="sidebar-option-checkbox" checked={item.selected} disabled={isDownloading} aria-label={item.title} />
              </div>
              <div className="sidebar-option-info">
                <div className="sidebar-option-text">
                  <h4 className="sidebar-option-item-title">{item.title}</h4>
                  {item.subtitle && <p className="sidebar-option-item-subtitle">{item.subtitle}</p>}
                </div>
                {item.status && (
                  <Badge variant={itemStatusVariant(item.status)} className={`sidebar-option-item-status status-badge-${item.status}`}>
                    {itemStatusLabel(item.status)}
                  </Badge>
                )}
              </div>
            </Card>
          ))
        )}
      </div>

      <div className="sidebar-option-footer">
        {isDownloading && progress ? (
          <div className="sidebar-progress-container">
            <div className="sidebar-progress-label">
              <span>{statusText || "ダウンロード中..."}</span>
              <span>{progress.current} / {progress.total}</span>
            </div>
            <Progress value={progressValue} />
          </div>
        ) : progress ? (
          <Button type="button" className="sidebar-option-install-btn success-view" onClick={onOpenLibraryAfterDone}>
            <LibraryIcon />
            ライブラリへ移動して読む
          </Button>
        ) : mode === "empty" ? null : (
          <Button type="button" className="sidebar-option-install-btn" onClick={onExecute} disabled={loading || selectedCount === 0}>
            <DownloadIcon />
            選択した {selectedCount} 件を保存
          </Button>
        )}
      </div>
      {toasts.length > 0 && (
        <div className="sidebar-toast-container">
          {toasts.map(toast => (
            <div key={toast.id} className={`sidebar-toast ${toast.type}`}>
              <span className="toast-text-content">{toast.text}</span>
            </div>
          ))}
        </div>
      )}
    </aside>
  );
}
