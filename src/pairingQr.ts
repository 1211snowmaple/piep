/**
 * 読み取った QR が、デスクトップ版の「携帯を追加」が出す登録文字列の形をしているか。
 *
 * 形が合うものだけを読み取りのあとそのまま接続に回す。中身の正しさ（版・役割・URL・鍵の長さ・証明書）は
 * Rust 側の `parse_pairing` が通信の前に確かめるので、ここでは「piep の QR ではないもの」
 * （ほかのサイトの URL、ただの文字、別アプリの QR）を接続に回さないことだけを受け持つ。
 */
export function isPairingPayload(text: string): boolean {
  let value: unknown;
  try { value = JSON.parse(text); } catch { return false; }
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const record = value as Record<string, unknown>;
  const allowed = new Set(["version", "role", "url", "secret", "fingerprint"]);
  if (Object.keys(record).some((key) => !allowed.has(key))) return false;
  return typeof record.version === "number"
    && typeof record.role === "string"
    && typeof record.url === "string" && record.url.startsWith("https://")
    && typeof record.secret === "string" && record.secret.length > 0
    && (record.fingerprint === undefined || typeof record.fingerprint === "string");
}
