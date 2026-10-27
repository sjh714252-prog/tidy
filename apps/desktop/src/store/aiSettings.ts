import { create } from "zustand";
import * as ipc from "../ipc/commands";
import type { AiProvider, AiSettings, ProviderSlot } from "../ipc/types";

export type AiTestTarget = "classify" | "embed";

/** Result of one section's "연결 테스트". */
export interface AiTestResult {
  busy: boolean;
  ok?: boolean;
  text?: string;
}

interface AiSettingsState {
  draft: AiSettings | null;
  /** Last saved values (what the backend has); `dirty()` compares against it. */
  loaded: AiSettings | null;
  /** Typed into the key field; sent once on save, then cleared. Never loaded back. */
  apiKey: string;
  /** Key for the embedding model (OpenAI-compatible); same rules as apiKey. */
  embedApiKey: string;
  busy: boolean;
  message: string | null;
  error: string | null;
  /** Reload from the backend. Unsaved edits are kept unless `force` (#8). */
  load: (force?: boolean) => Promise<void>;
  /** True when the draft or a typed key differs from what is saved. */
  dirty: () => boolean;
  patch: (p: Partial<AiSettings>) => void;
  setProvider: (p: AiProvider) => void;
  setApiKey: (k: string) => void;
  setEmbedProvider: (p: AiSettings["embedProvider"]) => void;
  setEmbedApiKey: (k: string) => void;
  save: () => Promise<boolean>;
  deleteKey: (which?: "classify" | "embed") => Promise<void>;
  /** Per-section connection test results (main AI / semantic search). */
  tests: Record<AiTestTarget, AiTestResult>;
  /** Save first if anything changed, then send one tiny request. */
  test: (which?: AiTestTarget) => Promise<void>;
}

/** Sensible defaults per provider (model names are only suggestions). */
const DEFAULTS: Record<Exclude<AiProvider, "">, Partial<AiSettings>> = {
  anthropic: { model: "claude-haiku-5-5", baseUrl: "", keyRef: "tidy/anthropic", effort: "low" },
  openai: { model: "", baseUrl: "https://api.openai.com/v1", keyRef: "tidy/openai", effort: "" },
  ollama: { model: "qwen3:8b", baseUrl: "http://localhost:11434", keyRef: "", effort: "" },
};

/** Embedding model suggestions (semantic search). */
const EMBED_DEFAULTS: Record<"openai" | "ollama", Partial<AiSettings>> = {
  openai: { embedModel: "text-embedding-3-small", embedBaseUrl: "https://api.openai.com/v1", embedKeyRef: "tidy/openai" },
  ollama: { embedModel: "nomic-embed-text", embedBaseUrl: "http://localhost:11434", embedKeyRef: "" },
};

const slotOf = (model: string, baseUrl: string, keyRef: string, effort: string, keyStored: boolean): ProviderSlot => ({
  model,
  baseUrl,
  keyRef,
  effort,
  keyStored,
});

/** Turn raw provider errors into what to do next (the raw text stays below). */
export function explainAiError(e: string): string | null {
  if (/credit balance is too low/i.test(e))
    return "Anthropic 계정의 크레딧(잔액)이 부족합니다. Tidy 문제가 아니라 계정 상태입니다 — console.anthropic.com의 Plans & Billing에서 충전하면 바로 됩니다.";
  if (/insufficient_quota|exceeded your current quota/i.test(e))
    return "OpenAI 계정의 사용 한도나 잔액이 부족합니다 — platform.openai.com의 Billing을 확인하세요.";
  if (/HTTP 401|invalid x-api-key|invalid api key|incorrect api key|authentication/i.test(e))
    return "API 키가 맞지 않습니다. 키를 다시 입력하고 저장하세요.";
  if (/no API key stored/i.test(e)) return "키체인에 이 키 이름으로 저장된 키가 없습니다. API 키를 입력하고 저장하세요.";
  if (/HTTP 404|model.*not.?found|not_found_error/i.test(e)) return "모델 이름이나 주소를 확인하세요.";
  if (/HTTP 429|rate.?limit|overloaded/i.test(e)) return "요청이 많아 잠시 막혔습니다. 조금 뒤 다시 시도하세요.";
  if (/network error|timed out|connection refused|error sending request/i.test(e))
    return "서버에 닿지 않았습니다. 주소와 네트워크를 확인하세요 (Ollama면 실행 중인지).";
  return null;
}

