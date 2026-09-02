import { invoke } from "@tauri-apps/api/core";
import { getSetting, setSetting } from "@/services/settingsApi";
import type { CollectionSuggestion } from "@/types/collections";

/**
 * 手伝いを頼む推論エンジン。
 *
 * piep はモデルを同梱しない。OpenAI 互換のエンドポイントを指すだけなので、
 * LM Studio でも Ollama でも llama.cpp でも同じ設定で繋がる。
 */
export interface AssistEngine {
  baseUrl: string;
  model: string;
  /** 利用者が送信を許可した外部URL。現在の baseUrl と一致するときだけ有効。 */
  remoteConsentUrl: string | null;
  /** **本文を送ることを、利用者が明示的に許したか。** 既定は許さない。 */
  allowBody: boolean;
  /** 機能別の指示と、送信してよい情報の上限。 */
  featureProfile?: AssistRequestProfile;
}

export interface AssistInputPolicy {
  includeTitle?: boolean;
  includeAuthor?: boolean;
  includeTags?: boolean;
  includeExcerpt?: boolean;
  maxItems?: number;
  maxTagsPerItem?: number;
}

export interface AssistRequestProfile {
  profileId: string;
  featureId: AssistFeatureId;
  additionalInstructions?: string;
  inputPolicy?: AssistInputPolicy;
}

export type AssistFeatureId =
  | "search_interpretation"
  | "work_synopsis"
  | "work_tagging"
  | "author_style"
  | "reader_recap"
  | "collection_split"
  | "collection_naming";

export interface AssistFeatureProfile {
  enabled: boolean;
  /** 空なら共通モデルを使う。 */
  model: string;
  /** Exact base URL + override model that passed a structured-output trial. */
  verifiedTarget: string | null;
  additionalInstructions: string;
  inputPolicy: AssistInputPolicy;
}

export interface AssistSettings extends AssistEngine {
  enabled: boolean;
  /** 試し書きに成功した URL とモデル。URL/モデルを変えたら破棄する。 */
  verifiedTarget: string | null;
  featureProfiles: Record<AssistFeatureId, AssistFeatureProfile>;
}

const SETTING_KEY = "assist_engine";
export const ASSIST_FEATURES: ReadonlyArray<{
  id: AssistFeatureId;
  label: string;
  description: string;
  needsBody: boolean;
}> = [
  { id: "search_interpretation", label: "言葉で探す", description: "読みたい内容を棚の条件へ翻訳", needsBody: false },
  { id: "work_synopsis", label: "作品のあらすじ", description: "本文から再読用の短い覚え書きを作成", needsBody: true },
  { id: "work_tagging", label: "タグの補完", description: "棚にあるタグだけを候補として提案", needsBody: false },
  { id: "author_style", label: "作者の作風メモ", description: "題名とタグから傾向を要約", needsBody: false },
  { id: "reader_recap", label: "前回のあらすじ", description: "前の作品の本文から要点を作成", needsBody: true },
  { id: "collection_split", label: "コレクションの分割案", description: "題名とタグから分け方を提案", needsBody: false },
  { id: "collection_naming", label: "コレクションの命名", description: "内容に沿う名前と説明を提案", needsBody: false },
] as const;

function defaultProfiles(): Record<AssistFeatureId, AssistFeatureProfile> {
  return Object.fromEntries(ASSIST_FEATURES.map((feature) => [feature.id, {
    enabled: true,
    model: "",
    verifiedTarget: null,
    additionalInstructions: "",
    inputPolicy: {
      includeTitle: true,
      includeAuthor: true,
      includeTags: true,
      includeExcerpt: feature.needsBody,
      maxItems: 200,
      maxTagsPerItem: 30,
    },
  }])) as Record<AssistFeatureId, AssistFeatureProfile>;
}

/** 既定は切ってある。切ったままでも piep は完結する。 */
export const DEFAULT_ASSIST_SETTINGS: AssistSettings = {
  enabled: false,
  // LM Studio の既定。Ollama なら http://127.0.0.1:11434/v1。
  baseUrl: "http://127.0.0.1:1234/v1",
  model: "",
  remoteConsentUrl: null,
  allowBody: false,
  verifiedTarget: null,
  featureProfiles: defaultProfiles(),
};

