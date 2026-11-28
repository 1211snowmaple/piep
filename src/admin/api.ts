/** 管理画面が話す API。同じ口の `/api/admin/*` だけを読む。 */

export class ApiFailure extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
    readonly retryAfterSeconds: number | null = null,
  ) {
    super(message);
  }
}

let lastAction = Date.now();
if (typeof window !== "undefined") {
  for (const name of ["pointerdown", "keydown"]) {
    window.addEventListener(name, () => { lastAction = Date.now(); }, { passive: true, capture: true });
  }
}
/** 操作がない間の読み直しは、操作として数えない。放置した画面でログインを保たないため。 */
const ACTIVE_WINDOW_MS = 60_000;

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {};
  if (body !== undefined) headers["Content-Type"] = "application/json";
  if (method === "GET" && Date.now() - lastAction > ACTIVE_WINDOW_MS) headers["X-Piep-Background"] = "1";
  const response = await fetch(path, {
    method,
    headers,
    credentials: "same-origin",
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!response.ok) {
    let parsed: { code?: string; message?: string; retryAfterSeconds?: number } = {};
    try {
      parsed = await response.json();
    } catch {
      // 本文が JSON でなければ、状態コードだけで伝える。
    }
    throw new ApiFailure(response.status, parsed.code ?? "unknown", parsed.message ?? "処理できませんでした", parsed.retryAfterSeconds ?? null);
  }
  if (response.status === 204 || response.status === 202) return undefined as T;
  return response.json() as Promise<T>;
}

const get = <T>(path: string) => request<T>("GET", path);
const post = <T>(path: string, body?: unknown) => request<T>("POST", path, body);
const put = <T>(path: string, body: unknown) => request<T>("PUT", path, body);

export type UpdatePhase =
  | { state: "idle" | "checking" }
  | { state: "waiting" | "applying"; version: string };

export interface UpdateStatus {
  auto: boolean;
  window: string;
  phase: UpdatePhase;
  latest: string | null;
  lastCheckedAt: string | null;
  lastError: string | null;
  lastAppliedAt: string | null;
  skipped: string[];
  previous: string | null;
}

export interface Device {
  id: string;
  name: string;
  role: "reader" | "admin";
  createdAt: string;
  lastSeenAt: string | null;
  /** 棚を配っているときだけ返る。 */
  readsShelf?: boolean;
}

/** 棚の受け取りの進み具合。 */
export interface ShipmentStatus {
  state: "idle" | "receiving" | "assembling" | "ready" | "failed";
  shipmentId: string | null;
  generation: string | null;
  works: number | null;
  /** 組み終えて立ち上げ直しを頼んだか（偽なら手で立ち上げ直すと切り替わる）。 */
  restarting: boolean | null;
  error: string | null;
}

export interface AdminStatus {
  version: string;
  apiVersion: number;
  /** 配っている棚。まだ受け取っていなければ null。 */
  shelf: { libraryId: string; syncEpoch: string; works: string } | null;
  devices: Device[];
  shipment?: ShipmentStatus;
  publicUrl: string | null;
  /** 起動時に読み込んだ TLS 証明書。TLS が無効なら null。 */
  certificate: { expiresAt: string; source: "files" | "self-signed" } | null;
  update: UpdateStatus | null;
  serverName: string;
  startedAt: string;
  adminWeb: "available" | "public-only" | "disabled" | "none";
  /** 組み終えた棚が、次の起動で配られるのを待っているか。 */
  shelfPending?: boolean;
}

export interface WebState {
  account: boolean;
  authenticated: boolean;
  serverName: string | null;
  username: string | null;
  sessionId: string | null;
}

export interface UpdateHistoryEntry {
  at: string;
  from: string | null;
  to: string;
  result: "applied" | "failed" | "rolled_back";
  downtimeSeconds: number | null;
  reason: string | null;
}

export interface AuditEntry {
  id: number;
  at: number;
  kind: string;
  action: string;
  actor: string;
  ip: string | null;
  ok: boolean;
  detail: string | null;
}

