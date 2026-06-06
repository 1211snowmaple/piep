import type { ReactNode } from "react";
import {
  AlertCircle,
  Heart,
  Home,
  Library,
  Palette,
  Settings,
  TerminalSquare,
} from "lucide-react";
import logo from "@/assets/piep.svg";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { ViewMode } from "@/types/navigation";

type ToastType = "success" | "error" | "info";

interface ToastMessage {
  id: number;
  text: string;
  type: ToastType;
}

interface SidebarNavProps {
  viewMode: ViewMode;
  libraryContext: boolean;
  isPixivAuthed: boolean;
  isFanboxAuthed: boolean;
  showConsole: boolean;
  showDownloadSidebar: boolean;
  toasts: ToastMessage[];
  logsCount: number;
  onNavClick: (mode: ViewMode) => void;
  onBackToLibrary: () => void;
  onToggleConsole: () => void;
}

interface NavButtonProps {
  label: string;
  active?: boolean;
  tone?: "neutral" | "pixiv" | "fanbox" | "epub" | "update";
  icon: ReactNode;
  trailing?: ReactNode;
  inset?: boolean;
  onClick: () => void;
}

const toneClass: Record<NonNullable<NavButtonProps["tone"]>, string> = {
  neutral: "data-[active=true]:bg-slate-200/70 data-[active=true]:text-slate-950 dark:data-[active=true]:bg-slate-800 dark:data-[active=true]:text-slate-50",
  pixiv: "data-[active=true]:bg-sky-100 data-[active=true]:text-sky-700 dark:data-[active=true]:bg-sky-950/60 dark:data-[active=true]:text-sky-300",
  fanbox: "data-[active=true]:bg-yellow-100 data-[active=true]:text-yellow-700 dark:data-[active=true]:bg-yellow-950/50 dark:data-[active=true]:text-yellow-300",
  epub: "data-[active=true]:bg-lime-100 data-[active=true]:text-lime-700 dark:data-[active=true]:bg-lime-950/50 dark:data-[active=true]:text-lime-300",
  update: "data-[active=true]:bg-emerald-100 data-[active=true]:text-emerald-700 dark:data-[active=true]:bg-emerald-950/50 dark:data-[active=true]:text-emerald-300",
};

function NavButton({
  label,
  active = false,
  tone = "neutral",
  icon,
  trailing,
  inset = false,
  onClick,
}: NavButtonProps) {
  return (
    <Button
      type="button"
      variant="ghost"
      data-active={active}
      onClick={onClick}
      className={cn(
        "relative h-9 w-full justify-start gap-2 rounded-md px-3 text-muted-foreground hover:bg-accent hover:text-accent-foreground",
        "data-[active=true]:font-semibold data-[active=true]:shadow-sm",
        "data-[active=true]:before:absolute data-[active=true]:before:left-0 data-[active=true]:before:top-1/2 data-[active=true]:before:h-4 data-[active=true]:before:w-0.5 data-[active=true]:before:-translate-y-1/2 data-[active=true]:before:rounded-r data-[active=true]:before:bg-current",
        inset && "h-8 pl-7 text-xs",
        toneClass[tone],
      )}
    >
      <span className="grid size-4 place-items-center text-current [&_svg]:size-4">{icon}</span>
      <span className="min-w-0 flex-1 truncate text-left">{label}</span>
      {trailing}
    </Button>
  );
}

function AuthDot({ active, label }: { active: boolean; label: string }) {
  return (
    <span
      aria-label={label}
      title={label}
      className={cn(
        "ml-auto size-1.5 rounded-full bg-muted-foreground/40",
        active && "bg-emerald-500 shadow-[0_0_8px_rgba(16,185,129,0.8)]",
      )}
    />
  );
}

function SidebarToastList({ toasts }: { toasts: ToastMessage[] }) {
  if (toasts.length === 0) return null;

  return (
    <div className="absolute bottom-16 left-2 right-2 space-y-2">
      {toasts.map((toast) => (
        <div
          key={toast.id}
          className={cn(
            "flex items-start gap-2 rounded-md border bg-card/95 px-3 py-2 text-xs shadow-lg backdrop-blur",
            toast.type === "error" && "border-destructive/30 text-destructive",
            toast.type === "success" && "border-emerald-500/30 text-emerald-700 dark:text-emerald-300",
          )}
        >
          {toast.type === "error" ? <AlertCircle className="mt-0.5 size-3.5 shrink-0" /> : null}
          <span className="min-w-0 break-words leading-relaxed">{toast.text}</span>
        </div>
      ))}
    </div>
  );
}

export function SidebarNav({
  viewMode,
  libraryContext,
  isPixivAuthed,
  isFanboxAuthed,
  showConsole,
  showDownloadSidebar,
  toasts,
  logsCount,
  onNavClick,
  onBackToLibrary,
  onToggleConsole,
}: SidebarNavProps) {
  const libraryActive = libraryContext || ["library", "epub", "update"].includes(viewMode);
  const openLibrary = libraryContext ? onBackToLibrary : () => onNavClick("library");

  return (
    <aside className="relative z-10 flex w-40 shrink-0 select-none flex-col border-r bg-card/80 backdrop-blur">
      <div className="flex h-20 items-center justify-center px-4">
        <img src={logo} className="h-auto w-28" alt="piep" />
      </div>
      <nav className="flex flex-1 flex-col gap-1 px-2 pb-3">
        <NavButton label="ホーム" active={!libraryContext && viewMode === "home"} icon={<Home />} onClick={() => onNavClick("home")} />
        <NavButton label="ライブラリ" active={libraryActive} icon={<Library />} onClick={openLibrary} />
        <NavButton
          label="Pixiv"
          active={!libraryContext && viewMode === "pixiv"}
          tone="pixiv"
          icon={<Palette />}
          onClick={() => onNavClick("pixiv")}
          trailing={<AuthDot active={isPixivAuthed} label={isPixivAuthed ? "Pixiv 連携済み" : "Pixiv 未連携"} />}
        />
        <NavButton
          label="FANBOX"
          active={!libraryContext && viewMode === "fanbox"}
          tone="fanbox"
          icon={<Heart />}
          onClick={() => onNavClick("fanbox")}
          trailing={<AuthDot active={isFanboxAuthed} label={isFanboxAuthed ? "FANBOX 連携済み" : "FANBOX 未連携"} />}
        />
        <NavButton label="設定" active={!libraryContext && viewMode === "settings"} icon={<Settings />} onClick={() => onNavClick("settings")} />
      </nav>
      <div className="px-2 pb-3">
        <Button
          type="button"
          variant={showConsole ? "secondary" : "ghost"}
          className="h-9 w-full justify-start gap-2 text-xs"
          onClick={onToggleConsole}
        >
          <TerminalSquare className="size-4" />
          LOGS
          {logsCount > 0 ? <Badge variant="secondary" className="ml-auto px-1.5">{logsCount}</Badge> : null}
        </Button>
      </div>
      {!showDownloadSidebar ? <SidebarToastList toasts={toasts} /> : null}
    </aside>
  );
}
