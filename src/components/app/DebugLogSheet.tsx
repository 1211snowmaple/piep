import type { RefObject } from "react";
import {
  AlertCircle,
  CheckCircle2,
  Info,
  TriangleAlert,
} from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

type LogType = "info" | "success" | "warn" | "error";

interface LogEntry {
  time: string;
  type: LogType;
  text: string;
}

interface DebugLogSheetProps {
  showConsole: boolean;
  logs: LogEntry[];
  consoleBottomRef: RefObject<HTMLDivElement | null>;
  onCloseConsole: () => void;
  onClearLogs: () => void;
  onConsoleScroll: (isUserScrollingUp: boolean) => void;
}

function LogIcon({ type }: { type: LogType }) {
  if (type === "success") return <CheckCircle2 className="size-3.5 text-emerald-500" />;
  if (type === "error") return <AlertCircle className="size-3.5 text-destructive" />;
  if (type === "warn") return <TriangleAlert className="size-3.5 text-amber-500" />;
  return <Info className="size-3.5 text-muted-foreground" />;
}

export function DebugLogSheet({
  showConsole,
  logs,
  consoleBottomRef,
  onCloseConsole,
  onClearLogs,
  onConsoleScroll,
}: DebugLogSheetProps) {
  return (
    <div
      className={cn(
        "fixed inset-x-0 bottom-0 z-50 max-h-[42vh] translate-y-full border-t bg-card/95 shadow-2xl backdrop-blur transition-transform duration-200",
        showConsole && "translate-y-0",
      )}
    >
      <div className="flex items-center justify-between border-b px-4 py-2">
        <div className="flex min-w-0 items-center gap-2">
          <span className="size-2 rounded-full bg-emerald-500 shadow-[0_0_10px_rgba(16,185,129,0.9)]" />
          <h3 className="truncate text-sm font-semibold">デバッグログコンソール</h3>
          <Badge variant="secondary">{logs.length} 件</Badge>
        </div>
        <div className="flex items-center gap-2">
          <Button type="button" variant="outline" size="sm" onClick={onClearLogs}>
            クリア
          </Button>
          <Button type="button" variant="ghost" size="sm" onClick={onCloseConsole}>
            閉じる
          </Button>
        </div>
      </div>
      <div
        className="max-h-[calc(42vh-45px)] overflow-auto px-4 py-3 font-mono text-xs"
        onScroll={(event) => {
          const target = event.currentTarget;
          onConsoleScroll(target.scrollHeight - target.scrollTop - target.clientHeight > 50);
        }}
      >
        {logs.length === 0 ? (
          <div className="rounded-md border border-dashed px-4 py-8 text-center font-sans text-sm text-muted-foreground">
            ログはありません。ダウンロードを開始するとここに詳細な進捗が表示されます。
          </div>
        ) : (
          <div className="space-y-1">
            {logs.map((log, idx) => (
              <div key={idx} className="flex items-start gap-2 rounded px-2 py-1 hover:bg-muted/60">
                <LogIcon type={log.type} />
                <span className="shrink-0 text-muted-foreground">[{log.time}]</span>
                <Badge variant={log.type === "error" ? "destructive" : "outline"} className="h-5 shrink-0 px-1.5 py-0 text-[10px]">
                  {log.type.toUpperCase()}
                </Badge>
                <span className="min-w-0 break-words text-foreground">{log.text}</span>
              </div>
            ))}
            <div ref={consoleBottomRef} />
          </div>
        )}
      </div>
    </div>
  );
}
