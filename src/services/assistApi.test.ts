import { describe, expect, it } from "vitest";
import {
  DEFAULT_ASSIST_SETTINGS,
  assistReady,
  assistTarget,
  normalizeAssistBaseUrl,
  validateAssistSettings,
  type AssistSettings,
} from "@/services/assistApi";

function settings(overrides: Partial<AssistSettings>): AssistSettings {
  return { ...DEFAULT_ASSIST_SETTINGS, ...overrides };
}

describe("assist settings safety", () => {
  it("binds external consent to the exact normalized HTTPS destination", () => {
    const base = settings({
      enabled: true,
      baseUrl: "https://models.example/v1/",
      model: "model-a",
      remoteConsentUrl: "https://models.example/v1",
    });
    base.verifiedTarget = assistTarget(base);
    expect(validateAssistSettings(base)).toBeNull();
    expect(assistReady(base)).toBe(true);

    expect(validateAssistSettings({ ...base, baseUrl: "https://other.example/v1" })).toMatch(/許可/);
    expect(validateAssistSettings({
      ...base,
      baseUrl: "http://models.example/v1",
      remoteConsentUrl: "http://models.example/v1",
      verifiedTarget: "http://models.example/v1\nmodel-a",
    })).toMatch(/HTTPS/);
  });

  it("requires a successful trial for the current URL and model", () => {
    const local = settings({ enabled: true, model: "model-a" });
    expect(validateAssistSettings(local)).toMatch(/試し書き/);
    local.verifiedTarget = assistTarget(local);
    expect(validateAssistSettings(local)).toBeNull();
    expect(assistReady({ ...local, model: "model-b" })).toBe(false);
  });

  it("normalizes only credential-free http endpoints", () => {
    expect(normalizeAssistBaseUrl(" https://models.example/v1/ ")).toBe("https://models.example/v1");
    expect(normalizeAssistBaseUrl("ftp://models.example/v1")).toBeNull();
    expect(normalizeAssistBaseUrl("https://user:secret@models.example/v1")).toBeNull();
    expect(normalizeAssistBaseUrl("https://models.example/v1?token=secret")).toBeNull();
  });
});
