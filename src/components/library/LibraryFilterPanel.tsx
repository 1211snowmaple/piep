import type { ReactNode } from "react";

export function LibraryFilterPanel({ children }: { children: ReactNode }) {
  return <div className="toolbar-row actions-row">{children}</div>;
}
