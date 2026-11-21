import { invoke } from "@tauri-apps/api/core";

export interface ServerDevice {
  id: string;
  name: string;
  role: "reader" | "admin";
  createdAt: string;
  lastSeenAt: string | null;
  /** 新しい棚を配っているサーバーだけにある。この携帯が棚を読めるか。 */
  readsShelf?: boolean;
}

export interface EmbeddedServerSettings {
  enabled: boolean;
  ip: string | null;
  behindProxy: boolean;
  publicUrl: string | null;
}

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
  embeddedRunning: boolean;
  embeddedUrl: string | null;
  embeddedFingerprint: string | null;
  embedded: EmbeddedServerSettings;
  devices: ServerDevice[];
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
  publicUrl: string | null;
  /** 自分で更新できるサーバーにだけある。 */
  update: RemoteServerUpdate | null;
  /** ここから下は 0.36 からある。 */
  serverName?: string;
  adminWeb?: "available" | "public-only" | "disabled" | "none";
  devices?: ServerDevice[];
  /** 送った棚を組み終えて、サーバーの次の起動で配るのを待っている。新しい棚を受け取れるサーバーだけにある。 */
  shelfPending?: boolean;
}

export interface RemoteServerUpdate {
  auto: boolean;
  phase: { state: "idle" | "checking" | "waiting" | "applying"; version?: string };
  latest: string | null;
  lastCheckedAt: string | null;
  lastError: string | null;
  lastAppliedAt: string | null;
  /** ここから下は 0.36 からある。 */
  window?: string;
  skipped?: string[];
}

export interface ServerPairing {
  payload: string;
  expiresInSeconds: number;
}

export interface ServerInterface {
  name: string;
  ip: string;
}

export const serverStatus = () => invoke<DesktopServerStatus>("server_status");
export const serverInterfaces = () => invoke<ServerInterface[]>("server_list_interfaces");
export const serverEmbeddedStart = (settings: EmbeddedServerSettings) =>
  invoke<DesktopServerStatus>("server_embedded_start", { settings });
export const serverEmbeddedStop = () => invoke<DesktopServerStatus>("server_embedded_stop");
export const serverCreateReaderPairing = () => invoke<ServerPairing>("server_create_reader_pairing");
export const serverRevokeDevice = (deviceId: string) =>
  invoke<DesktopServerStatus>("server_revoke_device", { deviceId });
export const serverRemoteDevices = () => invoke<ServerDevice[]>("server_list_remote_devices");
/** 単独サーバーの携帯に、新しい棚を読む許可を付け外しする。 */
export const serverSetRemoteShelfReader = (deviceId: string, read: boolean) =>
  invoke<void>("server_set_remote_shelf_reader", { deviceId, read });
export const serverRemoteStatus = () => invoke<RemoteServerStatus>("server_remote_status");
export const serverUpdateRemote = () => invoke<void>("server_update_remote");
export const serverCheckRemoteUpdate = () => invoke<unknown>("server_check_remote_update");
/** 管理画面へ入る、一度きりのリンクを作る。 */
export const serverAdminLink = () => invoke<string>("server_admin_link");
export const serverConnectRemote = (pairing: string) =>
  invoke<DesktopServerStatus>("server_connect_remote", { pairing });
export const serverDisconnectRemote = () => invoke<DesktopServerStatus>("server_disconnect_remote");
export const serverSetAutoSend = (enabled: boolean) =>
  invoke<DesktopServerStatus>("server_set_auto_send", { enabled });
export const serverSendNow = () => invoke<DesktopServerStatus>("server_send_now");
export const serverSendStop = () => invoke<DesktopServerStatus>("server_send_stop");