export const useAiSettings = create<AiSettingsState>((set, get) => ({
  draft: null,
  loaded: null,
  apiKey: "",
  embedApiKey: "",
  busy: false,
  message: null,
  error: null,
  tests: { classify: { busy: false }, embed: { busy: false } },
  load: async (force = false) => {
    try {
      const fresh = await ipc.getAiSettings();
      // Don't wipe what the user typed but hasn't saved yet (issue #8); the
      // settings tab remounts this section on every visit.
      if (force || !get().dirty()) {
        set({ draft: fresh, loaded: fresh, apiKey: "", embedApiKey: "", error: null });
      } else {
        set({ loaded: fresh });
      }
    } catch (e) {
      set({ error: String(e) });
    }
  },
  dirty: () => {
    const { loaded, draft, apiKey, embedApiKey } = get();
    if (apiKey.trim() || embedApiKey.trim()) return true;
    return !!loaded && !!draft && JSON.stringify(loaded) !== JSON.stringify(draft);
  },
  patch: (p) => {
    const d = get().draft;
    if (d) set({ draft: { ...d, ...p }, message: null });
  },
  setProvider: (provider) => {
    const d = get().draft;
    if (!d) return;
    // Remember the provider we leave; restore the one we go to (or its defaults).
    const saved = { ...d.saved };
    if (d.provider) saved[d.provider] = slotOf(d.model, d.baseUrl, d.keyRef, d.effort, d.keyStored);
    const back = provider ? saved[provider] : undefined;
    const next: Partial<AiSettings> = back
      ? { ...back }
      : provider
        ? { ...DEFAULTS[provider], keyStored: false }
        : {};
    set({ draft: { ...d, ...next, provider, saved }, message: null, tests: { ...get().tests, classify: { busy: false } } });
    if (!back && next.keyRef) {
      const ref = next.keyRef;
      void ipc.apiKeyStored(ref).then(
        (has) => {
          const cur = get().draft;
          if (cur && cur.provider === provider && cur.keyRef === ref) set({ draft: { ...cur, keyStored: has } });
        },
        () => {},
      );
    }
  },
  setApiKey: (apiKey) => set({ apiKey, message: null }),
  setEmbedProvider: (embedProvider) => {
    const d = get().draft;
    if (!d) return;
    const embedSaved = { ...d.embedSaved };
    if (d.embedProvider)
      embedSaved[d.embedProvider] = slotOf(d.embedModel, d.embedBaseUrl, d.embedKeyRef, "", d.embedKeyStored);
    const back = embedProvider ? embedSaved[embedProvider] : undefined;
    const next: Partial<AiSettings> = back
      ? { embedModel: back.model, embedBaseUrl: back.baseUrl, embedKeyRef: back.keyRef, embedKeyStored: back.keyStored }
      : embedProvider
        ? { ...EMBED_DEFAULTS[embedProvider], embedKeyStored: false }
        : {};
    set({
      draft: { ...d, ...next, embedProvider, embedSaved },
      message: null,
      tests: { ...get().tests, embed: { busy: false } },
    });
    if (!back && next.embedKeyRef) {
      const ref = next.embedKeyRef;
      void ipc.apiKeyStored(ref).then(
        (has) => {
          const cur = get().draft;
          if (cur && cur.embedProvider === embedProvider && cur.embedKeyRef === ref)
            set({ draft: { ...cur, embedKeyStored: has } });
        },
        () => {},
      );
    }
  },
  setEmbedApiKey: (embedApiKey) => set({ embedApiKey, message: null }),
  save: async () => {
    const d = get().draft;
    if (!d) return false;
    set({ busy: true, error: null, message: null });
    try {
      const saved = await ipc.saveAiSettings(d, get().apiKey.trim() || null, get().embedApiKey.trim() || null);
      set({
        draft: saved,
        loaded: saved,
        apiKey: "",
        embedApiKey: "",
        message: "AI 설정을 저장했습니다. 감시를 다시 시작하고, 의미검색 모델이 있으면 임베딩을 시작합니다.",
      });
      return true;
    } catch (e) {
      set({ error: String(e) });
      return false;
    } finally {
      set({ busy: false });
    }
  },
  deleteKey: async (which = "classify") => {
    const d = get().draft;
    const ref = which === "embed" ? d?.embedKeyRef : d?.keyRef;
    if (!d || !ref) return;
    try {
      const s = await ipc.deleteApiKey(ref);
      const next = { ...d, keyStored: s.keyStored, embedKeyStored: s.embedKeyStored };
      const loaded = get().loaded;
      set({
        draft: next,
        loaded: loaded ? { ...loaded, keyStored: s.keyStored, embedKeyStored: s.embedKeyStored } : loaded,
        message: "키체인에서 키를 지웠습니다.",
      });
    } catch (e) {
      set({ error: String(e) });
    }
  },
  test: async (which = "classify") => {
    const setResult = (r: AiTestResult) => set({ tests: { ...get().tests, [which]: r } });
    setResult({ busy: true });
    if (get().dirty() && !(await get().save())) {
      setResult({ busy: false, ok: false, text: `저장하지 못해 테스트하지 않았습니다: ${get().error ?? ""}` });
      return;
    }
    try {
      const ok = await (which === "embed" ? ipc.testEmbed() : ipc.testAi());
      setResult({ busy: false, ok: true, text: ok });
    } catch (e) {
      setResult({ busy: false, ok: false, text: String(e) });
    }
  },
}));