export async function loadAssistSettings(): Promise<AssistSettings> {
  const stored = await getSetting<Partial<AssistSettings>>(SETTING_KEY);
  const profiles = defaultProfiles();
  for (const feature of ASSIST_FEATURES) {
    profiles[feature.id] = { ...profiles[feature.id], ...stored?.featureProfiles?.[feature.id] };
  }
  return {
    ...DEFAULT_ASSIST_SETTINGS,
    ...stored,
    featureProfiles: profiles,
  };
}

export function saveAssistSettings(settings: AssistSettings): Promise<void> {
  const problem = validateAssistSettings(settings);
  if (problem) return Promise.reject(new Error(problem));
  const baseUrl = normalizeAssistBaseUrl(settings.baseUrl) ?? settings.baseUrl.trim();
  const remoteConsentUrl = settings.remoteConsentUrl
    ? normalizeAssistBaseUrl(settings.remoteConsentUrl)
    : null;
  return setSetting(SETTING_KEY, {
    ...settings,
    baseUrl,
    model: settings.model.trim(),
    remoteConsentUrl,
  });
}

/** 設定から、送るときの形だけを取り出す。 */
export function toEngine(settings: AssistSettings, featureId: AssistFeatureId = "collection_naming"): AssistEngine {
  const profile = settings.featureProfiles[featureId];
  return {
    baseUrl: normalizeAssistBaseUrl(settings.baseUrl) ?? settings.baseUrl.trim(),
    model: profile.model.trim() || settings.model.trim(),
    remoteConsentUrl: settings.remoteConsentUrl
      ? normalizeAssistBaseUrl(settings.remoteConsentUrl)
      : null,
    allowBody: settings.allowBody,
    featureProfile: {
      profileId: `default:${featureId}`,
      featureId,
      additionalInstructions: profile.additionalInstructions.trim() || undefined,
      inputPolicy: profile.inputPolicy,
    },
  };
}

export function normalizeAssistBaseUrl(raw: string): string | null {
  try {
    const url = new URL(raw.trim());
    if ((url.protocol !== "http:" && url.protocol !== "https:") || !url.hostname || url.username || url.password || url.search || url.hash) {
      return null;
    }
    return url.toString().replace(/\/+$/, "");
  } catch {
    return null;
  }
}

export function isLocalAssistUrl(raw: string): boolean {
  try {
    const host = new URL(raw.trim()).hostname;
    return host === "localhost" || host === "127.0.0.1" || host === "::1" || host === "0.0.0.0";
  } catch {
    return false;
  }
}

export function assistTarget(settings: Pick<AssistSettings, "baseUrl" | "model">): string | null {
  const base = normalizeAssistBaseUrl(settings.baseUrl);
  const model = settings.model.trim();
  return base && model ? `${base}\n${model}` : null;
}

export function validateAssistSettings(settings: AssistSettings): string | null {
  if (!settings.enabled) return null;
  const base = normalizeAssistBaseUrl(settings.baseUrl);
  if (!base || !settings.model.trim()) return "つなぎ先とモデルを指定してください";
  if (!isLocalAssistUrl(base)) {
    if (!base.startsWith("https://")) return "外部のつなぎ先には HTTPS が必要です";
    if (normalizeAssistBaseUrl(settings.remoteConsentUrl ?? "") !== base) {
      return "現在の外部宛先へ送ることを明示的に許可してください";
    }
  }
  if (settings.verifiedTarget !== assistTarget(settings)) {
    return "このつなぎ先とモデルで試し書きをしてから有効にしてください";
  }
  for (const feature of ASSIST_FEATURES) {
    const profile = settings.featureProfiles[feature.id];
    const override = profile.model.trim();
    if (profile.enabled && override && profile.verifiedTarget !== assistTarget({ baseUrl: settings.baseUrl, model: override })) {
      return `「${feature.label}」の機能別モデルを試してから保存してください`;
    }
  }
  return null;
}

