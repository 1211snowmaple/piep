import { invoke } from "@tauri-apps/api/core";
import type {
  TemplateFile,
  TemplateFileKind,
  TemplateInfo,
  TemplatePreview,
  TemplateSettings,
} from "@/types/epub";

export function exportEpubBatch<T = void>(payload: Record<string, unknown>): Promise<T> {
  return invoke<T>("export_epub_batch", payload);
}

/** 書き出しを途中でやめる。作りかけの 1 冊は書き切ってから止まる。 */
export function cancelEpubExport(): Promise<void> {
  return invoke<void>("cancel_epub_export");
}

/** `skipMissing` lets a collection with deleted works still be exported, with
 *  those works left out, rather than the export refusing outright. */
export function exportCollectionEpub(collectionId: string, templateName: string, outputDir: string, skipMissing = false): Promise<string> {
  return invoke<string>("export_collection_epub", { collectionId, templateName, outputDir, compressOptions: null, skipMissing });
}

export function listEpubTemplates(): Promise<TemplateInfo[]> {
  return invoke<TemplateInfo[]>("list_epub_templates");
}

export function getTemplateFiles(templateName: string): Promise<TemplateFile[]> {
  return invoke<TemplateFile[]>("get_template_files", { templateName });
}

export function listTemplateFileKinds(): Promise<TemplateFileKind[]> {
  return invoke<TemplateFileKind[]>("list_template_file_kinds");
}

export function readTemplateFile(templateName: string, filename: string): Promise<string> {
  return invoke<string>("read_template_file", { templateName, filename });
}

export function saveTemplateFile(templateName: string, filename: string, content: string): Promise<void> {
  return invoke<void>("save_template_file", { templateName, filename, content });
}

/** Returns the restored contents so the editor can show them without a reload. */
export function resetTemplateFile(templateName: string, filename: string): Promise<string> {
  return invoke<string>("reset_template_file", { templateName, filename });
}

export function createEpubTemplate(templateName: string, baseTemplate?: string): Promise<void> {
  return invoke<void>("create_epub_template", { templateName, baseTemplate });
}

export function renameEpubTemplate(templateName: string, nextName: string): Promise<void> {
  return invoke<void>("rename_epub_template", { templateName, nextName });
}

export function deleteEpubTemplate(templateName: string): Promise<void> {
  return invoke<void>("delete_epub_template", { templateName });
}

export function saveTemplateSettings(templateName: string, settings: TemplateSettings): Promise<TemplateSettings> {
  return invoke<TemplateSettings>("save_template_settings", { templateName, settings });
}

export function previewEpubTemplate(templateName: string, downloadId: number | null): Promise<TemplatePreview> {
  return invoke<TemplatePreview>("preview_epub_template", { templateName, downloadId });
}

