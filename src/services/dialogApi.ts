import {
  ask,
  message,
  open,
  save,
  type DialogFilter,
  type ConfirmDialogOptions,
  type MessageDialogOptions,
  type OpenDialogOptions,
  type SaveDialogOptions,
} from "@tauri-apps/plugin-dialog";

export function askDialog(messageText: string, options?: string | ConfirmDialogOptions): Promise<boolean> {
  return ask(messageText, options);
}

export async function messageDialog(messageText: string, options?: string | MessageDialogOptions): Promise<void> {
  await message(messageText, options);
}

export function openDialog(options?: OpenDialogOptions) {
  return open(options);
}

export async function openSingleDialog(options?: OpenDialogOptions): Promise<string | null> {
  const result = await open({ ...options, multiple: false });
  return typeof result === "string" ? result : null;
}

export function saveDialog(options?: SaveDialogOptions): Promise<string | null> {
  return save(options);
}

export type { ConfirmDialogOptions, DialogFilter, MessageDialogOptions, OpenDialogOptions, SaveDialogOptions };
