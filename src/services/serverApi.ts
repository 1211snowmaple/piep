import { invoke } from "@tauri-apps/api/core";

export interface RemoteServerInfo {
  url: string;
  fingerprint: string | null;
}

export interface ServerSendProgress {
  running: boolean;
  phase: string;
  filesTotal: number;
  filesDone: number;
  bytesTotal: number;
  bytesDone: number;
  message: string | null;
}

export interface ServerLastSend {
  finishedAt: string | null;
  works: number | null;
  filesAdded: number | null;
  error: string | null;
}

export interface DesktopServerStatus {
  remote: RemoteServerInfo | null;
  autoSend: boolean;
  send: ServerSendProgress;
  lastSend: ServerLastSend;
}

export interface RemoteServerStatus {
  version: string;
  apiVersion: number;
  /** 配っている棚。まだ受け取っていなければ null。 */
  shelf: { libraryId: string; syncEpoch: string; works: string } | null;
  /** 自分で更新できるサーバーにだけある。 */
  update: RemoteServerUpdate | null;
  /** ここから下は 0.36 からある。 */
  serverName?: string;
  adminWeb?: "available" | "public-only" | "disabled" | "none";
  /** 送った棚を組み終えて、サーバーの次の起動で配るのを待っている。棚を受け取れるサーバーだけにある。 */
  shelfPending?: boolean;
}

export interface RemoteServerUpdate {
  auto: boolean;
  phase: { state: "idle" | "checking" | "waiting" | "applying"; version?: string };
  latest: string | null;
  lastCheckedAt: string | null;
  /** ここから下は 0.36 からある。 */
  window?: string;
  skipped?: string[];
}

export const serverStatus = () => invoke<DesktopServerStatus>("server_status");
export const serverRemoteStatus = () => invoke<RemoteServerStatus>("server_remote_status");
/** 管理画面へ入る、一度きりのリンクを作る。 */
export const serverAdminLink = () => invoke<string>("server_admin_link");
export const serverConnectRemote = (pairing: string) =>
  invoke<DesktopServerStatus>("server_connect_remote", { pairing });
export const serverDisconnectRemote = () => invoke<DesktopServerStatus>("server_disconnect_remote");
export const serverSetAutoSend = (enabled: boolean) =>
  invoke<DesktopServerStatus>("server_set_auto_send", { enabled });
export const serverSendNow = () => invoke<DesktopServerStatus>("server_send_now");
export const serverSendStop = () => invoke<DesktopServerStatus>("server_send_stop");
