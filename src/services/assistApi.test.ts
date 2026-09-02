import { describe, expect, it } from "vitest";
import {
  DEFAULT_ASSIST_SETTINGS,
  assistReady,
  assistFeatureReady,
  assistTarget,
  normalizeAssistBaseUrl,
  toEngine,
  validateAssistSettings,
  type AssistSettings,
} from "@/services/assistApi";

function settings(overrides: Partial<AssistSettings>): AssistSettings {
  return { ...structuredClone(DEFAULT_ASSIST_SETTINGS), ...overrides };
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

  it("builds a feature-scoped engine without changing the fixed output contract", () => {
    const configured = settings({ enabled: true, model: "common-model" });
    configured.verifiedTarget = assistTarget(configured);
    configured.featureProfiles.work_tagging = {
      ...configured.featureProfiles.work_tagging,
      model: "tag-model",
      additionalInstructions: "短い理由だけを書く",
      inputPolicy: { includeTitle: true, includeAuthor: false, includeTags: true, maxItems: 20 },
    };

    expect(toEngine(configured, "work_tagging")).toMatchObject({
      model: "tag-model",
      featureProfile: {
        profileId: "default:work_tagging",
        featureId: "work_tagging",
        additionalInstructions: "短い理由だけを書く",
        inputPolicy: { includeAuthor: false, maxItems: 20 },
      },
    });
  });

  it("keeps the engine ready while allowing individual features to be disabled", () => {
    const configured = settings({ enabled: true, model: "model-a" });
    configured.verifiedTarget = assistTarget(configured);
    configured.featureProfiles.reader_recap = { ...configured.featureProfiles.reader_recap, enabled: false };
    expect(assistReady(configured)).toBe(true);
    expect(assistFeatureReady(configured, "reader_recap")).toBe(false);
    expect(assistFeatureReady(configured, "work_tagging")).toBe(true);
  });

  it("requires a separate trial when a feature overrides the common model", () => {
    const configured = settings({ enabled: true, model: "common-model" });
    configured.verifiedTarget = assistTarget(configured);
    configured.featureProfiles.work_tagging = {
      ...configured.featureProfiles.work_tagging,
      model: "tag-model",
      verifiedTarget: null,
    };
    expect(validateAssistSettings(configured)).toMatch(/タグの補完.*試して/);
    expect(assistFeatureReady(configured, "work_tagging")).toBe(false);

    configured.featureProfiles.work_tagging.verifiedTarget = assistTarget({
      baseUrl: configured.baseUrl,
      model: "tag-model",
    });
    expect(validateAssistSettings(configured)).toBeNull();
    expect(assistFeatureReady(configured, "work_tagging")).toBe(true);
  });
});
