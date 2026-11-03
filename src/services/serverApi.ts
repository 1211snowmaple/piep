import { invoke } from "@tauri-apps/api/core";

export interface ServerDevice {
  id: string;
  name: string;
  role: "reader" | "admin";
  createdAt: string;
  lastSeenAt: string | null;
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
  outsidePaths: number;
  skippedNames: number;
}

export interface ServerLastSend {
  finishedAt: string | null;
  generation: number | null;
  works: number | null;
  filesAdded: number | null;
  filesRemoved: number | null;
  missingReferenced: number | null;
  unchanged: boolean;
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
  works: number | null;
  publicUrl: string | null;
  sync: { generation: number; committedAt: string | null; indexPending: number | null; indexError: string | null } | null;
  /** 自分で更新できるサーバーにだけある。 */
  update: RemoteServerUpdate | null;
}

export interface RemoteServerUpdate {
  auto: boolean;
  phase: { state: "idle" | "checking" | "waiting" | "applying"; version?: string };
  latest: string | null;
  lastCheckedAt: string | null;
  lastError: string | null;
  lastAppliedAt: string | null;
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
export const serverRemoteStatus = () => invoke<RemoteServerStatus>("server_remote_status");
export const serverUpdateRemote = () => invoke<void>("server_update_remote");
export const serverConnectRemote = (pairing: string) =>
  invoke<DesktopServerStatus>("server_connect_remote", { pairing });
export const serverDisconnectRemote = () => invoke<DesktopServerStatus>("server_disconnect_remote");
export const serverSetAutoSend = (enabled: boolean) =>
  invoke<DesktopServerStatus>("server_set_auto_send", { enabled });
export const serverSendNow = () => invoke<DesktopServerStatus>("server_send_now");
export const serverSendStop = () => invoke<DesktopServerStatus>("server_send_stop");
