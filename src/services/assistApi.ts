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
  tagCriteria?: string;
  inputPolicy?: AssistInputPolicy;
}

export type AssistFeatureId =
  | "search_interpretation"
  | "work_synopsis"
  | "work_tagging"
  | "author_style"
  | "reader_recap"
  | "collection_split"
  | "collection_naming"
  | "editor_proofread";

export interface AssistFeatureProfile {
  enabled: boolean;
  /** 空なら共通モデルを使う。 */
  model: string;
  /** Exact base URL + override model that passed a structured-output trial. */
  verifiedTarget: string | null;
  additionalInstructions: string;
  /** タグ名：判定条件を一行ずつ。未指定ならタグ名から候補を探す。 */
  tagCriteria: string;
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
  { id: "search_interpretation", label: "言葉で探す", description: "読みたい内容を、棚の検索条件に言い換える", needsBody: false },
  { id: "work_synopsis", label: "作品のあらすじ", description: "本文から、読み返すためのあらすじを作る", needsBody: true },
  { id: "work_tagging", label: "タグの補完", description: "棚にあるタグから、足りないものを提案する", needsBody: false },
  { id: "author_style", label: "作者の題材メモ", description: "モデルや通信を使わず、手元の作品タグを集計します", needsBody: false },
  { id: "reader_recap", label: "前回のあらすじ", description: "前の話の本文から要点をまとめる", needsBody: true },
  { id: "collection_split", label: "コレクションの分割案", description: "題名とタグから分け方を提案する", needsBody: false },
  { id: "collection_naming", label: "コレクションの命名", description: "内容に合う名前と説明を提案する", needsBody: false },
  { id: "editor_proofread", label: "本文の校正", description: "編集中の本文から誤字・脱字の候補を挙げる（直すかは一件ずつ選べる）", needsBody: true },
] as const;

