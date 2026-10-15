export type ViewMode = "home" | "pixiv" | "fanbox" | "library" | "epub" | "update" | "settings";

export type WorkScreenMode = "detail" | "reader" | "editor";

export type LibraryEntityView =
  | { type: "person" | "series"; source: string; sourceKey: string }
  | null;
