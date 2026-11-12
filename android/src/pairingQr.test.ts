import { describe, expect, it } from "vitest";
import { isPairingPayload } from "./pairingQr";

const pairing = { version: 2, role: "reader", url: "https://192.168.0.5:38991", secret: "a".repeat(43), fingerprint: "b".repeat(64) };

describe("読み取った QR の形", () => {
  it("デスクトップ版の登録文字列は接続に回す", () => {
    expect(isPairingPayload(JSON.stringify(pairing))).toBe(true);
    const { fingerprint: _, ...trusted } = pairing;
    expect(isPairingPayload(JSON.stringify(trusted))).toBe(true);
  });

  it("piep の QR ではないものは回さない", () => {
    for (const text of [
      "https://example.com/",
      "WIFI:S:home;T:WPA;P:secret;;",
      "ただの文字",
      "[1,2]",
      "null",
      JSON.stringify({ ...pairing, url: "http://192.168.0.5:38991" }),
      JSON.stringify({ ...pairing, extra: "x" }),
      JSON.stringify({ ...pairing, secret: "" }),
      JSON.stringify({ url: pairing.url }),
    ]) expect(isPairingPayload(text), text).toBe(false);
  });
});