/** 手伝いが使える状態か。送信時と同じ検証を通った設定だけを返す。 */
export function assistReady(settings: AssistSettings | undefined): settings is AssistSettings {
  return Boolean(settings?.enabled && !validateAssistSettings(settings));
}

export function assistFeatureReady(settings: AssistSettings | undefined, featureId: AssistFeatureId): settings is AssistSettings {
  if (!assistReady(settings)) return false;
  const profile = settings.featureProfiles[featureId];
  if (!profile?.enabled) return false;
  const override = profile.model.trim();
  return !override || profile.verifiedTarget === assistTarget({ baseUrl: settings.baseUrl, model: override });
}

// ---- 設定 -----------------------------------------------------------------

export interface DiscoveredEngine {
  baseUrl: string;
  /** 「LM Studio」など、そこで動いていそうなものの名前。 */
  label: string;
  models: string[];
}

export interface TrialResult {
  name: string;
  subtitle: string;
  elapsedMs: number;
}

/** ホストとロード済みモデルから安全側に決めた実行プロファイル。 */
export interface AssistRuntimeProfile {
  localServer: boolean;
  logicalCpuCores: number;
  availableMemoryBytes: number | null;
  contextLength: number | null;
  evalBatchSize: number | null;
  serverParallelism: number | null;
  flashAttention: boolean | null;
  kvCacheOnGpu: boolean | null;
  concurrentRequests: number;
  summaryChunkChars: number;
  summaryMergeChars: number;
}

/**
 * この端末で動いている推論サーバーを探す。
 *
 * 利用者に番号を打たせない。よくある置き場所をこちらから叩いて、応答した
 * ものだけ返す。全部止まっていても2秒足らずで終わる。
 */
export function discoverAssistEngines(): Promise<DiscoveredEngine[]> {
  return invoke<DiscoveredEngine[]>("assist_discover_engines");
}

/** 現在のモデルを載せ直さず、広告されている能力だけから自動調整する。 */
export function getAssistRuntimeProfile(engine: AssistEngine): Promise<AssistRuntimeProfile> {
  return invoke<AssistRuntimeProfile>("assist_runtime_profile", { engine });
}

/** 設定したエンジンを、実際の仕事で試す。保存する前に、何が返るのかを見せる。 */
export function tryAssistEngine(engine: AssistEngine, collectionId?: string): Promise<TrialResult> {
  return invoke<TrialResult>("db_try_naming_engine", { engine, collectionId: collectionId ?? null });
}

// ---- タグ -----------------------------------------------------------------

/** 出どころの付いたタグ。`origin` は取得元、`llm` はモデルの案から採ったもの。 */
export interface TaggedName {
  name: string;
  source: "origin" | "manual" | "llm" | string;
}

export interface TagProposal {
  tag: string;
  /** なぜそのタグなのか。題名や概要のどこを見たか。 */
  reason: string;
}

/** この作品に足りていないタグを、棚の語彙から挙げてもらう。付けはしない。 */
export function suggestTags(engine: AssistEngine, downloadId: number): Promise<TagProposal[]> {
  return invokeGenerated<TagProposal[]>("assist_suggest_tags", { engine, downloadId });
}

/** 案から選んだタグを、`llm` 印で付ける。 */
export function acceptTags(downloadId: number, tags: string[]): Promise<TaggedName[]> {
  return invoke<TaggedName[]>("assist_accept_tags", { downloadId, tags });
}

/** 出どころ付きのタグ一覧。 */
export function workTags(downloadId: number): Promise<TaggedName[]> {
  return invoke<TaggedName[]>("assist_work_tags", { downloadId });
}

/** モデルの案から採ったタグを外す。取得元のタグは外せない。 */
export function removeAssistedTag(downloadId: number, tag: string): Promise<TaggedName[]> {
  return invoke<TaggedName[]>("assist_remove_tag", { downloadId, tag });
}

// ---- 言葉で探す -----------------------------------------------------------

export interface SearchIntent {
  includeTags: string[];
  excludeTags: string[];
  /** 意味検索へ渡す言い換え。タグで表せない部分がここに残る。 */
  query: string;
  /** どう読んだかの一行。外れていたら利用者が気づける。 */
  reading: string;
}