function defaultProfiles(): Record<AssistFeatureId, AssistFeatureProfile> {
  return Object.fromEntries(ASSIST_FEATURES.map((feature) => [feature.id, {
    enabled: true,
    model: "",
    verifiedTarget: null,
    additionalInstructions: "",
    tagCriteria: "",
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
    const savedProfile = stored?.featureProfiles?.[feature.id];
    profiles[feature.id] = {
      ...profiles[feature.id],
      ...savedProfile,
      inputPolicy: { ...profiles[feature.id].inputPolicy, ...savedProfile?.inputPolicy },
    };
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
      tagCriteria: featureId === "work_tagging" ? profile.tagCriteria.trim() || undefined : undefined,
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
    return host === "localhost" || host === "127.0.0.1" || host === "::1";
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
      return "外部の宛先へ送ることを許可してください";
    }
  }
  if (settings.verifiedTarget !== assistTarget(settings)) {
    return "このつなぎ先とモデルで試し書きをしてから有効にしてください";
  }
  for (const feature of ASSIST_FEATURES) {
    const profile = settings.featureProfiles[feature.id];
    if (feature.id === "work_tagging") {
      const problem = validateTagCriteria(profile.tagCriteria);
      if (problem) return problem;
    }
    const override = profile.model.trim();
    if (feature.id !== "author_style" && profile.enabled && override && profile.verifiedTarget !== assistTarget({ baseUrl: settings.baseUrl, model: override })) {
      return `「${feature.label}」だけで使うモデルを、試してから保存してください`;
    }
  }
  return null;
}

export function validateTagCriteria(text: string): string | null {
  if (text.length > 12_000) return "タグの判定条件は合計12000文字以内にしてください";
  const seen = new Set<string>();
  for (const line of text.split(/\r?\n/).filter((line) => line.trim())) {
    const separator = line.search(/[：:]/);
    if (separator < 1 || !line.slice(separator + 1).trim()) return "タグの判定条件は「タグ名：条件」を一行ずつ入力してください";
    const tag = line.slice(0, separator).trim();
    if (!tag || tag.length > 128 || line.slice(separator + 1).trim().length > 512) return "タグ名は128文字、判定条件は一行512文字以内にしてください";
    if (seen.has(tag)) return `「${tag}」の判定条件が重複しています`;
    seen.add(tag);
  }
  return seen.size > 100 ? "タグの判定条件は100件以内にしてください" : null;
}

/** 手伝いが使える状態か。送信時と同じ検証を通った設定だけを返す。 */
export function assistReady(settings: AssistSettings | undefined): settings is AssistSettings {
  return Boolean(settings?.enabled && !validateAssistSettings(settings));
}

export function assistFeatureReady(settings: AssistSettings | undefined, featureId: AssistFeatureId): settings is AssistSettings {
  if (featureId === "author_style") {
    return Boolean(settings && settings.featureProfiles.author_style?.enabled);
  }
  if (!assistReady(settings)) return false;
  const profile = settings.featureProfiles[featureId];
  if (!profile?.enabled) return false;
  const override = profile.model.trim();
  return !override || profile.verifiedTarget === assistTarget({ baseUrl: settings.baseUrl, model: override });
}

// ---- 設定 -----------------------------------------------------------------

export interface DiscoveredEngine {
  baseUrl: string;
  /** 「LM Studio」など、そこで動いているものの名前。 */
  label: string;
  /** 名前をサーバーの応答で確かめたか。false ならポート番号からの推測。 */
  identified: boolean;
  models: string[];
}

export interface TrialResult {
  name: string;
  subtitle: string;
  elapsedMs: number;
  /** 送った作品の題名。題名を送らない設定なら空の文字列。 */
  sentTitles: string[];
  /** モデルに選ばせた名前の候補。`name` はこの中の一つ。 */
  candidates: { name: string; subtitle: string }[];
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

/** 他作品で同時に付いているタグから作った候補。内容適合を表すものではない。 */
export interface RelatedTagCandidate {
  tag: string;
  reason: string;
  matchingWorkCount: number;
  cooccurringWorkCount: number;
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

/** 棚内でのタグ併用統計から候補を返す。読み取りだけで、作品のタグは変更しない。 */
export function relatedTagCandidates(downloadId: number): Promise<RelatedTagCandidate[]> {
  return invoke<RelatedTagCandidate[]>("assist_related_tags", { downloadId });
}

/** 統計候補を利用者が採用し、manual印で付ける。 */
export function acceptRelatedTags(downloadId: number, tags: string[]): Promise<TaggedName[]> {
  return invoke<TaggedName[]>("assist_accept_related_tags", { downloadId, tags });
}

/** 利用者が付けたmanualタグだけを外す。 */
export function removeManualTag(downloadId: number, tag: string): Promise<TaggedName[]> {
  return invoke<TaggedName[]>("assist_remove_related_tag", { downloadId, tag });
}

// ---- 言葉で探す -----------------------------------------------------------

export interface SearchIntent {
  includeTags: string[];
  excludeTags: string[];
  /** includeTags の結合方法。省略された旧応答は AND として扱う。 */
  includeTagMode?: "and" | "or";
  /** 意味検索へ渡す説明。タグ以外の条件や未知語も残す。 */
  query: string;
  /** どう読んだかの一行。外れていたら利用者が気づける。 */
  reading: string;
  /** 答える前にモデルが棚で調べたこと。道具を使わなかったときは無い。 */
  consulted?: ToolTrace[];
}

/** モデルが呼んだ読み取り専用の道具。arguments はモデルが渡した JSON 文字列。 */
export interface ToolTrace {
  name: string;
  arguments: string;
  ok: boolean;
}

/** 「こういうのが読みたい」を、棚のタグと検索語に翻訳する。検索はしない。 */
export function interpretSearch(engine: AssistEngine, phrase: string): Promise<SearchIntent> {
  return invokeGenerated<SearchIntent>("assist_interpret_search", { engine, phrase });
}

// ---- 覚え書き -------------------------------------------------------------

/** 書き下ろしのあらすじの一文と、アプリが番号から引いた原文の根拠。 */
export interface GroundedClaim {
  text: string;
  /** 原文の文番号（0始まり）。 */
  sources: number[];
  /** 番号の原文。モデルが書き写したものではない。 */
  evidence: string[];
  /** 本文を見せない照合役が、根拠の文とその前後だけで判定した結果。 */
  verdict: "supported" | "partial" | "unsupported";
  concern?: string | null;
}

export interface AssistNote {
  text: string;
  /** Original source suffix supplied by the app, independently of the note text. */
  sourceTail?: string | null;
  /** 書き下ろし方式（v12〜）のときだけある。 */
  claims?: GroundedClaim[] | null;
}

/** 保存してある覚え書き。どのモデルが書いたかも残る。 */
export interface StoredNote {
  text: string;
  sourceTail?: string | null;
  claims?: GroundedClaim[] | null;
  modelId: string;
  createdAt: string;
  featureId: AssistFeatureId;
  promptVersion: string;
  inputFingerprint: string;
  configFingerprint: string;
  promptStale: boolean;
  inputStale: boolean;
  /** null means the current model settings were unavailable for comparison. */
  configStale: boolean | null;
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

// ---- 校正 -----------------------------------------------------------------

/** 校正してほしい本文のひとまとまり。`id` は編集画面のブロックの身元。 */
export interface ProofreadSegment {
  id: string;
  text: string;
}

/** 直す候補。`before` はその段落にそのまま一度だけ現れる文字列。 */
export interface ProofreadSuggestion {
  segmentId: string;
  before: string;
  after: string;
  kind: "誤字" | "脱字" | "衍字" | "変換ミス" | "表記ゆれ" | "記号" | string;
  reason: string;
}

/** 一回に送れる量。Rust 側の上限（6,000字・60段落）より少し控えめにまとめる。 */
export const PROOFREAD_BATCH_CHARS = 3_000;
export const PROOFREAD_BATCH_SEGMENTS = 40;

/** 編集中の本文から誤字・脱字の候補を挙げてもらう。**直しはしない。** */
export function proofreadSegments(engine: AssistEngine, segments: ProofreadSegment[]): Promise<ProofreadSuggestion[]> {
  return invokeGenerated<ProofreadSuggestion[]>("assist_proofread", { engine, segments });
}

/** 保存してある覚え書きを読む。**モデルを呼ばない。** */
export function loadNote(subjectType: NoteSubject, subjectKey: string, noteKind: string, engine?: AssistEngine | null): Promise<StoredNote | null> {
  return invoke<StoredNote | null>("assist_load_note", { subjectType, subjectKey, noteKind, engine: engine ?? null });
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

/** あらすじ・前回のあらすじを作っているあいだの段階（`assist-progress` イベント）。 */
export interface AssistProgressEvent {
  feature: AssistFeatureId;
  downloadId: number;
  stage: "reading" | "combining" | "drafting" | "checking" | "revising" | "rechecking" | "writing";
  /** 長編を区切って読むときの、何番目の区切りか（1から）。 */
  section: number | null;
  sections: number | null;
  /** 答えの形が崩れて、同じ段階を頼み直しているところか。 */
  retry: boolean;
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
