import { describe, expect, it, vi } from "vitest";
import { getSetting } from "@/services/settingsApi";
import {
  DEFAULT_ASSIST_SETTINGS,
  assistReady,
  assistFeatureReady,
  assistTarget,
  isLocalAssistUrl,
  loadAssistSettings,
  normalizeAssistBaseUrl,
  toEngine,
  validateAssistSettings,
  validateTagCriteria,
  type AssistSettings,
} from "@/services/assistApi";

vi.mock("@/services/settingsApi", () => ({ getSetting: vi.fn(), setSetting: vi.fn() }));

function settings(overrides: Partial<AssistSettings>): AssistSettings {
  return { ...structuredClone(DEFAULT_ASSIST_SETTINGS), ...overrides };
}

describe("assist settings safety", () => {
  it("does not treat a wildcard bind address as a local destination", () => {
    expect(isLocalAssistUrl("http://127.0.0.1:1234/v1")).toBe(true);
    expect(isLocalAssistUrl("http://0.0.0.0:1234/v1")).toBe(false);
  });
  it("validates classification criteria and sends them only to tagging", () => {
    expect(validateTagCriteria("園芸：植物の手入れ\n料理: 調理の描写")).toBeNull();
    expect(validateTagCriteria("園芸：植物\n園芸: 手入れ")).toMatch(/重複/);
    expect(validateTagCriteria("園芸：")).toMatch(/一行/);
    const value = settings({});
    value.featureProfiles.work_tagging.tagCriteria = "園芸：植物の手入れ";
    expect(toEngine(value, "work_tagging").featureProfile?.tagCriteria).toBe("園芸：植物の手入れ");
    expect(toEngine(value, "collection_naming").featureProfile?.tagCriteria).toBeUndefined();
  });
  it("keeps default send restrictions when a saved input policy has only some fields", async () => {
    vi.mocked(getSetting).mockResolvedValueOnce({
      featureProfiles: { work_tagging: { inputPolicy: { includeTags: false } } },
    });
    const loaded = await loadAssistSettings();
    const engine = toEngine(loaded, "work_tagging");
    expect(engine.featureProfile?.inputPolicy).toMatchObject({
      includeTitle: true, includeTags: false, includeExcerpt: false,
    });
  });

  it("preserves an explicit overview opt-in while filling omitted input fields", async () => {
    vi.mocked(getSetting).mockResolvedValueOnce({
      featureProfiles: { work_tagging: { inputPolicy: { includeExcerpt: true } } },
    });
    const loaded = await loadAssistSettings();
    expect(toEngine(loaded, "work_tagging").featureProfile?.inputPolicy).toMatchObject({
      includeTitle: true, includeTags: true, includeExcerpt: true,
    });
  });

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

  it("keeps native author statistics ready without model settings or trials", () => {
    const localOnly = settings({ enabled: false, model: "" });
    localOnly.featureProfiles.author_style.model = "saved-native-override";
    expect(assistFeatureReady(localOnly, "author_style")).toBe(true);
    expect(assistFeatureReady(undefined, "author_style")).toBe(false);

    localOnly.featureProfiles.author_style.enabled = false;
    expect(assistFeatureReady(localOnly, "author_style")).toBe(false);
    expect(assistFeatureReady(localOnly, "reader_recap")).toBe(false);

    const configured = settings({ enabled: true, model: "model-a" });
    configured.verifiedTarget = assistTarget(configured);
    configured.featureProfiles.author_style.model = "saved-native-override";
    expect(validateAssistSettings(configured)).toBeNull();
    expect(assistFeatureReady(configured, "author_style")).toBe(true);

    configured.featureProfiles.work_tagging.model = "unverified-llm-override";
    expect(validateAssistSettings(configured)).toMatch(/タグの補完.*試して/);
    expect(assistFeatureReady(configured, "work_tagging")).toBe(false);
  });
});