/** 「こういうのが読みたい」を、棚のタグと検索語に翻訳する。検索はしない。 */
export function interpretSearch(engine: AssistEngine, phrase: string): Promise<SearchIntent> {
  return invokeGenerated<SearchIntent>("assist_interpret_search", { engine, phrase });
}

// ---- 覚え書き -------------------------------------------------------------

export interface AssistNote {
  text: string;
}

/** 保存してある覚え書き。どのモデルが書いたかも残る。 */
export interface StoredNote {
  text: string;
  modelId: string;
  createdAt: string;
  featureId: AssistFeatureId;
  promptVersion: string;
  inputFingerprint: string;
  configFingerprint: string;
  promptStale: boolean;
  inputStale: boolean;
}

export interface AssistProvenance {
  featureId: AssistFeatureId;
  promptVersion: string;
  modelId: string;
  inputFingerprint: string;
  configFingerprint: string;
  createdAt: string;
}

export interface GeneratedAssist<T> {
  value: T;
  provenance: AssistProvenance;
}

async function invokeGenerated<T>(command: string, args: Record<string, unknown>): Promise<T> {
  return (await invoke<GeneratedAssist<T>>(command, args)).value;
}

export type NoteSubject = "work" | "person" | "collection";

/** 保存してある覚え書きを読む。**モデルを呼ばない。** */
export function loadNote(subjectType: NoteSubject, subjectKey: string, noteKind: string): Promise<StoredNote | null> {
  return invoke<StoredNote | null>("assist_load_note", { subjectType, subjectKey, noteKind });
}

/** 覚え書きを消す。作り直したいときと、要らなくなったとき。 */
export function deleteNote(subjectType: NoteSubject, subjectKey: string, noteKind: string): Promise<boolean> {
  return invoke<boolean>("assist_delete_note", { subjectType, subjectKey, noteKind });
}

/** この作者の作風を、題名とタグからまとめてもらう。本文は送らない。 */
export function describeAuthor(engine: AssistEngine, source: string, personKey: string): Promise<AssistNote> {
  return invokeGenerated<AssistNote>("assist_describe_author", { engine, source, personKey });
}

/** 本文から、あとで思い出すためのあらすじを作ってもらう。**本文を送る。** */
export function summarizeWork(engine: AssistEngine, downloadId: number): Promise<AssistNote> {
  return invokeGenerated<AssistNote>("assist_summarize_work", { engine, downloadId });
}

/** 直前の話の要点を出す。**本文を送る。** */
export function recapPrevious(
  engine: AssistEngine,
  previousDownloadId: number,
  currentDownloadId: number,
): Promise<AssistNote> {
  return invokeGenerated<AssistNote>("assist_recap_previous", { engine, previousDownloadId, currentDownloadId });
}

// ---- 束 -------------------------------------------------------------------

export interface BundleSplit {
  name: string;
  /** この塊に入る作品の、一覧での位置（0 始まり）。 */
  positions: number[];
  reason: string;
}

/** この束を分けたほうがよいか、案を出してもらう。分けはしない。 */
export function proposeSplits(engine: AssistEngine, collectionId: string): Promise<BundleSplit[]> {
  return invokeGenerated<BundleSplit[]>("assist_propose_splits", { engine, collectionId });
}

/** 提案の名前を、モデルにも考えてもらう。既存の案に足すだけで置き換えない。 */
export function nameCollectionSuggestion(suggestionId: string, engine: AssistEngine): Promise<CollectionSuggestion> {
  return invoke<CollectionSuggestion>("db_name_collection_suggestion", { suggestionId, engine });
}

/** モデルが返した束の名前と、一行の説明。 */
export interface NamedBundle {
  name: string;
  subtitle: string;
}

/** すでにあるコレクションの名前と説明を、モデルにも考えてもらう。 */
export function nameCollectionWithModel(collectionId: string, engine: AssistEngine): Promise<GeneratedAssist<NamedBundle>> {
  return invoke<GeneratedAssist<NamedBundle>>("db_name_collection_with_model", { collectionId, engine });
}