export interface AdminSession {
  id: string;
  createdAt: number;
  lastSeenAt: number;
  expiresAt: number;
  recentAuthAt: number | null;
  ip: string | null;
  label: string | null;
}

export interface Storage {
  contentBytes: number;
  databaseBytes: number;
  indexBytes: number;
  freeBytes: number | null;
}

export type TlsMode = "none" | "self-signed" | "files";

export interface Connection {
  listen: string;
  adminListen: string | null;
  publicUrl: string | null;
  /** 公開の口しかないときに、管理画面だけを出すこの機械の中の口。 */
  localAdmin: string | null;
  tls: TlsMode;
}

/** 動いている設定と、server.toml に書いてある設定。違えば再起動で切り替わる。 */
export interface ConnectionState {
  running: Connection;
  saved: Connection | null;
  problem: string | null;
  /** 管理画面から立ち上げ直せるか。見張り役の下で動いているときだけ。 */
  restartable: boolean;
}

export interface ConnectionChange {
  listen: string;
  adminListen: string | null;
  publicUrl: string;
  tls: TlsMode;
  cert: string | null;
  key: string | null;
}

export interface Pairing {
  payload: string;
  expiresInSeconds: number;
}

export type TicketKind = "login" | "setup";

export const adminApi = {
  webState: () => get<WebState>("/api/admin/web/state"),
  login: (username: string, password: string) => post<unknown>("/api/admin/web/login", { username, password }),
  ticketInfo: (ticket: string) => post<{ kind: TicketKind; username: string | null }>("/api/admin/web/ticket-info", { ticket }),
  useTicket: (ticket: string) => post<unknown>("/api/admin/web/ticket", { ticket }),
  setup: (ticket: string, username: string | null, password: string) => post<unknown>("/api/admin/web/setup", { ticket, username, password }),
  logout: () => post<void>("/api/admin/web/logout"),
  reauth: (password: string) => post<void>("/api/admin/web/reauth", { password }),
  disableWeb: () => post<void>("/api/admin/web/disable"),
  status: () => get<AdminStatus>("/api/admin/status"),
  checkUpdate: () => post<UpdateStatus>("/api/admin/update/check"),
  applyUpdate: () => post<unknown>("/api/admin/update"),
  retrySkipped: () => post<void>("/api/admin/update/retry"),
  rollback: () => post<{ version: string }>("/api/admin/update/rollback"),
  updateHistory: () => get<UpdateHistoryEntry[]>("/api/admin/update/history"),
  setUpdateSettings: (auto: boolean, window: string) => put<unknown>("/api/admin/settings/update", { auto, window }),
  setServerName: (name: string) => put<void>("/api/admin/settings/server-name", { name }),
  changePassword: (password: string) => put<void>("/api/admin/password", { password }),
  sessions: () => get<{ sessions: AdminSession[]; currentId: string | null }>("/api/admin/sessions"),
  endSession: (id: string) => request<void>("DELETE", `/api/admin/sessions/${encodeURIComponent(id)}`),
  revokeDevice: (id: string) => request<void>("DELETE", `/api/admin/devices/${encodeURIComponent(id)}`),
  addReader: (url: string) => post<Pairing>("/api/admin/pairings", { url }),
  replaceAdmin: (url: string) => post<Pairing>("/api/admin/pairings", { url, role: "admin" }),
  storage: () => get<Storage>("/api/admin/storage"),
  connection: () => get<ConnectionState>("/api/admin/connection"),
  saveConnection: (change: ConnectionChange) => put<ConnectionState>("/api/admin/connection", change),
  restart: () => post<void>("/api/admin/restart"),
  audit: (kind: string | null, onlyFailures: boolean, before: number | null) => {
    const query = new URLSearchParams();
    if (kind) query.set("kind", kind);
    if (onlyFailures) query.set("ok", "false");
    if (before !== null) query.set("before", String(before));
    const text = query.toString();
    return get<AuditEntry[]>(`/api/admin/audit${text ? `?${text}` : ""}`);
  },
};

export function failureMessage(error: unknown): string {
  if (error instanceof ApiFailure) return error.message;
  return "サーバーに届きませんでした";
}
