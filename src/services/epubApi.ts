import { invoke } from "@tauri-apps/api/core";

export function exportEpub(payload: Record<string, unknown>): Promise<void> {
  return invoke<void>("export_epub", payload);
}

export function exportEpubBatch<T = void>(payload: Record<string, unknown>): Promise<T> {
  return invoke<T>("export_epub_batch", payload);
}

export function listEpubTemplates<T>(): Promise<T> {
  return invoke<T>("list_epub_templates");
}

export function getTemplateFiles<T>(templateName: string): Promise<T> {
  return invoke<T>("get_template_files", { templateName });
}

export function readTemplateFile(templateName: string, filename: string): Promise<string> {
  return invoke<string>("read_template_file", { templateName, filename });
}

export function saveTemplateFile(templateName: string, filename: string, content: string): Promise<void> {
  return invoke<void>("save_template_file", { templateName, filename, content });
}

export function createEpubTemplate(templateName: string): Promise<void> {
  return invoke<void>("create_epub_template", { templateName });
}

export function deleteEpubTemplate(templateName: string): Promise<void> {
  return invoke<void>("delete_epub_template", { templateName });
}
