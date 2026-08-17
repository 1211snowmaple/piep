/** Types shared with the Rust EPUB pipeline. Field names mirror the serde output. */

export interface InfoField {
  key: string;
  label: string;
  enabled: boolean;
}

export interface TemplateSettings {
  label: string;
  description: string;
  /** Sources this template volunteers for when the export picks automatically. */
  appliesTo: string[];
  language: string;
  pageProgression: "ltr" | "rtl";
  includeCoverPage: boolean;
  includeInfoPage: boolean;
  includeNcx: boolean;
  chapterToc: boolean;
  coverInReadingOrder: boolean;
  infoFields: InfoField[];
  strings: Record<string, string>;
}

export interface TemplateInfo {
  name: string;
  isBuiltin: boolean;
  fileCount: number;
  settings: TemplateSettings;
}

export interface TemplateFile {
  filename: string;
  sizeBytes: number;
  /** Whether the file still matches the copy the app ships. */
  customized: boolean;
}

export interface TemplateFileKind {
  filename: string;
  purpose: string;
  language: "css" | "xml";
}

/** One value a template can place, with what it holds for the previewed work. */
export interface DataField {
  path: string;
  group: string;
  label: string;
  sample: string;
  available: boolean;
}

export interface EpubValidationIssue {
  severity: "error" | "warning";
  code: string;
  location: string;
  message: string;
}

export interface EpubValidationReport {
  path: string;
  valid: boolean;
  fileSizeBytes: number;
  issues: EpubValidationIssue[];
}

export interface TemplatePreview {
  sampleTitle: string;
  sampleSource: string;
  sampleDownloadId: number | null;
  css: string;
  cover: string | null;
  info: string | null;
  page: string | null;
  nav: string;
  opf: string;
  ncx: string | null;
  issues: EpubValidationIssue[];
  fields: DataField[];
}

export interface ExportBatchResult {
  successCount: number;
  failedCount: number;
  failedIds: number[];
  /** Generated but not validated; these works remain in the export queue. */
  invalidIds: number[];
  outputFiles: string[];
  invalidCount: number;
  issues: EpubValidationIssue[];
}

export interface ExportProgress {
  phase: string;
  currentTitle: string;
  currentIndex: number;
  totalCount: number;
  message: string;
}
