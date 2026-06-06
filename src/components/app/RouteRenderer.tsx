import { Suspense, type ReactNode } from "react";
import { AppShell, type AppShellProps } from "./AppShell";

type RouteRendererShellProps = Omit<AppShellProps, "children" | "libraryContext">;

interface RouteRendererProps {
  shellProps: RouteRendererShellProps;
  libraryContext?: boolean;
  children: ReactNode;
}

export function ScreenFallback() {
  return (
    <div className="flex h-full min-h-64 w-full items-center justify-center bg-background text-muted-foreground">
      <div className="flex items-center gap-3 rounded-md border bg-card px-4 py-3 text-sm shadow-sm">
        <span className="size-2.5 animate-pulse rounded-full bg-primary" />
        読み込み中...
      </div>
    </div>
  );
}

export function RouteRenderer({ shellProps, libraryContext = false, children }: RouteRendererProps) {
  return (
    <AppShell {...shellProps} libraryContext={libraryContext}>
      <Suspense fallback={<ScreenFallback />}>
        {children}
      </Suspense>
    </AppShell>
  );
}
