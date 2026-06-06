import type { RefObject, ReactNode } from "react";
import { DebugLogSheet } from "@/components/app/DebugLogSheet";
import { SidebarNav } from "@/components/app/SidebarNav";
import type { ViewMode } from "@/types/navigation";

type ToastType = "success" | "error" | "info";
type LogType = "info" | "success" | "warn" | "error";

interface ToastMessage {
  id: number;
  text: string;
  type: ToastType;
}

interface LogEntry {
  time: string;
  type: LogType;
  text: string;
}

export interface AppShellProps {
  children: ReactNode;
  viewMode: ViewMode;
  libraryContext?: boolean;
  isPixivAuthed: boolean;
  isFanboxAuthed: boolean;
  showConsole: boolean;
  showDownloadSidebar: boolean;
  toasts: ToastMessage[];
  logs: LogEntry[];
  consoleBottomRef: RefObject<HTMLDivElement | null>;
  onNavClick: (mode: ViewMode) => void;
  onBackToLibrary: () => void;
  onToggleConsole: () => void;
  onCloseConsole: () => void;
  onClearLogs: () => void;
  onConsoleScroll: (isUserScrollingUp: boolean) => void;
}

export function AppShell({
  children,
  viewMode,
  libraryContext = false,
  isPixivAuthed,
  isFanboxAuthed,
  showConsole,
  showDownloadSidebar,
  toasts,
  logs,
  consoleBottomRef,
  onNavClick,
  onBackToLibrary,
  onToggleConsole,
  onCloseConsole,
  onClearLogs,
  onConsoleScroll,
}: AppShellProps) {
  return (
    <div className="flex h-screen w-screen overflow-hidden bg-background text-foreground">
      <SidebarNav
        viewMode={viewMode}
        libraryContext={libraryContext}
        isPixivAuthed={isPixivAuthed}
        isFanboxAuthed={isFanboxAuthed}
        showConsole={showConsole}
        showDownloadSidebar={showDownloadSidebar}
        toasts={toasts}
        logsCount={logs.length}
        onNavClick={onNavClick}
        onBackToLibrary={onBackToLibrary}
        onToggleConsole={onToggleConsole}
      />

      <main className="relative flex min-w-0 flex-1 flex-col overflow-hidden bg-background">
        {children}
      </main>

      <DebugLogSheet
        showConsole={showConsole}
        logs={logs}
        consoleBottomRef={consoleBottomRef}
        onCloseConsole={onCloseConsole}
        onClearLogs={onClearLogs}
        onConsoleScroll={onConsoleScroll}
      />
    </div>
  );
}
